//! Per-server Bearer token authentication for the MCP gateway.
//!
//! Each MCP server has its own cryptographically random Bearer token stored
//! as an Argon2id PHC hash in `mcp_servers.token_hash`. The gateway validates
//! incoming `Authorization: Bearer <token>` headers against the stored hash.
//!
//! # Token format
//!
//! Raw token: 32 random bytes encoded as URL-safe base64 without padding —
//! 43 characters. Clients present this value in the `Authorization` header.
//!
//! Stored hash: Argon2id PHC string (`$argon2id$v=19$m=65536,t=1,p=1$...`).
//! Parameters: memory=64 MiB, 1 iteration, 1 lane. Verification takes ~2 ms.
//!
//! # Concurrency model
//!
//! A `tokio::sync::Semaphore` limits concurrent Argon2 operations to
//! `available_parallelism() × 2` to prevent CPU saturation under load.
//! A short-lived `moka` cache (TTL 30 s) stores recent validation results
//! so repeated requests from the same client skip the Argon2 computation.

use crate::cache::ServerConfig;
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Algorithm, Argon2, Params, Version,
};
use base64ct::{Base64UrlUnpadded, Encoding};
use moka::sync::Cache;
use std::{sync::Arc, time::Duration};
use thiserror::Error;
use tokio::sync::Semaphore;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Number of random bytes in a raw Bearer token.
const TOKEN_BYTES: usize = 32;

/// Number of leading characters of the raw token that are safe to log.
pub const TOKEN_PREFIX_LEN: usize = 8;

/// Argon2id memory cost in kibibytes (64 MiB).
const ARGON2_MEMORY_KIB: u32 = 65_536;

/// Argon2id time cost (iterations).
const ARGON2_TIME_COST: u32 = 1;

/// Argon2id parallelism (lanes).
const ARGON2_PARALLELISM: u32 = 1;

/// Validation cache TTL in seconds.
const CACHE_TTL_SECS: u64 = 30;

/// Maximum number of entries in the validation cache.
const CACHE_MAX_CAPACITY: u64 = 100_000;

// ── Error types ───────────────────────────────────────────────────────────────

/// Errors returned by [`TokenValidator::validate_request`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum AuthError {
    /// `Authorization` header was absent from the request.
    #[error("missing Authorization header")]
    MissingHeader,

    /// `Authorization` header was present but not in `Bearer <token>` form.
    #[error("malformed Authorization header")]
    MalformedHeader,

    /// Token was syntactically valid but did not match the stored Argon2id hash.
    #[error("invalid token")]
    InvalidToken,

    /// Token matched but the server is suspended; respond with HTTP 403.
    #[error("server is suspended")]
    ServerSuspended,
}

/// Errors returned by [`generate_token`].
#[derive(Debug, Error)]
pub enum TokenGenerationError {
    /// Argon2id parameter construction or hashing failure.
    #[error("argon2 error: {0}")]
    Argon2(String),
}

// ── TokenValidator ────────────────────────────────────────────────────────────

/// Validates Bearer tokens for incoming MCP requests.
///
/// Create once at startup, wrap in `Arc`, and share across request handlers:
///
/// ```ignore
/// let validator = Arc::new(TokenValidator::new());
/// ```
///
/// All methods are safe to call concurrently. The internal semaphore and
/// moka cache are already `Send + Sync`.
pub struct TokenValidator {
    /// Limits concurrent Argon2id operations to prevent CPU saturation.
    argon2_semaphore: Arc<Semaphore>,
    /// Recent validation results. Key: `"<server_id>:<raw_token>"`.
    /// Value: `true` (token valid) or `false` (token invalid).
    validation_cache: Cache<String, bool>,
}

impl TokenValidator {
    /// Construct a new validator with CPU-aware Argon2 concurrency limits.
    ///
    /// Semaphore capacity = `available_parallelism() × 2` (default 8 if
    /// parallelism cannot be queried). Validation cache TTL = 30 seconds.
    pub fn new() -> Self {
        let parallelism = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let semaphore_permits = parallelism * 2;

        let validation_cache = Cache::builder()
            .max_capacity(CACHE_MAX_CAPACITY)
            .time_to_live(Duration::from_secs(CACHE_TTL_SECS))
            .build();

        Self {
            argon2_semaphore: Arc::new(Semaphore::new(semaphore_permits)),
            validation_cache,
        }
    }

    /// Validate an `Authorization: Bearer <token>` header against a server config.
    ///
    /// # Returns
    ///
    /// - `Ok(())` — token is valid and the server is `active`.
    /// - `Err(AuthError::MissingHeader)` — `Authorization` header absent.
    /// - `Err(AuthError::MalformedHeader)` — header present but not `Bearer <token>`.
    /// - `Err(AuthError::InvalidToken)` — token does not match the stored hash,
    ///   or no hash is configured for the server.
    /// - `Err(AuthError::ServerSuspended)` — token is valid but server is `"suspended"`.
    ///
    /// # Security
    ///
    /// The raw token value is **never** written to any log. Only the
    /// [`TOKEN_PREFIX_LEN`]-character prefix is safe to log.
    pub async fn validate_request(
        &self,
        authorization_header: Option<&str>,
        server_config: &ServerConfig,
    ) -> Result<(), AuthError> {
        let raw_token = extract_bearer_token(authorization_header)?;

        // Check the short-lived validation cache to skip re-hashing.
        let cache_key = format!("{}:{}", server_config.id, raw_token);
        if let Some(cached_valid) = self.validation_cache.get(&cache_key) {
            if cached_valid {
                return check_suspended(server_config);
            } else {
                return Err(AuthError::InvalidToken);
            }
        }

        // Cache miss — extract the stored hash and run Argon2id verification.
        let token_hash = match server_config.token_hash.as_deref() {
            Some(h) if !h.is_empty() => h.to_owned(),
            _ => {
                // No hash configured: reject and cache the negative result.
                self.validation_cache.insert(cache_key, false);
                return Err(AuthError::InvalidToken);
            }
        };

        // Acquire a semaphore permit before spawning the blocking Argon2 task.
        let _permit = self
            .argon2_semaphore
            .acquire()
            .await
            .map_err(|_| AuthError::InvalidToken)?;

        let raw_token_owned = raw_token.to_owned();
        let is_valid = tokio::task::spawn_blocking(move || {
            verify_argon2(&raw_token_owned, &token_hash)
        })
        .await
        .unwrap_or(false);

        // Cache result; permit is still held during the insert (safe).
        self.validation_cache.insert(cache_key, is_valid);

        if !is_valid {
            return Err(AuthError::InvalidToken);
        }

        check_suspended(server_config)
    }
}

impl Default for TokenValidator {
    fn default() -> Self {
        Self::new()
    }
}

// ── Public utilities ──────────────────────────────────────────────────────────

/// Generate a fresh per-server Bearer token and its Argon2id hash.
///
/// Returns `(raw_token, phc_hash)`:
///
/// - `raw_token` — 32 random bytes encoded as URL-safe base64 without padding
///   (43 chars). This value is given to the MCP client and must be kept secret.
/// - `phc_hash` — Argon2id PHC string (`$argon2id$...`) for storage in
///   `mcp_servers.token_hash`. Parameters: m=65536, t=1, p=1.
///
/// The function is CPU-bound (Argon2id ~2 ms). Call from a blocking context
/// or `tokio::task::spawn_blocking` when called from an async handler.
pub fn generate_token() -> Result<(String, String), TokenGenerationError> {
    let raw_bytes: [u8; TOKEN_BYTES] = rand::random();
    let raw_token = Base64UrlUnpadded::encode_string(&raw_bytes);

    let salt = SaltString::generate(&mut OsRng);
    let params = Params::new(ARGON2_MEMORY_KIB, ARGON2_TIME_COST, ARGON2_PARALLELISM, None)
        .map_err(|e| TokenGenerationError::Argon2(e.to_string()))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let phc_hash = argon2
        .hash_password(raw_token.as_bytes(), &salt)
        .map_err(|e| TokenGenerationError::Argon2(e.to_string()))?
        .to_string();

    Ok((raw_token, phc_hash))
}

/// Extract the first [`TOKEN_PREFIX_LEN`] characters of a raw token.
///
/// The prefix is safe to include in structured logs; it cannot be used to
/// reconstruct the full token or to pass authentication.
pub fn extract_token_prefix(raw_token: &str) -> String {
    raw_token.chars().take(TOKEN_PREFIX_LEN).collect()
}

// ── Private helpers ───────────────────────────────────────────────────────────

/// Parse the raw Bearer token from an `Authorization` header value.
///
/// Expects the format `Bearer <token>` (case-sensitive prefix, single space).
fn extract_bearer_token(authorization: Option<&str>) -> Result<&str, AuthError> {
    let header = authorization.ok_or(AuthError::MissingHeader)?;
    let token = header
        .strip_prefix("Bearer ")
        .ok_or(AuthError::MalformedHeader)?;
    if token.is_empty() {
        return Err(AuthError::MalformedHeader);
    }
    Ok(token)
}

/// Verify `raw_token` against a PHC hash string using Argon2id.
///
/// Returns `true` if the token matches, `false` on any mismatch or error.
/// This function is CPU-bound; run it inside `tokio::task::spawn_blocking`.
fn verify_argon2(raw_token: &str, phc_hash: &str) -> bool {
    let parsed = match PasswordHash::new(phc_hash) {
        Ok(h) => h,
        Err(_) => return false,
    };
    Argon2::default()
        .verify_password(raw_token.as_bytes(), &parsed)
        .is_ok()
}

/// Return `Ok(())` if the server is active or inactive; return
/// `Err(AuthError::ServerSuspended)` when status is `"suspended"`.
fn check_suspended(config: &ServerConfig) -> Result<(), AuthError> {
    if config.status == "suspended" {
        Err(AuthError::ServerSuspended)
    } else {
        Ok(())
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo
)]
mod tests {
    use super::*;
    use crate::cache::ServerConfig;
    use chrono::Utc;
    use uuid::Uuid;

    /// Build a [`ServerConfig`] with the given token hash and status.
    fn make_config_with_hash(token_hash: Option<String>, status: &str) -> ServerConfig {
        ServerConfig {
            id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            name: "Test Server".to_string(),
            slug: "test-server".to_string(),
            description: None,
            config_json: serde_json::json!({}),
            status: status.to_string(),
            config_version: 1,
            token_hash,
            token_prefix: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    // ── valid token accepted ──────────────────────────────────────────────────

    #[tokio::test]
    async fn valid_token_accepted() {
        let (raw, hash) = generate_token().expect("generate_token must not fail");
        let validator = TokenValidator::new();
        let config = make_config_with_hash(Some(hash), "active");
        let auth_header = format!("Bearer {raw}");

        let result = validator.validate_request(Some(&auth_header), &config).await;
        assert!(result.is_ok(), "valid token must be accepted; got {result:?}");
    }

    // ── invalid token rejected ────────────────────────────────────────────────

    #[tokio::test]
    async fn invalid_token_rejected() {
        let (_raw, hash) = generate_token().expect("generate_token must not fail");
        let validator = TokenValidator::new();
        let config = make_config_with_hash(Some(hash), "active");
        // Use a different token that does not match the stored hash.
        let wrong_token = "Bearer AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

        let result = validator.validate_request(Some(wrong_token), &config).await;
        assert_eq!(
            result,
            Err(AuthError::InvalidToken),
            "wrong token must return InvalidToken"
        );
    }

    // ── missing header rejected ───────────────────────────────────────────────

    #[tokio::test]
    async fn missing_header_rejected() {
        let validator = TokenValidator::new();
        let config = make_config_with_hash(None, "active");

        let result = validator.validate_request(None, &config).await;
        assert_eq!(
            result,
            Err(AuthError::MissingHeader),
            "absent Authorization header must return MissingHeader"
        );
    }

    // ── malformed header rejected ─────────────────────────────────────────────

    #[tokio::test]
    async fn malformed_header_rejected() {
        let validator = TokenValidator::new();
        let config = make_config_with_hash(None, "active");

        // Missing "Bearer " prefix.
        let result = validator
            .validate_request(Some("Token abc123"), &config)
            .await;
        assert_eq!(result, Err(AuthError::MalformedHeader));

        // Empty token after "Bearer ".
        let result2 = validator
            .validate_request(Some("Bearer "), &config)
            .await;
        assert_eq!(result2, Err(AuthError::MalformedHeader));
    }

    // ── suspended server returns 403 ─────────────────────────────────────────

    #[tokio::test]
    async fn suspended_server_returns_403() {
        let (raw, hash) = generate_token().expect("generate_token must not fail");
        let validator = TokenValidator::new();
        // Server is suspended even though the token is valid.
        let config = make_config_with_hash(Some(hash), "suspended");
        let auth_header = format!("Bearer {raw}");

        let result = validator.validate_request(Some(&auth_header), &config).await;
        assert_eq!(
            result,
            Err(AuthError::ServerSuspended),
            "valid token on suspended server must return ServerSuspended"
        );
    }

    // ── token prefix correctly extracted ─────────────────────────────────────

    #[test]
    fn token_prefix_correctly_extracted() {
        let raw = "abcdefghijklmnopqrstuvwxyz0123456789ABCDE";
        let prefix = extract_token_prefix(raw);
        assert_eq!(prefix, "abcdefgh", "prefix must be the first 8 chars");
        assert_eq!(prefix.len(), TOKEN_PREFIX_LEN);
    }

    // ── short token prefix does not panic ─────────────────────────────────────

    #[test]
    fn token_prefix_short_token_does_not_panic() {
        let prefix = extract_token_prefix("abc");
        assert_eq!(prefix, "abc");
    }

    // ── generate_token produces 43-char raw token ─────────────────────────────

    #[test]
    fn generate_token_raw_length() {
        let (raw, _hash) = generate_token().expect("generate_token must not fail");
        assert_eq!(raw.len(), 43, "URL-safe base64 of 32 bytes must be 43 chars");
        // Must use only URL-safe base64 alphabet.
        assert!(
            raw.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_'),
            "raw token must be URL-safe base64: {raw}"
        );
    }

    // ── generate_token hash is valid PHC string ───────────────────────────────

    #[test]
    fn generate_token_hash_is_valid_phc() {
        let (_raw, hash) = generate_token().expect("generate_token must not fail");
        assert!(hash.starts_with("$argon2id$"), "hash must be Argon2id PHC; got: {hash}");
    }

    // ── validation cache avoids re-hashing ───────────────────────────────────

    #[tokio::test]
    async fn validation_cache_hit_returns_cached_result() {
        let (raw, hash) = generate_token().expect("generate_token must not fail");
        let validator = TokenValidator::new();
        let config = make_config_with_hash(Some(hash), "active");
        let auth_header = format!("Bearer {raw}");

        // First call: miss → runs Argon2 → caches result.
        let r1 = validator.validate_request(Some(&auth_header), &config).await;
        assert!(r1.is_ok(), "first call must succeed");

        // Second call: cache hit → skips Argon2; must also succeed.
        let r2 = validator.validate_request(Some(&auth_header), &config).await;
        assert!(r2.is_ok(), "cached hit must also succeed");
    }

    // ── no token configured returns InvalidToken ──────────────────────────────

    #[tokio::test]
    async fn no_token_hash_configured_returns_invalid() {
        let validator = TokenValidator::new();
        // Server has no token hash.
        let config = make_config_with_hash(None, "active");

        let result = validator
            .validate_request(Some("Bearer sometoken"), &config)
            .await;
        assert_eq!(result, Err(AuthError::InvalidToken));
    }
}
