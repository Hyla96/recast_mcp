//! Environment-validated configuration for the Platform API service.
//!
//! Call [`Config::from_env`] at startup — it collects every missing or malformed variable
//! and returns a single [`mcp_common::ConfigErrors`] so operators see all problems at once.

use mcp_common::{env_optional_parsed, env_required, ConfigErrors, FromEnv};

/// Runtime configuration for the Platform API service.
///
/// All fields are loaded from environment variables by [`FromEnv::from_env`].
/// Fields not yet consumed by service logic are retained for future epics.
#[derive(Debug)]
#[allow(dead_code)]
pub struct Config {
    /// TCP port the API binds to. Set via `API_PORT` (default: `3001`).
    pub port: u16,

    /// PostgreSQL connection URL. Set via `DATABASE_URL` (required).
    pub database_url: String,

    /// Clerk secret key for server-side JWT verification.
    /// Set via `CLERK_SECRET_KEY` (required).
    pub clerk_secret_key: String,
}

impl FromEnv for Config {
    type Error = ConfigErrors;

    fn from_env() -> Result<Self, Self::Error> {
        let mut errors = ConfigErrors::new();

        // ── Required ──────────────────────────────────────────────────────────
        let database_url = env_required(&mut errors, "DATABASE_URL");
        let clerk_secret_key = env_required(&mut errors, "CLERK_SECRET_KEY");

        // ── Optional (parsed) ─────────────────────────────────────────────────
        let port: u16 = env_optional_parsed(&mut errors, "API_PORT", 3001);

        if !errors.is_empty() {
            return Err(errors);
        }

        match (database_url, clerk_secret_key) {
            (Some(database_url), Some(clerk_secret_key)) => Ok(Config {
                port,
                database_url,
                clerk_secret_key,
            }),
            // Logically unreachable: env_required pushes an error and returns None
            // whenever the variable is absent, so errors would be non-empty above.
            _ => Err(errors),
        }
    }
}
