//! Shared application state for the Credential Injector service.
//!
//! [`AppState`] is cheaply cloneable — all resource-heavy fields use interior
//! `Arc` wrapping, so a clone is a set of pointer increments.

use std::{net::IpAddr, sync::Arc, time::Duration};

use mcp_common::AuditLogger;
use mcp_crypto::CryptoKey;
use sqlx::PgPool;

use crate::cache::CredentialCache;

/// Shared state threaded through all axum handlers.
///
/// Construct once in `main()` and pass to `Router::with_state`.
#[derive(Clone)]
pub struct AppState {
    /// PostgreSQL connection pool used for cache-miss DB lookups and audit
    /// event writes.
    pub pool: PgPool,

    /// AES-256-GCM encryption key. Wrapped in `Arc` because [`CryptoKey`]
    /// is `!Clone` (it zeroes itself on drop).
    pub crypto_key: Arc<CryptoKey>,

    /// Non-blocking batched audit event logger.
    pub audit_logger: AuditLogger,

    /// LRU credential cache keyed by `server_id`.
    /// Wrapped in `Arc` so all handler tasks share one cache.
    pub cache: Arc<CredentialCache>,

    /// Shared `reqwest` HTTP client (internally connection-pooled).
    /// Reused across all requests so OS connections are multiplexed.
    pub http_client: reqwest::Client,

    /// IP addresses permitted to call `POST /inject`.
    pub allowed_ips: Arc<Vec<IpAddr>>,

    /// Shared secret Bearer token that callers must present on every request.
    pub shared_secret: Arc<String>,

    /// Timeout applied to every upstream HTTP request.
    pub upstream_timeout: Duration,

    /// When `true`, skips SSRF validation for the upstream URL.
    ///
    /// **MUST be `false` in all production deployments.** This flag exists
    /// solely for integration tests that direct the injector at a
    /// `MockUpstream` bound to `127.0.0.1`, which would otherwise be blocked
    /// by the SSRF loopback rule. [`build_app_state`] always sets this to
    /// `false`; tests construct [`AppState`] directly with `true` when needed.
    ///
    /// [`build_app_state`]: crate::build_app_state
    pub skip_ssrf: bool,
}
