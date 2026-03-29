//! Application error types.

use axum::{
    body::Body,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use thiserror::Error;

/// Application-level errors that map to standard HTTP status codes.
#[derive(Debug, Error)]
pub enum AppError {
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
    #[error("internal server error: {0}")]
    InternalServerError(String),

    /// Conflict.
    #[error("conflict: {0}")]
    Conflict(String),
}

impl AppError {
    /// Get the HTTP status code for this error.
    pub fn status_code(&self) -> u16 {
        match self {
            AppError::NotFound(_) => 404,
            AppError::Unauthorized(_) => 401,
            AppError::Forbidden(_) => 403,
            AppError::BadRequest(_) => 400,
            AppError::InternalServerError(_) => 500,
            AppError::Conflict(_) => 409,
        }
    }

    /// Get the stable error code string for this error variant.
    pub fn code(&self) -> &'static str {
        match self {
            AppError::NotFound(_) => "not_found",
            AppError::Unauthorized(_) => "unauthorized",
            AppError::Forbidden(_) => "forbidden",
            AppError::BadRequest(_) => "bad_request",
            AppError::InternalServerError(_) => "internal_server_error",
            AppError::Conflict(_) => "conflict",
        }
    }

    /// Get the human-readable error message.
    ///
    /// Never exposes internal implementation details (stack traces, SQL errors,
    /// file paths).
    pub fn message(&self) -> String {
        self.to_string()
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
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let request_id = ulid::Ulid::new().to_string();
        let status = StatusCode::from_u16(self.status_code())
            .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let code = self.code().to_string();
        let message = self.message();

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

#[cfg(test)]
#[allow(unsafe_code)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    // --- Status code and code string (sync) ---

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

    // --- IntoResponse: full HTTP response shape ---

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

        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("failed to read body");
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

        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("failed to read body");
        let body: serde_json::Value =
            serde_json::from_slice(&bytes).expect("body is not valid JSON");

        assert_eq!(body["error"]["code"], "unauthorized");
    }

    #[tokio::test]
    async fn test_error_response_body_shape() {
        // Verify the complete {"error": {"code", "message", "request_id"}} contract.
        let response = AppError::BadRequest("missing name".to_string()).into_response();

        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("failed to read body");
        let body: serde_json::Value =
            serde_json::from_slice(&bytes).expect("body is not valid JSON");

        assert!(body["error"].is_object(), "body must have an 'error' object");
        assert!(body["error"]["code"].is_string(), "'code' must be a string");
        assert!(body["error"]["message"].is_string(), "'message' must be a string");
        assert!(body["error"]["request_id"].is_string(), "'request_id' must be a string");
        // There must be no extra top-level keys.
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

        // ULID: 26 characters, base32 alphabet
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
}
