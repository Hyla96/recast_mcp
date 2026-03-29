//! Shared types and utilities for all services.

pub mod config;
pub mod error;
pub mod telemetry;

pub use config::{load_encryption_key, ConfigError};
pub use error::AppError;
pub use telemetry::{init_telemetry, TelemetryError, TelemetryGuard};

/// Trait for types that can be loaded from environment variables.
pub trait FromEnv: Sized {
    /// The error type returned when loading from environment fails.
    type Error: std::fmt::Display;

    /// Load this type from environment variables.
    ///
    /// # Errors
    ///
    /// Returns `Self::Error` if any required environment variables are missing or malformed.
    fn from_env() -> Result<Self, Self::Error>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_error_not_found() {
        let err = AppError::NotFound("resource".to_string());
        assert_eq!(err.status_code(), 404);
    }

    #[test]
    fn test_app_error_unauthorized() {
        let err = AppError::Unauthorized("invalid token".to_string());
        assert_eq!(err.status_code(), 401);
    }

    #[test]
    fn test_app_error_internal_server_error() {
        let err = AppError::InternalServerError("database error".to_string());
        assert_eq!(err.status_code(), 500);
    }
}
