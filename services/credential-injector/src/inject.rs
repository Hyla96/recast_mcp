//! `POST /inject` handler.
//!
//! # Security checks (both occur **before** any DB access)
//!
//! 1. Caller IP must be present in `MCP_INJECTOR_ALLOWED_CALLER_IPS`.
//! 2. `Authorization: Bearer <secret>` must equal `MCP_INJECTOR_SHARED_SECRET`.
//!
//! # Flow
//!
//! Credential lookup (cache → DB) → AES-256-GCM decrypt → SSRF re-validation
//! (Phase 1 + DNS) → upstream HTTP request with injected auth → zeroize
//! plaintext → emit `CredentialAccess` audit event → proxy response.

use std::{collections::HashMap, net::SocketAddr};

use axum::{
    extract::{ConnectInfo, State},
    http::HeaderMap,
    Json,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use mcp_common::{validate_url_with_dns, AppError, AuditAction, AuditEvent, SanitizedErrorMsg};
use mcp_crypto::decrypt;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use uuid::Uuid;

use crate::{app_state::AppState, cache::CachedCredential};

// ── Request / response types ──────────────────────────────────────────────────

/// JSON body accepted by `POST /inject`.
#[derive(Debug, Serialize, Deserialize)]
pub struct RequestSkeleton {
    /// The MCP server whose stored credential should be injected into the
    /// upstream request.
    pub server_id: Uuid,
    /// HTTP method for the upstream request (e.g. `"GET"`, `"POST"`).
    pub method: String,
    /// Full upstream URL. Must pass SSRF validation (Phase 1 + DNS).
    pub url: String,
    /// Additional request headers merged with the injected auth header.
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// Optional JSON body forwarded to the upstream.
    pub body: Option<serde_json::Value>,
}

/// Response returned by a successful `POST /inject`.
///
/// HTTP status is always **200** regardless of what the upstream returned.
/// The upstream's own status code is carried in the `status` field so the
/// gateway can make routing decisions without re-parsing the body.
#[derive(Debug, Serialize)]
pub struct InjectResponse {
    /// HTTP status code returned by the upstream.
    pub status: u16,
    /// Upstream response body, parsed as JSON.
    /// Falls back to `null` if the body is empty or not valid JSON.
    pub body: serde_json::Value,
    /// Upstream response headers (lowercase header names).
    pub headers: HashMap<String, String>,
}

// ── Handler ───────────────────────────────────────────────────────────────────

/// Axum handler for `POST /inject`.
///
/// Validates the caller, resolves and decrypts the credential, runs SSRF
/// protection, makes the upstream HTTP call with the credential injected, then
/// returns the upstream response to the gateway.
///
/// # Errors
///
/// | Condition                          | HTTP status |
/// |------------------------------------|-------------|
/// | IP not in allowlist                | 403         |
/// | Wrong shared secret                | 403         |
/// | No credential for `server_id`      | 404         |
/// | URL blocked by SSRF protection     | 422         |
/// | Upstream timed out                 | 504         |
/// | Upstream unreachable               | 502         |
pub async fn inject_handler(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(skeleton): Json<RequestSkeleton>,
) -> Result<Json<InjectResponse>, AppError> {
    // ── 1. IP allowlist check (before any DB access) ───────────────────────
    let caller_ip = addr.ip();
    if !state.allowed_ips.contains(&caller_ip) {
        tracing::warn!(caller_ip = %caller_ip, "inject: caller IP not in allowlist");
        return Err(AppError::Forbidden(format!(
            "caller IP {caller_ip} is not in the allowed list"
        )));
    }

    // ── 2. Shared secret check (before any DB access) ──────────────────────
    let provided_secret = extract_bearer_token(&headers).map_err(|()| {
        AppError::Forbidden("missing or invalid Authorization header".to_string())
    })?;
    if provided_secret != state.shared_secret.as_str() {
        tracing::warn!("inject: invalid shared secret — rejecting request");
        return Err(AppError::Forbidden("invalid shared secret".to_string()));
    }

    let server_id = skeleton.server_id;

    // ── 3. Credential lookup: cache → DB ───────────────────────────────────
    // Lock scope: acquire, clone result, release — NEVER hold across .await.
    let cached = {
        let mut guard = state.cache.lock().unwrap_or_else(|e| e.into_inner());
        guard.get(&server_id).cloned()
    };

    let credential: CachedCredential = match cached {
        Some(c) => c,
        None => {
            // Cache miss: query DB for the most recent credential for this server.
            let row = sqlx::query(
                "SELECT encrypted_payload, auth_type, key_name
                 FROM credentials
                 WHERE server_id = $1
                 ORDER BY created_at DESC
                 LIMIT 1",
            )
            .bind(server_id)
            .fetch_optional(&state.pool)
            .await
            .map_err(|e| {
                AppError::InternalServerError(format!("credential DB lookup failed: {e}"))
            })?
            .ok_or(AppError::CredentialNotFound)?;

            let encrypted_payload: Vec<u8> = row.try_get("encrypted_payload").map_err(|e| {
                AppError::InternalServerError(format!("row decode encrypted_payload: {e}"))
            })?;
            let auth_type: String = row.try_get("auth_type").map_err(|e| {
                AppError::InternalServerError(format!("row decode auth_type: {e}"))
            })?;
            let key_name: Option<String> = row.try_get("key_name").map_err(|e| {
                AppError::InternalServerError(format!("row decode key_name: {e}"))
            })?;

            let cred = CachedCredential {
                encrypted_payload,
                auth_type,
                key_name,
            };

            // Insert into cache. Re-acquire lock; still no .await while held.
            {
                let mut guard = state.cache.lock().unwrap_or_else(|e| e.into_inner());
                guard.put(server_id, cred.clone());
            }

            cred
        }
    };

    // ── 4. Decrypt credential (decrypt returns Zeroizing<Vec<u8>>) ─────────
    let plaintext = decrypt(&state.crypto_key, &credential.encrypted_payload).map_err(|_| {
        AppError::InternalServerError("credential decryption failed".to_string())
    })?;

    // ── 5. SSRF re-validation: Phase 1 (sync) + Phase 2 (DNS async) ────────
    // `skip_ssrf` is only ever `true` in integration tests that direct the
    // injector at a MockUpstream on 127.0.0.1; it is always `false` in
    // production (set by `build_app_state`).
    let parsed_url = url::Url::parse(&skeleton.url)
        .map_err(|_| AppError::BadRequest(format!("invalid upstream URL: {}", skeleton.url)))?;
    if !state.skip_ssrf {
        validate_url_with_dns(&parsed_url).await?;
    }

    // ── 6. Build upstream request with injected credential ─────────────────
    let method = reqwest::Method::from_bytes(skeleton.method.as_bytes()).map_err(|_| {
        AppError::BadRequest(format!("invalid HTTP method: {}", skeleton.method))
    })?;

    let mut req_builder = state.http_client.request(method, skeleton.url.as_str());

    // Merge skeleton headers (auth header will be added below, overriding any
    // Authorization header the caller may have included in skeleton.headers).
    for (key, value) in &skeleton.headers {
        req_builder = req_builder.header(key.as_str(), value.as_str());
    }

    // Convert plaintext bytes to UTF-8. This is the only point where the
    // decrypted value is held as a &str — it is NEVER logged or stored.
    let plaintext_str = std::str::from_utf8(&plaintext).map_err(|_| {
        AppError::InternalServerError("credential value is not valid UTF-8".to_string())
    })?;

    req_builder = match credential.auth_type.as_str() {
        "bearer" => req_builder.bearer_auth(plaintext_str),
        "api_key_header" => {
            let name = credential.key_name.as_deref().unwrap_or("X-Api-Key");
            req_builder.header(name, plaintext_str)
        }
        "api_key_query" => {
            let name = credential.key_name.as_deref().unwrap_or("api_key");
            req_builder.query(&[(name, plaintext_str)])
        }
        "basic" => {
            // Treat the stored value as "user:password". Base64-encode the
            // entire string per RFC 7617 §2.
            let encoded = STANDARD.encode(plaintext_str.as_bytes());
            req_builder.header("Authorization", format!("Basic {encoded}"))
        }
        other => {
            return Err(AppError::InternalServerError(format!(
                "unknown auth_type in stored credential: {other}"
            )));
        }
    };

    if let Some(ref body) = skeleton.body {
        req_builder = req_builder.json(body);
    }

    // ── 7. Execute upstream request ────────────────────────────────────────
    let upstream_result = req_builder.send().await;

    // Plaintext is a Zeroizing<Vec<u8>> — memory is zeroed when the binding
    // goes out of scope. Explicit drop after the HTTP call ensures it happens
    // as early as possible, before we await response body parsing.
    drop(plaintext);

    let response = match upstream_result {
        Ok(r) => r,
        Err(e) => {
            let app_err = if e.is_timeout() {
                AppError::UpstreamTimeout
            } else {
                AppError::UpstreamUnreachable { reason: e.to_string() }
            };

            state.audit_logger.log(AuditEvent {
                action: AuditAction::CredentialAccessFailure,
                user_id: None,
                server_id: Some(server_id),
                success: false,
                error_msg: Some(SanitizedErrorMsg::new(app_err.code())),
                metadata: None,
                correlation_id: None,
            });

            return Err(app_err);
        }
    };

    let upstream_status = response.status().as_u16();

    let resp_headers: HashMap<String, String> = response
        .headers()
        .iter()
        .filter_map(|(k, v)| v.to_str().ok().map(|v| (k.to_string(), v.to_string())))
        .collect();

    let resp_body: serde_json::Value = response.json().await.unwrap_or(serde_json::Value::Null);

    // ── 8. Audit event ─────────────────────────────────────────────────────
    state.audit_logger.log(AuditEvent {
        action: AuditAction::CredentialAccess,
        user_id: None,
        server_id: Some(server_id),
        success: true,
        error_msg: None,
        metadata: Some(serde_json::json!({ "upstream_status": upstream_status })),
        correlation_id: None,
    });

    Ok(Json(InjectResponse {
        status: upstream_status,
        body: resp_body,
        headers: resp_headers,
    }))
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Extracts the raw Bearer token string from the `Authorization` header.
///
/// Returns `Err(())` when the header is absent, non-ASCII, or does not follow
/// the `Bearer <token>` format.
fn extract_bearer_token(headers: &HeaderMap) -> Result<String, ()> {
    let auth_value = headers
        .get(axum::http::header::AUTHORIZATION)
        .ok_or(())?
        .to_str()
        .map_err(|_| ())?;

    auth_value
        .strip_prefix("Bearer ")
        .map(|s| s.to_string())
        .ok_or(())
}
