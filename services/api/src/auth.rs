//! Clerk JWT authentication middleware and JWKS cache.
//!
//! The middleware validates RS256 JWTs issued by Clerk, caches the JWKS with
//! a 5-minute TTL (refreshing automatically on unknown `kid`), upserts the
//! user record in the local database, and injects [`AuthenticatedUser`] into
//! request extensions for downstream handlers.

use axum::{
    extract::{Request, State},
    middleware::Next,
    response::{IntoResponse, Response},
};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use mcp_common::{middleware::RequestId, AppError, AuditAction, AuditEvent, SanitizedErrorMsg};
use reqwest::Client;
use serde::Deserialize;
use sqlx::Row;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::app_state::AppState;

// ── Constants ─────────────────────────────────────────────────────────────────

const JWKS_TTL: Duration = Duration::from_secs(300); // 5 minutes

// ── JWKS types ────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct JwkSet {
    keys: Vec<Jwk>,
}

#[derive(Debug, Deserialize, Clone)]
struct Jwk {
    /// Key ID — used to match the `kid` header in a JWT.
    kid: String,
    /// Key type — must be `"RSA"` for Clerk's RS256 tokens.
    kty: String,
    /// Base64url-encoded RSA modulus.
    n: String,
    /// Base64url-encoded RSA public exponent.
    e: String,
}

struct CachedJwks {
    keys: Vec<Jwk>,
    fetched_at: Instant,
}

// ── JwksCache ─────────────────────────────────────────────────────────────────

/// Thread-safe JWKS cache with a 5-minute TTL.
///
/// On first access (or after TTL expiry) the cache fetches the JWK set from
/// `jwks_url`. On an unknown `kid`, the cache is refreshed immediately to
/// handle key rotations.
///
/// The `jwks_url` is injectable so tests can point the cache at a
/// [`mcp_common::testing::MockUpstream`] instead of the live Clerk endpoint.
#[derive(Clone)]
pub struct JwksCache {
    inner: Arc<RwLock<Option<CachedJwks>>>,
    /// JWKS endpoint URL — injected at construction time for testability.
    pub jwks_url: String,
    client: Client,
}

impl JwksCache {
    /// Creates a new `JwksCache` targeting the given JWKS endpoint URL.
    pub fn new(jwks_url: impl Into<String>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(None)),
            jwks_url: jwks_url.into(),
            client: Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
        }
    }

    /// Returns the [`DecodingKey`] for the given key ID.
    ///
    /// - Serves from cache when the cache is warm and the key is present.
    /// - Refreshes from the network when the cache is cold, expired, or the
    ///   `kid` is absent.
    pub async fn get_key(&self, kid: &str) -> Result<DecodingKey, AppError> {
        // Fast path: cache is warm and TTL has not expired.
        {
            let guard = self.inner.read().await;
            if let Some(cached) = guard.as_ref() {
                if cached.fetched_at.elapsed() < JWKS_TTL {
                    if let Some(key) = find_decoding_key(&cached.keys, kid) {
                        return Ok(key);
                    }
                    // kid not found — fall through to a cache refresh (key rotation)
                }
            }
        }

        // Slow path: fetch a fresh JWKS and update the cache.
        self.refresh().await?;

        let guard = self.inner.read().await;
        guard
            .as_ref()
            .and_then(|c| find_decoding_key(&c.keys, kid))
            .ok_or_else(|| {
                AppError::Unauthorized(format!(
                    "No matching RSA key found for kid '{kid}'."
                ))
            })
    }

    async fn refresh(&self) -> Result<(), AppError> {
        let resp = self
            .client
            .get(&self.jwks_url)
            .send()
            .await
            .map_err(|e| {
                tracing::error!("JWKS fetch error: {e}");
                AppError::InternalServerError("Failed to fetch JWKS.".to_string())
            })?;

        let body: serde_json::Value = resp.json().await.map_err(|e| {
            tracing::error!("JWKS parse error: {e}");
            AppError::InternalServerError("Failed to parse JWKS response.".to_string())
        })?;

        let jwk_set: JwkSet = serde_json::from_value(body).map_err(|e| {
            tracing::error!("JWKS deserialize error: {e}");
            AppError::InternalServerError("Invalid JWKS format.".to_string())
        })?;

        let mut guard = self.inner.write().await;
        *guard = Some(CachedJwks {
            keys: jwk_set.keys,
            fetched_at: Instant::now(),
        });
        Ok(())
    }
}

fn find_decoding_key(keys: &[Jwk], kid: &str) -> Option<DecodingKey> {
    keys.iter()
        .find(|k| k.kty == "RSA" && k.kid == kid)
        .and_then(|k| DecodingKey::from_rsa_components(&k.n, &k.e).ok())
}

// ── AuthenticatedUser ─────────────────────────────────────────────────────────

/// A successfully validated and upserted platform user.
///
/// Injected into request extensions by [`clerk_jwt_middleware`] after JWT
/// validation and database upsert succeed. Handlers extract it via
/// `Extension<AuthenticatedUser>`.
///
/// `clerk_id` is used by TASK-017+ handlers (webhook processing, server
/// ownership checks). The field is declared `#[allow(dead_code)]` so the
/// compiler does not warn while those handlers are being implemented.
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct AuthenticatedUser {
    /// Internal platform user ID (UUID from the `users` table).
    pub id: Uuid,
    /// Clerk subject identifier (e.g. `user_2abc...`).
    pub clerk_id: String,
    /// User email address.
    pub email: String,
}

// ── JWT claims ────────────────────────────────────────────────────────────────

/// Claims extracted from a Clerk JWT.
///
/// `exp`, `nbf`, and `iss` are validated automatically by [`jsonwebtoken`] via
/// the [`Validation`] configuration; they do not need to appear here.
/// `email` requires a Clerk session claims template that includes the field.
#[derive(Debug, Deserialize)]
struct ClerkClaims {
    /// Clerk user ID (the `sub` claim).
    sub: String,
    /// User email address (requires the Clerk session claims template to
    /// include the `email` field).
    email: String,
}

// ── Helper functions ──────────────────────────────────────────────────────────

/// Extracts the raw token from an `Authorization: Bearer <token>` header.
pub(crate) fn extract_bearer_token(req: &Request) -> Option<&str> {
    req.headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
}

fn emit_auth_failure(state: &AppState, user_id: Option<Uuid>, reason: &str) {
    state.audit_logger.log(AuditEvent {
        action: AuditAction::AuthFailure,
        user_id,
        server_id: None,
        success: false,
        error_msg: Some(SanitizedErrorMsg::new(reason)),
        metadata: None,
        correlation_id: None,
    });
}

/// Upserts the user record in the database, returning `(id, email)`.
///
/// Uses an `ON CONFLICT (clerk_id) DO UPDATE` so that email changes from
/// Clerk propagate to the local table on the next login.
async fn upsert_user(
    pool: &sqlx::PgPool,
    clerk_id: &str,
    email: &str,
) -> Result<(Uuid, String), sqlx::Error> {
    let row = sqlx::query(
        r#"INSERT INTO users (clerk_id, email)
           VALUES ($1, $2)
           ON CONFLICT (clerk_id)
           DO UPDATE SET email = EXCLUDED.email, updated_at = NOW()
           RETURNING id, email"#,
    )
    .bind(clerk_id)
    .bind(email)
    .fetch_one(pool)
    .await?;

    Ok((row.get("id"), row.get("email")))
}

// ── Middleware ────────────────────────────────────────────────────────────────

/// Clerk JWT authentication middleware.
///
/// Applied via [`axum::middleware::from_fn_with_state`] to the `/v1/*` router
/// (excluding `/v1/webhooks/clerk` which is handled in TASK-017).
///
/// On success:
/// - Upserts the user in `users` via the email in JWT claims.
/// - Injects [`AuthenticatedUser`] into request extensions.
/// - Records `clerk_id` and `user_id` on the active tracing span.
/// - Emits an [`AuditAction::AuthSuccess`] event.
///
/// On failure:
/// - Returns `401 UNAUTHORIZED` or `401 TOKEN_EXPIRED`.
/// - Logs the failure at `warn` level with `request_id` (never the token).
/// - Emits an [`AuditAction::AuthFailure`] event.
pub async fn clerk_jwt_middleware(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    let request_id = req
        .extensions()
        .get::<RequestId>()
        .map(|r| r.0.clone())
        .unwrap_or_default();

    // ── 1. Extract Bearer token ───────────────────────────────────────────
    let token = match extract_bearer_token(&req) {
        Some(t) => t.to_owned(),
        None => {
            tracing::warn!(request_id = %request_id, "auth: missing Authorization header");
            emit_auth_failure(&state, None, "missing Authorization header");
            return AppError::Unauthorized(
                "Missing or invalid Authorization header.".to_string(),
            )
            .into_response();
        }
    };

    // ── 2. Decode JWT header to get key ID ────────────────────────────────
    let header = match decode_header(&token) {
        Ok(h) => h,
        Err(_) => {
            tracing::warn!(request_id = %request_id, "auth: malformed JWT header");
            emit_auth_failure(&state, None, "malformed JWT header");
            return AppError::Unauthorized("Invalid token.".to_string()).into_response();
        }
    };

    let kid = match header.kid {
        Some(k) => k,
        None => {
            tracing::warn!(request_id = %request_id, "auth: JWT missing kid");
            emit_auth_failure(&state, None, "JWT missing kid header");
            return AppError::Unauthorized("Invalid token.".to_string()).into_response();
        }
    };

    // ── 3. Fetch matching public key from JWKS cache ───────────────────────
    let decoding_key = match state.jwks_cache.get_key(&kid).await {
        Ok(k) => k,
        Err(e) => {
            tracing::warn!(request_id = %request_id, error = %e, "auth: key fetch failed");
            emit_auth_failure(&state, None, "JWKS key fetch failed");
            return AppError::Unauthorized("Invalid token.".to_string()).into_response();
        }
    };

    // ── 4. Validate signature, exp, nbf, iss ──────────────────────────────
    let mut validation = Validation::new(Algorithm::RS256);
    validation.validate_nbf = true;
    if !state.config.clerk_issuer.is_empty() {
        validation.set_issuer(&[state.config.clerk_issuer.as_str()]);
    }

    let claims = match decode::<ClerkClaims>(&token, &decoding_key, &validation) {
        Ok(data) => data.claims,
        Err(e) => {
            use jsonwebtoken::errors::ErrorKind;
            match e.kind() {
                ErrorKind::ExpiredSignature => {
                    tracing::warn!(request_id = %request_id, "auth: expired JWT");
                    emit_auth_failure(&state, None, "expired JWT");
                    return AppError::TokenExpired.into_response();
                }
                _ => {
                    tracing::warn!(request_id = %request_id, error = %e, "auth: invalid JWT");
                    emit_auth_failure(&state, None, "invalid JWT signature or claims");
                    return AppError::Unauthorized("Invalid token.".to_string())
                        .into_response();
                }
            }
        }
    };

    // ── 5. Upsert user in the database ────────────────────────────────────
    let (user_id, user_email) =
        match upsert_user(&state.pool, &claims.sub, &claims.email).await {
            Ok(u) => u,
            Err(e) => {
                tracing::error!(request_id = %request_id, error = %e, "auth: user upsert failed");
                return AppError::InternalServerError(
                    "Failed to update user record.".to_string(),
                )
                .into_response();
            }
        };

    // ── 6. Record identifiers on the active tracing span ─────────────────
    tracing::Span::current()
        .record("clerk_id", claims.sub.as_str())
        .record("user_id", user_id.to_string().as_str());

    // ── 7. Emit AuthSuccess audit event ───────────────────────────────────
    state.audit_logger.log(AuditEvent {
        action: AuditAction::AuthSuccess,
        user_id: Some(user_id),
        server_id: None,
        success: true,
        error_msg: None,
        metadata: None,
        correlation_id: None,
    });

    // ── 8. Inject authenticated user into request extensions ──────────────
    req.extensions_mut().insert(AuthenticatedUser {
        id: user_id,
        clerk_id: claims.sub,
        email: user_email,
    });

    next.run(req).await
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};

    #[test]
    fn extract_bearer_strips_prefix() {
        let req = Request::builder()
            .header("Authorization", "Bearer my-token")
            .body(Body::empty())
            .unwrap();
        assert_eq!(extract_bearer_token(&req), Some("my-token"));
    }

    #[test]
    fn extract_bearer_missing_header_returns_none() {
        let req = Request::builder().body(Body::empty()).unwrap();
        assert_eq!(extract_bearer_token(&req), None);
    }

    #[test]
    fn extract_bearer_wrong_scheme_returns_none() {
        let req = Request::builder()
            .header("Authorization", "Basic dXNlcjpwYXNz")
            .body(Body::empty())
            .unwrap();
        assert_eq!(extract_bearer_token(&req), None);
    }

    #[test]
    fn extract_bearer_bearer_only_no_token_returns_empty_str() {
        // "Bearer " with nothing after — strip_prefix succeeds but returns ""
        let req = Request::builder()
            .header("Authorization", "Bearer ")
            .body(Body::empty())
            .unwrap();
        // strip_prefix("Bearer ") on "Bearer " → Some("") — not None
        assert_eq!(extract_bearer_token(&req), Some(""));
    }
}
