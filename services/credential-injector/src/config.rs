//! Environment-validated configuration for the Credential Injector service.
//!
//! Call [`Config::from_env`] at startup — it collects every missing or malformed
//! variable and returns a single [`mcp_common::ConfigErrors`] so operators see all
//! problems at once.

use std::net::{IpAddr, Ipv4Addr};

use mcp_common::{
    env_optional_parsed, env_required, load_encryption_key, ConfigError, ConfigErrors, FromEnv,
};

/// Runtime configuration for the Credential Injector service.
///
/// All fields are loaded from environment variables by [`FromEnv::from_env`].
#[derive(Debug)]
pub struct Config {
    /// TCP port the injector binds to. Set via `INJECTOR_PORT` (default: `3002`).
    pub port: u16,

    /// PostgreSQL connection URL. Set via `DATABASE_URL` (required).
    pub database_url: String,

    /// 32-byte AES-256-GCM encryption key decoded from a 64-char hex string.
    /// Set via `ENCRYPTION_KEY` (required).
    pub encryption_key: [u8; 32],

    /// Shared secret Bearer token for authenticating POST /inject callers.
    /// Set via `MCP_INJECTOR_SHARED_SECRET` (required).
    pub shared_secret: String,

    /// IP addresses allowed to call `POST /inject` (comma-separated list).
    /// Set via `MCP_INJECTOR_ALLOWED_CALLER_IPS` (default: `127.0.0.1`).
    pub allowed_caller_ips: Vec<IpAddr>,

    /// Upstream HTTP request timeout in seconds.
    /// Set via `MCP_INJECTOR_UPSTREAM_TIMEOUT_SECS` (default: `30`).
    pub upstream_timeout_secs: u64,
}

impl FromEnv for Config {
    type Error = ConfigErrors;

    fn from_env() -> Result<Self, Self::Error> {
        let mut errors = ConfigErrors::new();

        // ── Required ──────────────────────────────────────────────────────────
        let database_url = env_required(&mut errors, "DATABASE_URL");
        let shared_secret = env_required(&mut errors, "MCP_INJECTOR_SHARED_SECRET");

        // ENCRYPTION_KEY has its own validation logic (hex decode + length check).
        let encryption_key = match load_encryption_key() {
            Ok(k) => Some(k),
            Err(e) => {
                errors.push(e);
                None
            }
        };

        // ── Optional (parsed) ─────────────────────────────────────────────────
        let port: u16 = env_optional_parsed(&mut errors, "INJECTOR_PORT", 3002);
        let upstream_timeout_secs: u64 =
            env_optional_parsed(&mut errors, "MCP_INJECTOR_UPSTREAM_TIMEOUT_SECS", 30);

        // Parse allowed IPs (comma-separated). Defaults to 127.0.0.1 if unset.
        let allowed_caller_ips = parse_allowed_ips(&mut errors);

        if !errors.is_empty() {
            return Err(errors);
        }

        match (database_url, encryption_key, shared_secret) {
            (Some(database_url), Some(encryption_key), Some(shared_secret)) => Ok(Config {
                port,
                database_url,
                encryption_key,
                shared_secret,
                allowed_caller_ips,
                upstream_timeout_secs,
            }),
            // Logically unreachable: all None paths push an error above, so
            // `errors` is non-empty and we would have returned Err above.
            _ => Err(errors),
        }
    }
}

/// Parses `MCP_INJECTOR_ALLOWED_CALLER_IPS` as a comma-separated list of IPs.
///
/// Defaults to `["127.0.0.1"]` when the variable is unset.
/// Malformed entries are recorded in `errors` and skipped.
fn parse_allowed_ips(errors: &mut ConfigErrors) -> Vec<IpAddr> {
    match std::env::var("MCP_INJECTOR_ALLOWED_CALLER_IPS") {
        Err(_) => vec![IpAddr::V4(Ipv4Addr::LOCALHOST)],
        Ok(val) => {
            let mut ips: Vec<IpAddr> = Vec::new();
            for part in val.split(',') {
                let trimmed = part.trim();
                match trimmed.parse::<IpAddr>() {
                    Ok(ip) => ips.push(ip),
                    Err(_) => errors.push(ConfigError::InvalidValue {
                        var: "MCP_INJECTOR_ALLOWED_CALLER_IPS".to_string(),
                        reason: format!("'{trimmed}' is not a valid IP address"),
                    }),
                }
            }
            // If all entries were malformed, fall back to loopback.
            if ips.is_empty() {
                ips.push(IpAddr::V4(Ipv4Addr::LOCALHOST));
            }
            ips
        }
    }
}
