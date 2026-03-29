//! Environment-validated configuration for the Gateway service.
//!
//! Call [`Config::from_env`] at startup — it collects every missing or malformed variable
//! and returns a single [`mcp_common::ConfigErrors`] so operators see all problems at once.

use mcp_common::{env_optional, env_optional_parsed, env_required, ConfigErrors, FromEnv};

/// Runtime configuration for the Gateway service.
///
/// All fields are loaded from environment variables by [`FromEnv::from_env`].
/// Fields not yet consumed by service logic are retained for future epics.
#[derive(Debug)]
#[allow(dead_code)]
pub struct Config {
    /// TCP port the gateway binds to. Set via `GATEWAY_PORT` (default: `3000`).
    pub port: u16,

    /// PostgreSQL connection URL. Set via `DATABASE_URL` (required).
    pub database_url: String,

    /// Path to the credential injector Unix domain socket or HTTP address.
    /// Set via `INJECTOR_SOCKET_PATH` (default: `/tmp/recast-injector.sock`).
    pub injector_socket_path: String,

    /// Maximum upstream response body size in bytes.
    /// Set via `UPSTREAM_MAX_RESPONSE_BYTES` (default: `102400`).
    pub upstream_max_response_bytes: usize,

    /// Upstream request timeout in seconds.
    /// Set via `UPSTREAM_TIMEOUT_SECS` (default: `30`).
    pub upstream_timeout_secs: u64,

    /// Maximum MCP tool calls per minute per server (token-bucket rate limit).
    /// Set via `RATE_LIMIT_CALLS_PER_MIN_PER_SERVER` (default: `100`).
    pub rate_limit_calls_per_min_per_server: u32,

    /// Maximum MCP tool calls per minute per user (token-bucket rate limit).
    /// Set via `RATE_LIMIT_CALLS_PER_MIN_PER_USER` (default: `1000`).
    pub rate_limit_calls_per_min_per_user: u32,

    /// Redis connection URL for rate-limit token buckets.
    /// Set via `REDIS_URL` (default: `redis://127.0.0.1:6379`).
    /// When Redis is unreachable, the gateway falls back to in-process buckets.
    pub redis_url: String,

    /// Enable the rate-limit middleware.
    /// Set via `FEATURE_RATE_LIMIT_ENABLED` (default: `true`).
    /// Set to `false` to disable entirely; no `X-RateLimit-*` headers are
    /// added when disabled.
    pub feature_rate_limit_enabled: bool,
}

impl FromEnv for Config {
    type Error = ConfigErrors;

    fn from_env() -> Result<Self, Self::Error> {
        let mut errors = ConfigErrors::new();

        // ── Required ──────────────────────────────────────────────────────────
        let database_url = env_required(&mut errors, "DATABASE_URL");

        // ── Optional (parsed) ─────────────────────────────────────────────────
        let port: u16 = env_optional_parsed(&mut errors, "GATEWAY_PORT", 3000);
        let upstream_max_response_bytes: usize =
            env_optional_parsed(&mut errors, "UPSTREAM_MAX_RESPONSE_BYTES", 102_400);
        let upstream_timeout_secs: u64 =
            env_optional_parsed(&mut errors, "UPSTREAM_TIMEOUT_SECS", 30);
        let rate_limit_calls_per_min_per_server: u32 =
            env_optional_parsed(&mut errors, "RATE_LIMIT_CALLS_PER_MIN_PER_SERVER", 100);
        let rate_limit_calls_per_min_per_user: u32 =
            env_optional_parsed(&mut errors, "RATE_LIMIT_CALLS_PER_MIN_PER_USER", 1000);

        // ── Optional (string) ─────────────────────────────────────────────────
        let injector_socket_path =
            env_optional("INJECTOR_SOCKET_PATH", "/tmp/recast-injector.sock");
        let redis_url = env_optional("REDIS_URL", "redis://127.0.0.1:6379");

        let feature_rate_limit_enabled: bool =
            env_optional_parsed(&mut errors, "FEATURE_RATE_LIMIT_ENABLED", true);

        if !errors.is_empty() {
            return Err(errors);
        }

        match database_url {
            Some(database_url) => Ok(Config {
                port,
                database_url,
                injector_socket_path,
                upstream_max_response_bytes,
                upstream_timeout_secs,
                rate_limit_calls_per_min_per_server,
                rate_limit_calls_per_min_per_user,
                redis_url,
                feature_rate_limit_enabled,
            }),
            // Logically unreachable: env_required pushes an error and returns None
            // whenever the variable is absent, so errors would be non-empty above.
            None => Err(errors),
        }
    }
}
