//! Configuration loading utilities for service startup.

use std::{env, fmt, str::FromStr};

/// Errors that can occur when loading configuration from the environment.
#[derive(Debug, Clone, thiserror::Error)]
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

/// A collection of [`ConfigError`]s accumulated during [`crate::FromEnv::from_env`].
///
/// Services collect all errors before reporting so that operators see every
/// misconfigured variable in a single startup message rather than one at a time.
#[derive(Debug, Default)]
pub struct ConfigErrors(Vec<ConfigError>);

impl ConfigErrors {
    /// Create an empty collection.
    #[must_use]
    pub fn new() -> Self {
        Self(Vec::new())
    }

    /// Append an error to the collection.
    pub fn push(&mut self, e: ConfigError) {
        self.0.push(e);
    }

    /// Returns `true` when no errors have been accumulated.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Returns the number of accumulated errors.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl fmt::Display for ConfigErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "configuration errors ({} total):", self.0.len())?;
        for e in &self.0 {
            write!(f, "\n  - {e}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ConfigErrors {}

// ── Environment helpers ────────────────────────────────────────────────────

/// Read a **required** environment variable.
///
/// Returns `Some(value)` on success. On failure, pushes a [`ConfigError::MissingVar`]
/// onto `errors` and returns `None` so that callers can continue collecting all errors
/// before returning.
///
/// # Examples
///
/// ```rust,no_run
/// use mcp_common::config::{ConfigErrors, env_required};
///
/// let mut errors = ConfigErrors::new();
/// let db_url = env_required(&mut errors, "DATABASE_URL");
/// ```
pub fn env_required(errors: &mut ConfigErrors, var: &str) -> Option<String> {
    match env::var(var) {
        Ok(v) => Some(v),
        Err(_) => {
            errors.push(ConfigError::MissingVar(var.to_string()));
            None
        }
    }
}

/// Read an **optional** environment variable, falling back to `default`.
///
/// No error is recorded when the variable is absent.
///
/// # Examples
///
/// ```rust
/// use mcp_common::config::env_optional;
///
/// let log_level = env_optional("RUST_LOG", "info");
/// assert!(!log_level.is_empty());
/// ```
#[must_use]
pub fn env_optional(var: &str, default: &str) -> String {
    env::var(var).unwrap_or_else(|_| default.to_string())
}

/// Read an **optional** environment variable and parse it as `T`.
///
/// - If the variable is absent, `default` is returned silently.
/// - If the variable is present but cannot be parsed, a [`ConfigError::InvalidValue`]
///   is pushed onto `errors` and `default` is returned so that collection continues.
///
/// # Examples
///
/// ```rust,no_run
/// use mcp_common::config::{ConfigErrors, env_optional_parsed};
///
/// let mut errors = ConfigErrors::new();
/// let port: u16 = env_optional_parsed(&mut errors, "GATEWAY_PORT", 3000);
/// ```
pub fn env_optional_parsed<T>(errors: &mut ConfigErrors, var: &str, default: T) -> T
where
    T: FromStr,
    T::Err: fmt::Display,
{
    match env::var(var) {
        Err(_) => default,
        Ok(raw) => match raw.parse::<T>() {
            Ok(v) => v,
            Err(e) => {
                errors.push(ConfigError::InvalidValue {
                    var: var.to_string(),
                    reason: e.to_string(),
                });
                default
            }
        },
    }
}

// ── AES-256-GCM key loader ─────────────────────────────────────────────────

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
#[allow(unsafe_code)]
mod tests {
    use super::*;
    use crate::FromEnv;

    // ── Helpers that mirror load_encryption_key without touching the live env ──

    /// Decode a hex string into a 32-byte array, returning a `ConfigError` on failure.
    fn decode_hex_key(hex_str: &str) -> Result<[u8; 32], ConfigError> {
        let bytes = hex::decode(hex_str).map_err(|_| ConfigError::InvalidValue {
            var: "ENCRYPTION_KEY".to_string(),
            reason: "value is not valid hexadecimal".to_string(),
        })?;
        let len = bytes.len();
        bytes.try_into().map_err(|_| ConfigError::InvalidValue {
            var: "ENCRYPTION_KEY".to_string(),
            reason: format!("expected 32 bytes (64 hex chars) but decoded {len} bytes"),
        })
    }

    /// Like `load_encryption_key` but reads from a caller-supplied env var name.
    fn load_from_var(var: &str) -> Result<[u8; 32], ConfigError> {
        let hex_str = env::var(var).map_err(|_| ConfigError::MissingVar(var.to_string()))?;
        decode_hex_key(&hex_str).map_err(|e| match e {
            ConfigError::InvalidValue { reason, .. } => ConfigError::InvalidValue {
                var: var.to_string(),
                reason,
            },
            other => other,
        })
    }

    #[test]
    fn test_load_encryption_key_missing_env_var() {
        let result = load_from_var("ENCRYPTION_KEY_TEST_MISSING_XYZ");
        assert!(
            matches!(result, Err(ConfigError::MissingVar(ref v)) if v == "ENCRYPTION_KEY_TEST_MISSING_XYZ"),
            "expected MissingVar, got: {result:?}"
        );
    }

    #[test]
    fn test_load_encryption_key_wrong_length() {
        let short_hex = "a".repeat(62);
        let result = decode_hex_key(&short_hex);
        assert!(
            matches!(result, Err(ConfigError::InvalidValue { .. })),
            "expected InvalidValue for short key, got: {result:?}"
        );
    }

    #[test]
    fn test_load_encryption_key_invalid_hex() {
        let result = decode_hex_key(
            "not-valid-hex!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!",
        );
        assert!(
            matches!(result, Err(ConfigError::InvalidValue { .. })),
            "expected InvalidValue for bad hex, got: {result:?}"
        );
    }

    #[test]
    fn test_load_encryption_key_valid() {
        let valid_hex = "a".repeat(64);
        let result = decode_hex_key(&valid_hex);
        assert!(
            result.is_ok(),
            "expected Ok for valid 32-byte hex key, got: {result:?}"
        );
        let key = result.expect("key should be valid in test");
        assert_eq!(key.len(), 32);
    }

    // ── FromEnv integration tests using a minimal test Config ─────────────

    /// Global mutex that serializes tests which mutate or depend on the
    /// `TEST_CFG_ALPHA_7F3B` / `TEST_CFG_BETA_7F3B` environment variables.
    /// Without serialization, `test_from_env_succeeds_with_all_vars_set` may
    /// run concurrently with `test_from_env_fails_with_multiple_missing_vars_and_lists_all`,
    /// causing the latter to see the vars that the former sets.
    static ENV_CFG_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A minimal two-field config used to exercise `FromEnv` in tests.
    /// Uses env var names unlikely to conflict with any real service var.
    #[derive(Debug)]
    struct TestConfig {
        alpha: String,
        beta: String,
    }

    impl FromEnv for TestConfig {
        type Error = ConfigErrors;

        fn from_env() -> Result<Self, Self::Error> {
            let mut errors = ConfigErrors::new();
            let alpha = env_required(&mut errors, "TEST_CFG_ALPHA_7F3B");
            let beta = env_required(&mut errors, "TEST_CFG_BETA_7F3B");

            if !errors.is_empty() {
                return Err(errors);
            }

            match (alpha, beta) {
                (Some(alpha), Some(beta)) => Ok(TestConfig { alpha, beta }),
                // logically unreachable: env_required always pushes an error when it returns None
                _ => Err(errors),
            }
        }
    }

    #[test]
    fn test_from_env_succeeds_with_all_vars_set() {
        // Hold the mutex for the lifetime of this test to prevent concurrent
        // tests from seeing stale env vars. Recover from poisoning so a
        // previous test panic does not permanently block this test.
        let _guard = ENV_CFG_MUTEX.lock().unwrap_or_else(|e| e.into_inner());

        // Safety: these uniquely-prefixed names are not used by any other test.
        // SAFETY: test-only env mutation with collision-resistant names.
        unsafe {
            env::set_var("TEST_CFG_ALPHA_7F3B", "hello");
            env::set_var("TEST_CFG_BETA_7F3B", "world");
        }

        let result = TestConfig::from_env();

        unsafe {
            env::remove_var("TEST_CFG_ALPHA_7F3B");
            env::remove_var("TEST_CFG_BETA_7F3B");
        }

        assert!(result.is_ok(), "expected Ok, got: {}", result.unwrap_err());
        let cfg = result.expect("config must be Ok in success test");
        assert_eq!(cfg.alpha, "hello");
        assert_eq!(cfg.beta, "world");
    }

    #[test]
    fn test_from_env_fails_with_single_missing_var() {
        // Only alpha is absent; beta is absent too by default but we only care
        // that at least one error is reported with the correct variable name.
        // Both TEST_CFG_* vars are absent → ensure we get MissingVar for alpha.
        let mut errors = ConfigErrors::new();
        let _ = env_required(&mut errors, "TEST_CFG_MISSING_SINGLE_9A1C");
        assert_eq!(errors.len(), 1);
        let msg = errors.to_string();
        assert!(
            msg.contains("TEST_CFG_MISSING_SINGLE_9A1C"),
            "error message must name the missing variable; got: {msg}"
        );
    }

    #[test]
    fn test_from_env_fails_with_multiple_missing_vars_and_lists_all() {
        // Serialize with respect to test_from_env_succeeds_with_all_vars_set
        // to ensure the vars are absent when this test runs.
        let _guard = ENV_CFG_MUTEX.lock().unwrap_or_else(|e| e.into_inner());

        // Both absent — FromEnv must report both in one error.
        let result = TestConfig::from_env();
        assert!(result.is_err(), "expected Err when both required vars are absent");

        let errors = result.unwrap_err();
        assert_eq!(
            errors.len(),
            2,
            "expected 2 errors for 2 missing vars, got {}",
            errors.len()
        );

        let msg = errors.to_string();
        assert!(
            msg.contains("TEST_CFG_ALPHA_7F3B"),
            "error message must name ALPHA; got: {msg}"
        );
        assert!(
            msg.contains("TEST_CFG_BETA_7F3B"),
            "error message must name BETA; got: {msg}"
        );
    }

    #[test]
    fn test_config_errors_display_lists_all_errors() {
        let mut errors = ConfigErrors::new();
        errors.push(ConfigError::MissingVar("VAR_ONE".to_string()));
        errors.push(ConfigError::MissingVar("VAR_TWO".to_string()));
        errors.push(ConfigError::InvalidValue {
            var: "VAR_THREE".to_string(),
            reason: "not a number".to_string(),
        });

        let msg = errors.to_string();
        assert!(msg.contains("3 total"), "must show count; got: {msg}");
        assert!(msg.contains("VAR_ONE"), "must list VAR_ONE; got: {msg}");
        assert!(msg.contains("VAR_TWO"), "must list VAR_TWO; got: {msg}");
        assert!(msg.contains("VAR_THREE"), "must list VAR_THREE; got: {msg}");
    }

    #[test]
    fn test_env_optional_parsed_invalid_value_pushes_error() {
        let mut errors = ConfigErrors::new();
        // Set a non-numeric value for a port variable
        unsafe {
            env::set_var("TEST_PORT_INVALID_9B2D", "not-a-port");
        }
        let port: u16 = env_optional_parsed(&mut errors, "TEST_PORT_INVALID_9B2D", 9999);
        unsafe {
            env::remove_var("TEST_PORT_INVALID_9B2D");
        }

        assert_eq!(port, 9999, "default must be returned on parse failure");
        assert_eq!(errors.len(), 1, "one error must be accumulated");
        let msg = errors.to_string();
        assert!(msg.contains("TEST_PORT_INVALID_9B2D"), "error must name the variable; got: {msg}");
    }
}
