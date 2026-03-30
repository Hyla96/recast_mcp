//! Environment-validated configuration for the Platform API service.
//!
//! Call [`ApiConfig::from_env`] at startup — it collects every missing or malformed variable
//! and returns a single [`mcp_common::ConfigErrors`] so operators see all problems at once.

use mcp_common::{env_optional, env_optional_parsed, env_required, ConfigErrors, FromEnv};

/// Runtime configuration for the Platform API service.
///
/// All fields are loaded from environment variables by [`FromEnv::from_env`].
/// Fields not yet consumed by service logic are retained for future epics.
#[derive(Debug)]
#[allow(dead_code)]
pub struct ApiConfig {
    /// TCP port the API binds to. Set via `API_PORT` (default: `3001`).
    pub port: u16,

    /// PostgreSQL connection URL. Set via `DATABASE_URL` (required).
    pub database_url: String,

    /// Clerk secret key for server-side JWT verification.
    /// Set via `CLERK_SECRET_KEY` (required).
    pub clerk_secret_key: String,

    /// Clerk JWKS endpoint URL for JWT public-key validation.
    ///
    /// Set via `CLERK_JWKS_URL` (required). Example:
    /// `https://<frontend-api>/.well-known/jwks.json`.
    pub clerk_jwks_url: String,

    /// Clerk webhook signing secret for Svix signature verification.
    ///
    /// Set via `CLERK_WEBHOOK_SECRET` (required). Obtained from the Clerk
    /// dashboard under Webhooks → Signing Secret. Format: `whsec_<base64>`.
    pub clerk_webhook_secret: String,

    /// Expected JWT issuer (`iss` claim). Set via `CLERK_ISSUER`.
    ///
    /// When set, every JWT must carry a matching `iss` claim.
    /// When empty (not recommended in production), issuer validation is skipped.
    pub clerk_issuer: String,

    /// AES-256-GCM encryption key for credential values, as a 64-character hex string.
    ///
    /// Set via `MCP_ENCRYPTION_KEY` (required). Must decode to exactly 32 bytes.
    /// Generate with: `openssl rand -hex 32`.
    pub encryption_key: String,

    /// Allowed CORS origins as a comma-separated list.
    ///
    /// Set via `MCP_API_CORS_ORIGINS`. When absent or empty, all origins are
    /// allowed (permissive CORS). Example: `https://app.example.com,https://staging.example.com`.
    pub cors_origins: Vec<String>,

    /// Base URL of the MCP Gateway, used to construct the `mcp_url` field
    /// returned in server responses.
    ///
    /// Set via `GATEWAY_BASE_URL` (required). Example: `https://mcp.example.com`.
    pub gateway_base_url: String,
}

impl FromEnv for ApiConfig {
    type Error = ConfigErrors;

    fn from_env() -> Result<Self, Self::Error> {
        let mut errors = ConfigErrors::new();

        // ── Required ──────────────────────────────────────────────────────────
        let database_url = env_required(&mut errors, "DATABASE_URL");
        let clerk_secret_key = env_required(&mut errors, "CLERK_SECRET_KEY");
        let clerk_jwks_url = env_required(&mut errors, "CLERK_JWKS_URL");
        let clerk_webhook_secret = env_required(&mut errors, "CLERK_WEBHOOK_SECRET");
        let encryption_key = env_required(&mut errors, "MCP_ENCRYPTION_KEY");
        let gateway_base_url = env_required(&mut errors, "GATEWAY_BASE_URL");

        // ── Optional (parsed) ─────────────────────────────────────────────────
        let port: u16 = env_optional_parsed(&mut errors, "API_PORT", 3001);

        // ── Optional (raw strings) ────────────────────────────────────────────
        let clerk_issuer = env_optional("CLERK_ISSUER", "");

        // ── Optional (raw string → Vec<String>) ───────────────────────────────
        let cors_origins_raw = env_optional("MCP_API_CORS_ORIGINS", "");
        let cors_origins: Vec<String> = if cors_origins_raw.is_empty() {
            vec![]
        } else {
            cors_origins_raw
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        };

        if !errors.is_empty() {
            return Err(errors);
        }

        match (
            database_url,
            clerk_secret_key,
            clerk_jwks_url,
            clerk_webhook_secret,
            encryption_key,
            gateway_base_url,
        ) {
            (
                Some(database_url),
                Some(clerk_secret_key),
                Some(clerk_jwks_url),
                Some(clerk_webhook_secret),
                Some(encryption_key),
                Some(gateway_base_url),
            ) => Ok(ApiConfig {
                port,
                database_url,
                clerk_secret_key,
                clerk_jwks_url,
                clerk_webhook_secret,
                encryption_key,
                clerk_issuer,
                cors_origins,
                gateway_base_url,
            }),
            // Logically unreachable: env_required pushes an error and returns None
            // whenever the variable is absent, so errors would be non-empty above.
            _ => Err(errors),
        }
    }
}
