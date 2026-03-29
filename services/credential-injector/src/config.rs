//! Environment-validated configuration for the Credential Injector service.
//!
//! Call [`Config::from_env`] at startup — it collects every missing or malformed variable
//! and returns a single [`mcp_common::ConfigErrors`] so operators see all problems at once.

use mcp_common::{env_optional_parsed, env_required, load_encryption_key, ConfigErrors, FromEnv};

/// Runtime configuration for the Credential Injector service.
///
/// All fields are loaded from environment variables by [`FromEnv::from_env`].
/// Fields not yet consumed by service logic are retained for future epics.
#[derive(Debug)]
#[allow(dead_code)]
pub struct Config {
    /// TCP port the injector binds to. Set via `INJECTOR_PORT` (default: `3002`).
    pub port: u16,

    /// PostgreSQL connection URL. Set via `DATABASE_URL` (required).
    pub database_url: String,

    /// 32-byte AES-256-GCM encryption key decoded from a 64-char hex string.
    /// Set via `ENCRYPTION_KEY` (required).
    pub encryption_key: [u8; 32],
}

impl FromEnv for Config {
    type Error = ConfigErrors;

    fn from_env() -> Result<Self, Self::Error> {
        let mut errors = ConfigErrors::new();

        // ── Required ──────────────────────────────────────────────────────────
        let database_url = env_required(&mut errors, "DATABASE_URL");

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

        if !errors.is_empty() {
            return Err(errors);
        }

        match (database_url, encryption_key) {
            (Some(database_url), Some(encryption_key)) => Ok(Config {
                port,
                database_url,
                encryption_key,
            }),
            // Logically unreachable: both paths above push errors when they return None,
            // so errors would be non-empty and we would have returned Err above.
            _ => Err(errors),
        }
    }
}
