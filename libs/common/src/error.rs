//! Application error types and JSON-RPC error mapping.

use axum::{
    body::Body,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use mcp_protocol::error_codes;
use serde::Serialize;
use thiserror::Error;

// ── SanitizedErrorMsg ────────────────────────────────────────────────────────

/// A newtype wrapper for error messages that are safe to store in the audit log.
///
/// Using a dedicated newtype enforces at compile time that audit log entries
/// only receive pre-approved, sanitised strings — never raw SQL errors, stack
/// traces, or other internal details.
///
/// Construct via [`SanitizedErrorMsg::new`].
#[derive(Debug, Clone, Serialize)]
pub struct SanitizedErrorMsg(String);

impl SanitizedErrorMsg {
    /// Create a new sanitized error message from any string-like value.
    pub fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }

    /// Returns the inner string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SanitizedErrorMsg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

// ── McpError ─────────────────────────────────────────────────────────────────

/// An application error mapped to a JSON-RPC 2.0 error object.
///
/// This type lives in `mcp-common` alongside [`AppError`] because Rust's orphan
/// rule requires that for `impl From<AppError> for McpError` at least one of the
/// trait or the Self type is local to the implementing crate. Since `From` is
/// from `std` (foreign), `McpError` must be local here.
///
/// Error code constants are defined in [`mcp_protocol::error_codes`].
#[derive(Debug, Clone, Serialize)]
pub struct McpError {
    /// The JSON-RPC error code.
    pub code: i32,
    /// The human-readable error message (never includes internal details).
    pub message: String,
    /// Optional additional data attached to the error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl McpError {
    /// Creates a new `McpError` with no data payload.
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }
}

impl From<AppError> for McpError {
    fn from(err: AppError) -> Self {
        match err {
            // Method / resource not found
            AppError::ToolNotFound => {
                McpError::new(error_codes::METHOD_NOT_FOUND, "Tool not found.")
            }
            AppError::CredentialNotFound => {
                McpError::new(error_codes::METHOD_NOT_FOUND, "Credential not found.")
            }
            AppError::NotFound(_) => {
                McpError::new(error_codes::METHOD_NOT_FOUND, "Resource not found.")
            }

            // Invalid input / parameters
            AppError::Validation { field, message } => McpError::new(
                error_codes::INVALID_PARAMS,
                format!("Validation error for '{field}': {message}"),
            ),
            AppError::BadRequest(_) => {
                McpError::new(error_codes::INVALID_PARAMS, "Invalid request parameters.")
            }

            // Internal / upstream errors — never expose internal detail to the caller
            AppError::InternalServerError(_)
            | AppError::UpstreamError { .. }
            | AppError::UpstreamTimeout
            | AppError::UpstreamUnreachable { .. }
            | AppError::SsrfBlocked { .. } => {
                McpError::new(error_codes::INTERNAL_ERROR, "An internal error occurred.")
            }

            // Auth errors — server-defined error range (-32000)
            AppError::Unauthorized(_) | AppError::TokenExpired => {
                McpError::new(error_codes::SERVER_ERROR_BASE, "Unauthorized.")
            }
            AppError::Forbidden(_) => {
                McpError::new(error_codes::SERVER_ERROR_BASE, "Forbidden.")
            }

            // Rate limiting
            AppError::RateLimited { retry_after_secs } => McpError::new(
                error_codes::SERVER_ERROR_BASE,
                format!("Rate limit exceeded. Retry after {retry_after_secs}s."),
            ),

            // Conflict
            AppError::Conflict(_) => {
                McpError::new(error_codes::SERVER_ERROR_BASE, "Conflict.")
            }
        }
    }
}

// ── AppError ─────────────────────────────────────────────────────────────────

/// Application-level errors that map to standard HTTP status codes.
#[derive(Debug, Error)]
pub enum AppError {
    // ── Existing variants ────────────────────────────────────────────────────

    /// Resource not found.
    #[error("not found: {0}")]
    NotFound(String),

    /// Unauthorized request.
    #[error("unauthorized: {0}")]
    Unauthorized(String),

    /// Forbidden request.
    #[error("forbidden: {0}")]
    Forbidden(String),

    /// Bad request.
    #[error("bad request: {0}")]
    BadRequest(String),

    /// Internal server error.
    ///
    /// **Security note**: the inner string is logged at `error` level with the
    /// `request_id` but is NEVER included in the HTTP response body. The
    /// response always carries a generic "An internal error occurred." message.
    #[error("internal server error: {0}")]
    InternalServerError(String),

    /// Conflict.
    #[error("conflict: {0}")]
    Conflict(String),

    // ── New variants ─────────────────────────────────────────────────────────

    /// The caller's token has expired.
    #[error("token has expired")]
    TokenExpired,

    /// An outgoing request was blocked by SSRF protection.
    #[error("ssrf blocked: {reason} (url: {url})")]
    SsrfBlocked {
        /// The URL that was blocked.
        url: String,
        /// The reason the URL was blocked (e.g. "private IP range").
        reason: String,
    },

    /// The caller has exceeded their rate limit.
    #[error("rate limited: retry after {retry_after_secs} seconds")]
    RateLimited {
        /// Seconds to wait before retrying.
        retry_after_secs: u64,
    },

    /// The upstream API returned an error HTTP status.
    #[error("upstream error {status}")]
    UpstreamError {
        /// The HTTP status code returned by the upstream.
        status: u16,
        /// The response body from the upstream (truncated if large).
        body: String,
    },

    /// The upstream API did not respond within the configured timeout.
    #[error("upstream request timed out")]
    UpstreamTimeout,

    /// The upstream host could not be reached (DNS failure, connection refused, etc.).
    #[error("upstream unreachable: {reason}")]
    UpstreamUnreachable {
        /// Reason the upstream could not be reached.
        reason: String,
    },

    /// A referenced credential does not exist (or has been deleted).
    #[error("credential not found")]
    CredentialNotFound,

    /// A referenced MCP tool does not exist on the configured upstream.
    #[error("tool not found")]
    ToolNotFound,

    /// Input validation failed for a specific field.
    #[error("validation error for '{field}': {message}")]
    Validation {
        /// The field that failed validation.
        field: String,
        /// Human-readable description of the failure.
        message: String,
    },
}

impl AppError {
    /// Returns the HTTP status code for this error variant.
    pub fn status_code(&self) -> u16 {
        match self {
            AppError::NotFound(_) | AppError::CredentialNotFound | AppError::ToolNotFound => 404,
            AppError::Unauthorized(_) | AppError::TokenExpired => 401,
            AppError::Forbidden(_) => 403,
            AppError::BadRequest(_) | AppError::Validation { .. } => 400,
            AppError::InternalServerError(_) => 500,
            AppError::Conflict(_) => 409,
            AppError::SsrfBlocked { .. } => 422,
            AppError::RateLimited { .. } => 429,
            AppError::UpstreamError { .. } | AppError::UpstreamUnreachable { .. } => 502,
            AppError::UpstreamTimeout => 504,
        }
    }

    /// Returns the stable snake_case error code string for this variant.
    ///
    /// These codes are included in every error response body and are stable
    /// across versions — clients may use them for programmatic error handling.
    pub fn code(&self) -> &'static str {
        match self {
            AppError::NotFound(_) => "not_found",
            AppError::Unauthorized(_) => "unauthorized",
            AppError::Forbidden(_) => "forbidden",
            AppError::BadRequest(_) => "bad_request",
            AppError::InternalServerError(_) => "internal_server_error",
            AppError::Conflict(_) => "conflict",
            AppError::TokenExpired => "token_expired",
            AppError::SsrfBlocked { .. } => "ssrf_blocked",
            AppError::RateLimited { .. } => "rate_limited",
            AppError::UpstreamError { .. } => "upstream_error",
            AppError::UpstreamTimeout => "upstream_timeout",
            AppError::UpstreamUnreachable { .. } => "upstream_unreachable",
            AppError::CredentialNotFound => "credential_not_found",
            AppError::ToolNotFound => "tool_not_found",
            AppError::Validation { .. } => "validation_error",
        }
    }

    /// Returns the **public** message included in the HTTP response body.
    ///
    /// For [`AppError::InternalServerError`] this is always a generic string —
    /// the actual detail is logged at `error` level by [`IntoResponse`] with the
    /// `request_id` attached so the full error can be correlated in logs.
    pub fn public_message(&self) -> String {
        match self {
            AppError::InternalServerError(_) => "An internal error occurred.".to_string(),
            other => other.to_string(),
        }
    }
}

/// Converts `AppError` into an axum HTTP response.
///
/// Produces the standard JSON error body shape:
/// `{"error": {"code": "...", "message": "...", "request_id": "..."}}`.
///
/// A fresh ULID is generated as the `request_id` and written to both the
/// response body and the `X-Request-ID` response header so the two always
/// match.
///
/// For [`AppError::InternalServerError`], the response body carries a generic
/// "An internal error occurred." message. The real error detail is logged at
/// `error` level with the `request_id` so it can be correlated in the log
/// aggregator without exposing internal state to callers.
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let request_id = ulid::Ulid::new().to_string();
        let status = StatusCode::from_u16(self.status_code())
            .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let code = self.code().to_string();

        // Log internal errors with full detail before stripping them from the body.
        if let AppError::InternalServerError(ref detail) = self {
            tracing::error!(
                request_id = %request_id,
                error = %detail,
                "internal server error"
            );
        }

        let message = self.public_message();

        let body = ErrorResponse {
            error: ErrorDetails {
                code: code.clone(),
                message,
                request_id: request_id.clone(),
            },
        };

        let json_body = serde_json::to_string(&body).unwrap_or_else(|_| {
            format!(
                r#"{{"error":{{"code":"{code}","message":"error serialization failed","request_id":"{request_id}"}}}}"#
            )
        });

        let header_value = request_id
            .parse::<header::HeaderValue>()
            .unwrap_or_else(|_| header::HeaderValue::from_static("error"));

        Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, "application/json")
            .header("x-request-id", header_value)
            .body(Body::from(json_body))
            .unwrap_or_else(|_| Response::new(Body::empty()))
    }
}

/// Standard API error response body.
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    /// The error details.
    pub error: ErrorDetails,
}

/// Error details nested inside [`ErrorResponse`].
#[derive(Debug, Serialize)]
pub struct ErrorDetails {
    /// Stable snake_case error code (e.g. `"not_found"`).
    pub code: String,
    /// Human-readable description that never exposes internals.
    pub message: String,
    /// Per-request ULID, also echoed in the `X-Request-ID` response header.
    pub request_id: String,
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(unsafe_code)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    // ── Status code and code string ──────────────────────────────────────────

    #[test]
    fn test_not_found_status_and_code() {
        let err = AppError::NotFound("user".to_string());
        assert_eq!(err.status_code(), 404);
        assert_eq!(err.code(), "not_found");
    }

    #[test]
    fn test_unauthorized_status_and_code() {
        let err = AppError::Unauthorized("invalid token".to_string());
        assert_eq!(err.status_code(), 401);
        assert_eq!(err.code(), "unauthorized");
    }

    #[test]
    fn test_forbidden_status_and_code() {
        let err = AppError::Forbidden("access denied".to_string());
        assert_eq!(err.status_code(), 403);
        assert_eq!(err.code(), "forbidden");
    }

    #[test]
    fn test_bad_request_status_and_code() {
        let err = AppError::BadRequest("missing field".to_string());
        assert_eq!(err.status_code(), 400);
        assert_eq!(err.code(), "bad_request");
    }

    #[test]
    fn test_internal_server_error_status_and_code() {
        let err = AppError::InternalServerError("database unavailable".to_string());
        assert_eq!(err.status_code(), 500);
        assert_eq!(err.code(), "internal_server_error");
    }

    #[test]
    fn test_conflict_status_and_code() {
        let err = AppError::Conflict("duplicate key".to_string());
        assert_eq!(err.status_code(), 409);
        assert_eq!(err.code(), "conflict");
    }

    #[test]
    fn test_token_expired_status_and_code() {
        let err = AppError::TokenExpired;
        assert_eq!(err.status_code(), 401);
        assert_eq!(err.code(), "token_expired");
    }

    #[test]
    fn test_ssrf_blocked_status_and_code() {
        let err = AppError::SsrfBlocked {
            url: "http://192.168.1.1".to_string(),
            reason: "private IP range".to_string(),
        };
        assert_eq!(err.status_code(), 422);
        assert_eq!(err.code(), "ssrf_blocked");
    }

    #[test]
    fn test_rate_limited_status_and_code() {
        let err = AppError::RateLimited { retry_after_secs: 30 };
        assert_eq!(err.status_code(), 429);
        assert_eq!(err.code(), "rate_limited");
    }

    #[test]
    fn test_upstream_error_status_and_code() {
        let err = AppError::UpstreamError {
            status: 500,
            body: "Internal Server Error".to_string(),
        };
        assert_eq!(err.status_code(), 502);
        assert_eq!(err.code(), "upstream_error");
    }

    #[test]
    fn test_upstream_timeout_status_and_code() {
        let err = AppError::UpstreamTimeout;
        assert_eq!(err.status_code(), 504);
        assert_eq!(err.code(), "upstream_timeout");
    }

    #[test]
    fn test_upstream_unreachable_status_and_code() {
        let err = AppError::UpstreamUnreachable {
            reason: "connection refused".to_string(),
        };
        assert_eq!(err.status_code(), 502);
        assert_eq!(err.code(), "upstream_unreachable");
    }

    #[test]
    fn test_credential_not_found_status_and_code() {
        let err = AppError::CredentialNotFound;
        assert_eq!(err.status_code(), 404);
        assert_eq!(err.code(), "credential_not_found");
    }

    #[test]
    fn test_tool_not_found_status_and_code() {
        let err = AppError::ToolNotFound;
        assert_eq!(err.status_code(), 404);
        assert_eq!(err.code(), "tool_not_found");
    }

    #[test]
    fn test_validation_status_and_code() {
        let err = AppError::Validation {
            field: "email".to_string(),
            message: "must be a valid email address".to_string(),
        };
        assert_eq!(err.status_code(), 400);
        assert_eq!(err.code(), "validation_error");
    }

    // ── IntoResponse: HTTP status and JSON body shape ─────────────────────────

    #[tokio::test]
    async fn test_not_found_into_response_http_404() {
        let response = AppError::NotFound("x".to_string()).into_response();

        assert_eq!(response.status().as_u16(), 404);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).expect("content-type missing"),
            "application/json"
        );

        let request_id_header = response
            .headers()
            .get("x-request-id")
            .expect("x-request-id header missing")
            .to_str()
            .expect("header is not ascii")
            .to_string();

        let bytes =
            to_bytes(response.into_body(), usize::MAX).await.expect("failed to read body");
        let body: serde_json::Value =
            serde_json::from_slice(&bytes).expect("body is not valid JSON");

        assert_eq!(body["error"]["code"], "not_found");
        assert!(body["error"]["message"].as_str().unwrap_or("").contains("not found"));
        let body_request_id = body["error"]["request_id"].as_str().expect("request_id missing");
        assert!(!body_request_id.is_empty(), "request_id must be non-empty");
        assert_eq!(
            body_request_id, request_id_header,
            "X-Request-ID header and body request_id must match"
        );
    }

    #[tokio::test]
    async fn test_unauthorized_into_response_http_401() {
        let response = AppError::Unauthorized("bad token".to_string()).into_response();
        assert_eq!(response.status().as_u16(), 401);
        let bytes =
            to_bytes(response.into_body(), usize::MAX).await.expect("failed to read body");
        let body: serde_json::Value =
            serde_json::from_slice(&bytes).expect("body is not valid JSON");
        assert_eq!(body["error"]["code"], "unauthorized");
    }

    #[tokio::test]
    async fn test_error_response_body_shape() {
        let response = AppError::BadRequest("missing name".to_string()).into_response();
        let bytes =
            to_bytes(response.into_body(), usize::MAX).await.expect("failed to read body");
        let body: serde_json::Value =
            serde_json::from_slice(&bytes).expect("body is not valid JSON");

        assert!(body["error"].is_object(), "body must have an 'error' object");
        assert!(body["error"]["code"].is_string(), "'code' must be a string");
        assert!(body["error"]["message"].is_string(), "'message' must be a string");
        assert!(body["error"]["request_id"].is_string(), "'request_id' must be a string");
        assert_eq!(
            body.as_object().expect("body is not an object").len(),
            1,
            "body must have exactly one top-level key: 'error'"
        );
    }

    #[tokio::test]
    async fn test_request_id_is_ulid_format() {
        let response = AppError::NotFound("resource".to_string()).into_response();
        let request_id_header = response
            .headers()
            .get("x-request-id")
            .expect("missing x-request-id header")
            .to_str()
            .expect("non-ascii header")
            .to_string();
        assert_eq!(request_id_header.len(), 26, "ULID must be 26 characters");
    }

    #[test]
    fn test_error_response_serialization() {
        let response = ErrorResponse {
            error: ErrorDetails {
                code: "not_found".to_string(),
                message: "not found: user".to_string(),
                request_id: "01HZEXAMPLEULID00000000000".to_string(),
            },
        };
        let json = serde_json::to_string(&response).expect("failed to serialize");
        assert!(json.contains("\"code\":\"not_found\""));
        assert!(json.contains("\"request_id\":\"01HZEXAMPLEULID00000000000\""));
    }

    // ── New variant IntoResponse tests ────────────────────────────────────────

    #[tokio::test]
    async fn test_token_expired_into_response() {
        let response = AppError::TokenExpired.into_response();
        assert_eq!(response.status().as_u16(), 401);
        let bytes =
            to_bytes(response.into_body(), usize::MAX).await.expect("failed to read body");
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], "token_expired");
    }

    #[tokio::test]
    async fn test_ssrf_blocked_into_response() {
        let response = AppError::SsrfBlocked {
            url: "http://192.168.1.1".to_string(),
            reason: "private IP range".to_string(),
        }
        .into_response();
        assert_eq!(response.status().as_u16(), 422);
        let bytes =
            to_bytes(response.into_body(), usize::MAX).await.expect("failed to read body");
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], "ssrf_blocked");
    }

    #[tokio::test]
    async fn test_rate_limited_into_response() {
        let response = AppError::RateLimited { retry_after_secs: 10 }.into_response();
        assert_eq!(response.status().as_u16(), 429);
        let bytes =
            to_bytes(response.into_body(), usize::MAX).await.expect("failed to read body");
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], "rate_limited");
        assert!(
            body["error"]["message"].as_str().unwrap_or("").contains("10"),
            "rate_limited message should include retry_after_secs"
        );
    }

    #[tokio::test]
    async fn test_upstream_error_into_response() {
        let response = AppError::UpstreamError {
            status: 503,
            body: "service unavailable".to_string(),
        }
        .into_response();
        assert_eq!(response.status().as_u16(), 502);
        let bytes =
            to_bytes(response.into_body(), usize::MAX).await.expect("failed to read body");
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], "upstream_error");
    }

    #[tokio::test]
    async fn test_upstream_timeout_into_response() {
        let response = AppError::UpstreamTimeout.into_response();
        assert_eq!(response.status().as_u16(), 504);
        let bytes =
            to_bytes(response.into_body(), usize::MAX).await.expect("failed to read body");
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], "upstream_timeout");
    }

    #[tokio::test]
    async fn test_upstream_unreachable_into_response() {
        let response = AppError::UpstreamUnreachable {
            reason: "connection refused".to_string(),
        }
        .into_response();
        assert_eq!(response.status().as_u16(), 502);
        let bytes =
            to_bytes(response.into_body(), usize::MAX).await.expect("failed to read body");
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], "upstream_unreachable");
    }

    #[tokio::test]
    async fn test_credential_not_found_into_response() {
        let response = AppError::CredentialNotFound.into_response();
        assert_eq!(response.status().as_u16(), 404);
        let bytes =
            to_bytes(response.into_body(), usize::MAX).await.expect("failed to read body");
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], "credential_not_found");
    }

    #[tokio::test]
    async fn test_tool_not_found_into_response() {
        let response = AppError::ToolNotFound.into_response();
        assert_eq!(response.status().as_u16(), 404);
        let bytes =
            to_bytes(response.into_body(), usize::MAX).await.expect("failed to read body");
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], "tool_not_found");
    }

    #[tokio::test]
    async fn test_validation_into_response() {
        let response = AppError::Validation {
            field: "display_name".to_string(),
            message: "must not exceed 100 characters".to_string(),
        }
        .into_response();
        assert_eq!(response.status().as_u16(), 400);
        let bytes =
            to_bytes(response.into_body(), usize::MAX).await.expect("failed to read body");
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], "validation_error");
    }

    // ── InternalServerError must NOT leak internal detail ─────────────────────

    #[tokio::test]
    async fn test_internal_server_error_response_hides_detail() {
        let internal_detail = "pg: relation \"secrets\" does not exist";
        let response =
            AppError::InternalServerError(internal_detail.to_string()).into_response();
        assert_eq!(response.status().as_u16(), 500);
        let bytes =
            to_bytes(response.into_body(), usize::MAX).await.expect("failed to read body");
        let body_str = std::str::from_utf8(&bytes).expect("body is not UTF-8");

        // The internal SQL detail must NOT appear in the response body.
        assert!(
            !body_str.contains(internal_detail),
            "internal detail must not appear in the HTTP response body"
        );
        // But the code must still be present.
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], "internal_server_error");
        assert_eq!(body["error"]["message"], "An internal error occurred.");
    }

    // ── McpError conversions ──────────────────────────────────────────────────

    #[test]
    fn test_mcp_error_from_tool_not_found() {
        let err = McpError::from(AppError::ToolNotFound);
        assert_eq!(err.code, error_codes::METHOD_NOT_FOUND);
    }

    #[test]
    fn test_mcp_error_from_credential_not_found() {
        let err = McpError::from(AppError::CredentialNotFound);
        assert_eq!(err.code, error_codes::METHOD_NOT_FOUND);
    }

    #[test]
    fn test_mcp_error_from_not_found() {
        let err = McpError::from(AppError::NotFound("resource".to_string()));
        assert_eq!(err.code, error_codes::METHOD_NOT_FOUND);
    }

    #[test]
    fn test_mcp_error_from_validation() {
        let err = McpError::from(AppError::Validation {
            field: "name".to_string(),
            message: "required".to_string(),
        });
        assert_eq!(err.code, error_codes::INVALID_PARAMS);
        assert!(err.message.contains("name"));
    }

    #[test]
    fn test_mcp_error_from_bad_request() {
        let err = McpError::from(AppError::BadRequest("bad input".to_string()));
        assert_eq!(err.code, error_codes::INVALID_PARAMS);
    }

    #[test]
    fn test_mcp_error_from_internal_server_error_hides_detail() {
        let err =
            McpError::from(AppError::InternalServerError("DB exploded".to_string()));
        assert_eq!(err.code, error_codes::INTERNAL_ERROR);
        assert!(
            !err.message.contains("DB exploded"),
            "internal detail must not appear in McpError message"
        );
    }

    #[test]
    fn test_mcp_error_from_upstream_error() {
        let err =
            McpError::from(AppError::UpstreamError { status: 500, body: "oops".to_string() });
        assert_eq!(err.code, error_codes::INTERNAL_ERROR);
    }

    #[test]
    fn test_mcp_error_from_upstream_timeout() {
        let err = McpError::from(AppError::UpstreamTimeout);
        assert_eq!(err.code, error_codes::INTERNAL_ERROR);
    }

    #[test]
    fn test_mcp_error_from_upstream_unreachable() {
        let err = McpError::from(AppError::UpstreamUnreachable {
            reason: "refused".to_string(),
        });
        assert_eq!(err.code, error_codes::INTERNAL_ERROR);
    }

    #[test]
    fn test_mcp_error_from_ssrf_blocked() {
        let err = McpError::from(AppError::SsrfBlocked {
            url: "http://169.254.169.254".to_string(),
            reason: "cloud metadata endpoint".to_string(),
        });
        assert_eq!(err.code, error_codes::INTERNAL_ERROR);
        // SSRF details must not leak to the JSON-RPC caller
        assert!(
            !err.message.contains("169.254.169.254"),
            "SSRF target URL must not appear in McpError"
        );
    }

    #[test]
    fn test_mcp_error_from_unauthorized() {
        let err = McpError::from(AppError::Unauthorized("no token".to_string()));
        assert_eq!(err.code, error_codes::SERVER_ERROR_BASE);
    }

    #[test]
    fn test_mcp_error_from_token_expired() {
        let err = McpError::from(AppError::TokenExpired);
        assert_eq!(err.code, error_codes::SERVER_ERROR_BASE);
    }

    #[test]
    fn test_mcp_error_from_forbidden() {
        let err = McpError::from(AppError::Forbidden("no access".to_string()));
        assert_eq!(err.code, error_codes::SERVER_ERROR_BASE);
    }

    #[test]
    fn test_mcp_error_from_rate_limited() {
        let err = McpError::from(AppError::RateLimited { retry_after_secs: 60 });
        assert_eq!(err.code, error_codes::SERVER_ERROR_BASE);
        assert!(err.message.contains("60"));
    }

    #[test]
    fn test_mcp_error_from_conflict() {
        let err = McpError::from(AppError::Conflict("slug collision".to_string()));
        assert_eq!(err.code, error_codes::SERVER_ERROR_BASE);
    }

    // ── SanitizedErrorMsg ────────────────────────────────────────────────────

    #[test]
    fn test_sanitized_error_msg_display() {
        let msg = SanitizedErrorMsg::new("credential rotation failed");
        assert_eq!(msg.to_string(), "credential rotation failed");
        assert_eq!(msg.as_str(), "credential rotation failed");
    }

    #[test]
    fn test_sanitized_error_msg_serializes_as_string() {
        let msg = SanitizedErrorMsg::new("some error");
        let json = serde_json::to_string(&msg).expect("serialize failed");
        assert_eq!(json, r#""some error""#);
    }
}
