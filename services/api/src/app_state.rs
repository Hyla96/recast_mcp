//! Shared application state threaded through all axum handlers.

use crate::{auth::JwksCache, config::ApiConfig, credentials::CredentialService, servers::ServerService};
use mcp_common::AppError;
use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

use mcp_common::AuditLogger;

// ── SSRF validator type ───────────────────────────────────────────────────────

/// The `Future` type returned by an SSRF validator function.
pub type SsrfValidatorFuture = Pin<Box<dyn Future<Output = Result<(), AppError>> + Send>>;

/// An async SSRF validation function.
///
/// In production: wraps [`mcp_common::validate_url_with_dns`].
/// In tests: use a passthrough (`Arc::new(|_| Box::pin(async { Ok(()) }))`) to allow
/// requests to `127.0.0.1` mock servers.
pub type SsrfValidatorFn = Arc<dyn Fn(url::Url) -> SsrfValidatorFuture + Send + Sync>;

// ── AppState ──────────────────────────────────────────────────────────────────

/// Application state available to every request handler via `axum::extract::State<AppState>`.
///
/// All fields are cheap to clone — `PgPool`, `AuditLogger`, `JwksCache`, and `Arc<ApiConfig>`
/// are all internally reference-counted, so `Clone` is O(1) and does not copy data.
#[derive(Clone)]
pub struct AppState {
    /// PostgreSQL connection pool, shared across all request handlers.
    pub pool: sqlx::PgPool,

    /// Service configuration loaded once at startup.
    pub config: Arc<ApiConfig>,

    /// Async audit logger — fire-and-forget, never blocks handlers.
    ///
    /// Stored in `AppState` so that handlers can emit audit events and the
    /// main shutdown sequence can drain the logger before exit.
    pub audit_logger: AuditLogger,

    /// JWKS cache for Clerk JWT public-key validation.
    ///
    /// Wraps an `Arc<RwLock<...>>` internally — cloning is O(1) and all
    /// clones share the same cached key set.
    pub jwks_cache: JwksCache,

    /// Credential encryption/decryption service.
    ///
    /// Wraps the AES-256-GCM key and provides `store`, `rotate`, `delete`,
    /// and `list_for_server` operations. Cloning is O(1) — the key is
    /// `Arc`-wrapped inside the service.
    pub credential_service: CredentialService,

    /// MCP server management service.
    ///
    /// Encapsulates all SQL for server CRUD: create, list, get, update, delete,
    /// and ownership checks. Cloning is O(1) — the pool and audit logger are
    /// `Arc`-wrapped inside the service.
    pub server_service: ServerService,

    /// Shared `reqwest::Client` for outbound proxy test requests.
    ///
    /// Built once at startup with a 10-second TCP connect timeout.
    /// The per-request timeout is enforced in the proxy handler via
    /// `tokio::select!` so the handler can distinguish timeouts from
    /// connectivity errors.
    pub http_client: reqwest::Client,

    /// Async SSRF validation function for proxy test requests.
    ///
    /// Production: wraps `validate_url_with_dns` (Phase 1 + DNS resolution).
    /// Tests: passthrough (`Arc::new(|_| Box::pin(async { Ok(()) }))`) to allow
    /// `127.0.0.1` mock servers without triggering SSRF protection.
    pub ssrf_validator: SsrfValidatorFn,

    /// Maximum duration to wait for the upstream response in proxy test calls.
    ///
    /// Default: 30 seconds (production).
    /// Tests should set a shorter value (e.g. 150 ms) to keep test suites fast.
    pub proxy_timeout: Duration,
}
