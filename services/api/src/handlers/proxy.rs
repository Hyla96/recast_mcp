//! Proxy test endpoint — `POST /v1/proxy/test`.
//!
//! Accepts a URL template, HTTP method, path/query params, auth config, and
//! optional request body. Validates the fully-constructed URL against SSRF rules,
//! dispatches the upstream request via a shared `reqwest::Client`, and returns
//! the outcome as JSON.
//!
//! # Response shapes (all returned as HTTP 200)
//!
//! **Success:**
//! ```json
//! { "status": 200, "headers": {...}, "body": {...} }
//! ```
//! or (for non-JSON upstream responses):
//! ```json
//! { "status": 200, "headers": {...}, "body_raw": "..." }
//! ```
//! If the response exceeds 100 KB, the body is truncated and the response
//! includes `X-Recast-Truncated: true`.
//!
//! **Timeout:** `{ "outcome": "timeout" }`
//!
//! **Connectivity error:** `{ "outcome": "connectivity_error", "host": "..." }`

use axum::{
    extract::State,
    http::{HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Extension, Json,
};
use mcp_common::{AppError, AuditAction, AuditEvent};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::{app_state::AppState, auth::AuthenticatedUser};

// ── Constants ─────────────────────────────────────────────────────────────────

/// Maximum size of the caller-supplied request body (template) in bytes.
const MAX_REQUEST_BODY_BYTES: usize = 100 * 1024;

/// Maximum size of the upstream response body returned to the caller.
/// Responses exceeding this limit are truncated at this boundary.
const MAX_RESPONSE_BODY_BYTES: usize = 100 * 1024;

/// `User-Agent` header sent with every proxy test request.
const PROXY_USER_AGENT: &str = "recast-mcp-proxy/1.0";

/// Response header set to `"true"` when the upstream body was truncated.
const HEADER_RECAST_TRUNCATED: &str = "x-recast-truncated";

/// Response headers from the upstream that are stripped before forwarding.
/// Uses a blocklist approach — everything else is forwarded.
const SENSITIVE_RESPONSE_HEADERS: &[&str] = &[
    "authorization",
    "cookie",
    "set-cookie",
    "www-authenticate",
    "proxy-authorization",
    "proxy-authenticate",
];

// ── Request types ─────────────────────────────────────────────────────────────

/// Supported HTTP methods for proxy test calls.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyHttpMethod {
    /// HTTP GET.
    #[serde(rename = "GET")]
    Get,
    /// HTTP POST.
    #[serde(rename = "POST")]
    Post,
    /// HTTP PUT.
    #[serde(rename = "PUT")]
    Put,
    /// HTTP DELETE.
    #[serde(rename = "DELETE")]
    Delete,
    /// HTTP PATCH.
    #[serde(rename = "PATCH")]
    Patch,
}

impl ProxyHttpMethod {
    fn as_reqwest_method(self) -> reqwest::Method {
        match self {
            Self::Get => reqwest::Method::GET,
            Self::Post => reqwest::Method::POST,
            Self::Put => reqwest::Method::PUT,
            Self::Delete => reqwest::Method::DELETE,
            Self::Patch => reqwest::Method::PATCH,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Get => "get",
            Self::Post => "post",
            Self::Put => "put",
            Self::Delete => "delete",
            Self::Patch => "patch",
        }
    }
}

/// Placement of an API key credential in the proxied request.
#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApiKeyPlacement {
    /// API key sent as a request header.
    Header,
    /// API key appended as a query string parameter.
    Query,
}

/// Auth type discriminator for `AuthConfig`.
#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthType {
    /// No authentication.
    None,
    /// HTTP Bearer token (`Authorization: Bearer <token>`).
    Bearer,
    /// API key in a header or query parameter.
    ApiKey,
    /// HTTP Basic authentication.
    Basic,
}

/// Authentication configuration for the proxied request.
///
/// Uses a flat struct with `deny_unknown_fields` so that unexpected keys in the
/// caller's JSON return HTTP 422 automatically. Required fields for each auth
/// type are validated explicitly in the handler.
#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct AuthConfig {
    /// Auth type selector.
    #[serde(rename = "type")]
    pub auth_type: AuthType,
    /// Bearer token value. Required when `auth_type = "bearer"`.
    pub token: Option<String>,
    /// Where to place the API key. Required when `auth_type = "api_key"`.
    pub placement: Option<ApiKeyPlacement>,
    /// API key header/param name. Required when `auth_type = "api_key"`.
    pub key_name: Option<String>,
    /// API key value. Required when `auth_type = "api_key"`.
    pub key_value: Option<String>,
    /// HTTP Basic username. Required when `auth_type = "basic"`.
    pub username: Option<String>,
    /// HTTP Basic password. Required when `auth_type = "basic"`.
    pub password: Option<String>,
}

/// Request body for `POST /v1/proxy/test`.
#[derive(Deserialize, Debug)]
pub struct ProxyTestRequest {
    /// URL template — may contain `{param}` placeholders for path parameters.
    pub url: String,
    /// HTTP method to use for the upstream request.
    pub method: ProxyHttpMethod,
    /// Values for `{param}` placeholders in `url`.
    #[serde(default)]
    pub path_params: HashMap<String, String>,
    /// Query parameters appended to the URL (after SSRF validation).
    #[serde(default)]
    pub query_params: HashMap<String, String>,
    /// Authentication credentials.
    pub auth: AuthConfig,
    /// Optional request body for POST/PUT/PATCH methods. Capped at 100 KB.
    pub body: Option<String>,
}

// ── Response types ────────────────────────────────────────────────────────────

/// JSON body returned when the upstream responded (any HTTP status).
#[derive(Serialize)]
struct ProxySuccessBody {
    /// HTTP status code from the upstream response.
    status: u16,
    /// Safe subset of upstream response headers (sensitive headers removed).
    headers: HashMap<String, String>,
    /// Parsed upstream body when `Content-Type: application/json`.
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<serde_json::Value>,
    /// Raw upstream body (UTF-8, possibly lossy) for non-JSON responses.
    #[serde(skip_serializing_if = "Option::is_none")]
    body_raw: Option<String>,
}

// Internal discriminated union — never serialized directly.
enum ProxyOutcome {
    Success {
        status: u16,
        headers: HashMap<String, String>,
        body: Option<serde_json::Value>,
        body_raw: Option<String>,
        truncated: bool,
    },
    Timeout,
    ConnectivityError {
        host: String,
    },
}

// ── RedactedAuth ──────────────────────────────────────────────────────────────

/// Wrapper that prevents credential values from appearing in tracing spans.
///
/// `Display` shows auth type + non-sensitive fields only.
struct RedactedAuth<'a>(&'a AuthConfig);

impl std::fmt::Display for RedactedAuth<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0.auth_type {
            AuthType::None => write!(f, "none"),
            AuthType::Bearer => write!(f, "bearer([REDACTED])"),
            AuthType::ApiKey => write!(
                f,
                "api_key(placement={:?}, name={}, value=[REDACTED])",
                self.0.placement,
                self.0.key_name.as_deref().unwrap_or("")
            ),
            AuthType::Basic => write!(
                f,
                "basic(username={}, password=[REDACTED])",
                self.0.username.as_deref().unwrap_or("")
            ),
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Validates that all required auth fields are present for `auth.auth_type`.
fn validate_auth_fields(auth: &AuthConfig) -> Result<(), AppError> {
    match auth.auth_type {
        AuthType::None => Ok(()),
        AuthType::Bearer => {
            if auth.token.is_none() {
                return Err(AppError::Validation {
                    field: "auth.token".to_string(),
                    message: "required for bearer auth".to_string(),
                });
            }
            Ok(())
        }
        AuthType::ApiKey => {
            if auth.placement.is_none() {
                return Err(AppError::Validation {
                    field: "auth.placement".to_string(),
                    message: "required for api_key auth".to_string(),
                });
            }
            if auth.key_name.is_none() {
                return Err(AppError::Validation {
                    field: "auth.key_name".to_string(),
                    message: "required for api_key auth".to_string(),
                });
            }
            if auth.key_value.is_none() {
                return Err(AppError::Validation {
                    field: "auth.key_value".to_string(),
                    message: "required for api_key auth".to_string(),
                });
            }
            Ok(())
        }
        AuthType::Basic => {
            if auth.username.is_none() {
                return Err(AppError::Validation {
                    field: "auth.username".to_string(),
                    message: "required for basic auth".to_string(),
                });
            }
            if auth.password.is_none() {
                return Err(AppError::Validation {
                    field: "auth.password".to_string(),
                    message: "required for basic auth".to_string(),
                });
            }
            Ok(())
        }
    }
}

/// Percent-encodes a path parameter value using the RFC 3986 unreserved
/// character set: `ALPHA / DIGIT / "-" / "." / "_" / "~"`.
///
/// All other bytes are encoded as `%XX`. Non-ASCII UTF-8 codepoints are
/// encoded byte-by-byte, so the output is always valid ASCII.
fn percent_encode_path_param(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            let _ = std::fmt::Write::write_fmt(
                &mut encoded,
                format_args!("%{byte:02X}"),
            );
        }
    }
    encoded
}

/// Extracts upstream response headers, omitting known sensitive ones.
fn extract_safe_headers(headers: &reqwest::header::HeaderMap) -> HashMap<String, String> {
    headers
        .iter()
        .filter(|(name, _)| {
            !SENSITIVE_RESPONSE_HEADERS.contains(&name.as_str())
        })
        .filter_map(|(name, value)| {
            value.to_str().ok().map(|v| (name.to_string(), v.to_string()))
        })
        .collect()
}

/// Returns the audit-log label for an auth config.
fn auth_type_label(auth: &AuthConfig) -> &'static str {
    match auth.auth_type {
        AuthType::None => "none",
        AuthType::Bearer => "bearer",
        AuthType::ApiKey => match &auth.placement {
            Some(ApiKeyPlacement::Header) => "api_key_header",
            Some(ApiKeyPlacement::Query) | None => "api_key_query",
        },
        AuthType::Basic => "basic",
    }
}

// ── Handler ───────────────────────────────────────────────────────────────────

/// `POST /v1/proxy/test`
///
/// Accepts a URL template + auth config from the builder frontend, constructs
/// the upstream URL, validates it against SSRF rules, dispatches the request
/// via a shared `reqwest::Client`, and returns the outcome as JSON.
///
/// All outcome shapes (success, timeout, connectivity_error) are returned as
/// HTTP 200 so the frontend can distinguish them in the `outcome` field without
/// inspecting the HTTP status code.
///
/// # Auth-type-specific behaviour
///
/// - `bearer`: adds `Authorization: Bearer <token>` header.
/// - `api_key` (header): adds `<key_name>: <key_value>` header.
/// - `api_key` (query): appends `?<key_name>=<key_value>` to the URL **after**
///   SSRF validation so the credential never leaks into the validated URL string.
/// - `basic`: uses `Authorization: Basic <base64(user:pass)>` header.
///
/// # Client disconnect
///
/// The upstream request is raced via `tokio::select!` against the
/// `proxy_timeout`. If the axum connection drops, the containing future is
/// cancelled, which automatically drops the in-flight `reqwest` request.
pub async fn proxy_test_handler(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Json(req): Json<ProxyTestRequest>,
) -> Result<Response, AppError> {
    let span = tracing::info_span!(
        "proxy_test",
        actor_id = %user.id,
        method = req.method.as_str(),
        url_host = tracing::field::Empty,
        auth_type = tracing::field::Empty,
    );
    let _enter = span.enter();

    // ── 1. Validate caller-supplied request body size ─────────────────────────
    if req.body.as_ref().map(|b| b.len()).unwrap_or(0) > MAX_REQUEST_BODY_BYTES {
        return Err(AppError::Validation {
            field: "body".to_string(),
            message: "must not exceed 100 KB".to_string(),
        });
    }

    // ── 2. Validate required auth fields ─────────────────────────────────────
    validate_auth_fields(&req.auth)?;

    // ── 3. Construct upstream URL ─────────────────────────────────────────────
    // Substitute path params first, then parse, then append query params.
    let mut url_str = req.url.clone();
    for (key, value) in &req.path_params {
        let placeholder = format!("{{{key}}}");
        url_str = url_str.replace(&placeholder, &percent_encode_path_param(value));
    }

    let mut parsed_url = url_str.parse::<url::Url>().map_err(|_| AppError::Validation {
        field: "url".to_string(),
        message: "invalid URL format".to_string(),
    })?;

    // Append caller-supplied query params (before SSRF check; the api_key+query
    // credential is added AFTER the check to prevent it from leaking into logs).
    for (key, value) in &req.query_params {
        parsed_url.query_pairs_mut().append_pair(key, value);
    }

    let url_host = parsed_url.host_str().unwrap_or("unknown").to_string();
    let auth_type = auth_type_label(&req.auth);
    span.record("url_host", url_host.as_str());
    span.record("auth_type", auth_type);

    tracing::debug!(
        url_host = %url_host,
        method = req.method.as_str(),
        auth = %RedactedAuth(&req.auth),
        "dispatching proxy test request"
    );

    // ── 4. SSRF validation — Phase 1 + async DNS ─────────────────────────────
    // The api_key+query credential is NOT yet in the URL here.
    (state.ssrf_validator)(parsed_url.clone()).await?;

    // ── 5. Append api_key+query credential AFTER SSRF validation ─────────────
    if req.auth.auth_type == AuthType::ApiKey
        && req.auth.placement == Some(ApiKeyPlacement::Query)
    {
        if let (Some(name), Some(value)) = (&req.auth.key_name, &req.auth.key_value) {
            parsed_url.query_pairs_mut().append_pair(name, value);
        }
    }

    // ── 6. Build reqwest request ──────────────────────────────────────────────
    let method = req.method.as_reqwest_method();
    let mut request_builder = state
        .http_client
        .request(method, parsed_url.as_str())
        .header(reqwest::header::USER_AGENT, PROXY_USER_AGENT);

    // Inject auth credentials — values must not appear in any log output.
    request_builder = match &req.auth.auth_type {
        AuthType::None => request_builder,
        AuthType::Bearer => {
            let token = req.auth.token.as_deref().unwrap_or("");
            request_builder
                .header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"))
        }
        AuthType::ApiKey => match req.auth.placement {
            Some(ApiKeyPlacement::Header) => {
                let name = req.auth.key_name.as_deref().unwrap_or("");
                let value = req.auth.key_value.as_deref().unwrap_or("");
                request_builder.header(name, value)
            }
            Some(ApiKeyPlacement::Query) | None => {
                // Already appended to URL above.
                request_builder
            }
        },
        AuthType::Basic => {
            let username = req.auth.username.as_deref().unwrap_or("");
            request_builder.basic_auth(username, req.auth.password.as_deref())
        }
    };

    // Attach optional body (POST/PUT/PATCH).
    if let Some(ref body_str) = req.body {
        request_builder = request_builder
            .body(body_str.clone())
            .header(reqwest::header::CONTENT_TYPE, "application/json");
    }

    let upstream_request = request_builder.build().map_err(|e| {
        AppError::InternalServerError(format!("failed to build upstream request: {e}"))
    })?;

    // ── 7. Dispatch via tokio::select! (upstream vs. timeout) ────────────────
    //
    // Races the upstream call against `proxy_timeout`. When the axum connection
    // drops, the enclosing future is cancelled, which automatically cancels the
    // in-flight reqwest request (Rust async Future cancellation).
    let outcome = tokio::select! {
        result = state.http_client.execute(upstream_request) => {
            match result {
                Ok(response) => {
                    let status = response.status().as_u16();
                    let safe_headers = extract_safe_headers(response.headers());
                    let is_json = response
                        .headers()
                        .get(reqwest::header::CONTENT_TYPE)
                        .and_then(|v| v.to_str().ok())
                        .map(|ct| ct.contains("application/json"))
                        .unwrap_or(false);

                    let raw_bytes = response.bytes().await.map_err(|e| {
                        AppError::InternalServerError(format!(
                            "failed to read upstream response body: {e}"
                        ))
                    })?;

                    let truncated = raw_bytes.len() > MAX_RESPONSE_BODY_BYTES;
                    let body_slice = &raw_bytes[..raw_bytes.len().min(MAX_RESPONSE_BODY_BYTES)];

                    let (body, body_raw) = if is_json {
                        let json_val: Option<serde_json::Value> =
                            serde_json::from_slice(body_slice).ok();
                        (json_val, None)
                    } else {
                        let raw_str = String::from_utf8_lossy(body_slice).into_owned();
                        (None, Some(raw_str))
                    };

                    ProxyOutcome::Success {
                        status,
                        headers: safe_headers,
                        body,
                        body_raw,
                        truncated,
                    }
                }
                Err(e) if e.is_connect() => {
                    ProxyOutcome::ConnectivityError { host: url_host.clone() }
                }
                Err(e) if e.is_timeout() => ProxyOutcome::Timeout,
                Err(e) => {
                    // Treat other request errors (e.g., TLS, redirect loops) as
                    // connectivity failures so the caller can "use sample response".
                    tracing::warn!(
                        url_host = %url_host,
                        error = %e,
                        "proxy test request failed"
                    );
                    ProxyOutcome::ConnectivityError { host: url_host.clone() }
                }
            }
        }
        _ = tokio::time::sleep(state.proxy_timeout) => {
            ProxyOutcome::Timeout
        }
    };

    // ── 8. Emit audit log ─────────────────────────────────────────────────────
    let outcome_status: Option<u16> = match &outcome {
        ProxyOutcome::Success { status, .. } => Some(*status),
        ProxyOutcome::Timeout | ProxyOutcome::ConnectivityError { .. } => None,
    };

    state.audit_logger.log(AuditEvent {
        action: AuditAction::ProxyTest,
        user_id: Some(user.id),
        server_id: None,
        success: true,
        error_msg: None,
        metadata: Some(serde_json::json!({
            "url_host": url_host,
            "method": req.method.as_str(),
            "auth_type": auth_type,
            "status": outcome_status,
        })),
        correlation_id: None,
    });

    // ── 9. Build HTTP response ────────────────────────────────────────────────
    let response = match outcome {
        ProxyOutcome::Timeout => (
            StatusCode::OK,
            Json(serde_json::json!({ "outcome": "timeout" })),
        )
            .into_response(),

        ProxyOutcome::ConnectivityError { ref host } => (
            StatusCode::OK,
            Json(serde_json::json!({
                "outcome": "connectivity_error",
                "host": host,
            })),
        )
            .into_response(),

        ProxyOutcome::Success { status, headers, body, body_raw, truncated } => {
            let success_body = ProxySuccessBody { status, headers, body, body_raw };
            let mut response = (StatusCode::OK, Json(success_body)).into_response();
            if truncated {
                response.headers_mut().insert(
                    HeaderName::from_static(HEADER_RECAST_TRUNCATED),
                    HeaderValue::from_static("true"),
                );
            }
            response
        }
    };

    Ok(response)
}
