//! Application error types.

use serde::Serialize;
use thiserror::Error;

/// Application-level errors.
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

    /// Get the error code string for this error.
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

    /// Get the error message.
    pub fn message(&self) -> String {
        self.to_string()
    }
}

/// Standard API error response body.
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    /// The error details.
    pub error: ErrorDetails,
}

/// Error details in the API response.
#[derive(Debug, Serialize)]
pub struct ErrorDetails {
    /// The error code.
    pub code: String,
    /// The error message.
    pub message: String,
    /// The request ID.
    pub request_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_not_found_serialization() {
        let err = AppError::NotFound("user".to_string());
        let response = ErrorResponse {
            error: ErrorDetails {
                code: err.code().to_string(),
                message: err.message(),
                request_id: "req-123".to_string(),
            },
        };

        let json = serde_json::to_string(&response).expect("failed to serialize");
        assert!(json.contains("\"code\":\"not_found\""));
        assert!(json.contains("\"request_id\":\"req-123\""));
    }

    #[test]
    fn test_unauthorized_status_code() {
        let err = AppError::Unauthorized("invalid token".to_string());
        assert_eq!(err.status_code(), 401);
        assert_eq!(err.code(), "unauthorized");
    }

    #[test]
    fn test_internal_server_error_status_code() {
        let err = AppError::InternalServerError("database unavailable".to_string());
        assert_eq!(err.status_code(), 500);
        assert_eq!(err.code(), "internal_server_error");
    }
}
