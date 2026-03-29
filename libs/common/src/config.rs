//! Configuration loading utilities for service startup.

use std::env;

/// Errors that can occur when loading configuration from the environment.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// A required environment variable is not set.
    #[error("missing required environment variable: {0}")]
    MissingVar(String),

    /// An environment variable is present but its value is invalid.
    #[error("invalid value for environment variable {var}: {reason}")]
    InvalidValue {
        /// The name of the environment variable.
        var: String,
        /// Why the value was rejected.
        reason: String,
    },
}

/// Load the AES-256-GCM encryption key from the `ENCRYPTION_KEY` environment variable.
///
/// The variable must contain exactly 64 lowercase or uppercase hexadecimal characters,
/// representing a 32-byte key.
///
/// # Errors
///
/// Returns [`ConfigError::MissingVar`] if `ENCRYPTION_KEY` is not set.
/// Returns [`ConfigError::InvalidValue`] if the value is not valid hex or does not decode to
/// exactly 32 bytes.
///
/// # Examples
///
/// ```rust,no_run
/// use mcp_common::config::load_encryption_key;
///
/// // Assumes ENCRYPTION_KEY is set to a valid 64-char hex string.
/// let key = load_encryption_key().expect("encryption key must be configured");
/// assert_eq!(key.len(), 32);
/// ```
pub fn load_encryption_key() -> Result<[u8; 32], ConfigError> {
    const VAR: &str = "ENCRYPTION_KEY";

    let hex_str = env::var(VAR).map_err(|_| ConfigError::MissingVar(VAR.to_string()))?;

    let bytes = hex::decode(&hex_str).map_err(|_| ConfigError::InvalidValue {
        var: VAR.to_string(),
        reason: "value is not valid hexadecimal".to_string(),
    })?;

    let len = bytes.len();
    bytes.try_into().map_err(|_| ConfigError::InvalidValue {
        var: VAR.to_string(),
        reason: format!("expected 32 bytes (64 hex chars) but decoded {len} bytes"),
    })
}

#[cfg(test)]
mod tests {
    use std::env;

    /// Decode a hex string into a 32-byte array, returning a `ConfigError` on failure.
    /// Used to test the key-loading logic without mutating the live environment.
    fn decode_hex_key(hex_str: &str) -> Result<[u8; 32], super::ConfigError> {
        let bytes = hex::decode(hex_str).map_err(|_| super::ConfigError::InvalidValue {
            var: "ENCRYPTION_KEY".to_string(),
            reason: "value is not valid hexadecimal".to_string(),
        })?;
        let len = bytes.len();
        bytes.try_into().map_err(|_| super::ConfigError::InvalidValue {
            var: "ENCRYPTION_KEY".to_string(),
            reason: format!("expected 32 bytes (64 hex chars) but decoded {len} bytes"),
        })
    }

    /// Like [`super::load_encryption_key`] but reads from a caller-supplied env var name.
    fn load_from_var(var: &str) -> Result<[u8; 32], super::ConfigError> {
        let hex_str = env::var(var).map_err(|_| super::ConfigError::MissingVar(var.to_string()))?;
        decode_hex_key(&hex_str).map_err(|e| match e {
            super::ConfigError::InvalidValue { reason, .. } => super::ConfigError::InvalidValue {
                var: var.to_string(),
                reason,
            },
            other => other,
        })
    }

    #[test]
    fn test_load_encryption_key_missing_env_var() {
        // Use a deliberately absent env var name to test the MissingVar path
        // without mutating the real ENCRYPTION_KEY (shared across parallel threads).
        let result = load_from_var("ENCRYPTION_KEY_TEST_MISSING_XYZ");
        assert!(
            matches!(result, Err(super::ConfigError::MissingVar(ref v)) if v == "ENCRYPTION_KEY_TEST_MISSING_XYZ"),
            "expected MissingVar, got: {result:?}"
        );
    }

    #[test]
    fn test_load_encryption_key_wrong_length() {
        // 31 bytes = 62 hex chars — one byte too short.
        let short_hex = "a".repeat(62);
        let result = decode_hex_key(&short_hex);
        assert!(
            matches!(result, Err(super::ConfigError::InvalidValue { .. })),
            "expected InvalidValue for short key, got: {result:?}"
        );
    }

    #[test]
    fn test_load_encryption_key_invalid_hex() {
        let result = decode_hex_key("not-valid-hex!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!");
        assert!(
            matches!(result, Err(super::ConfigError::InvalidValue { .. })),
            "expected InvalidValue for bad hex, got: {result:?}"
        );
    }

    #[test]
    fn test_load_encryption_key_valid() {
        // Exactly 64 hex chars = 32 bytes.
        let valid_hex = "a".repeat(64);
        let result = decode_hex_key(&valid_hex);
        assert!(result.is_ok(), "expected Ok for valid 32-byte hex key, got: {result:?}");
        let key = result.expect("key should be valid");
        assert_eq!(key.len(), 32);
    }
}
