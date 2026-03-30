//! Shared application state threaded through all axum handlers.

use crate::{auth::JwksCache, config::ApiConfig, credentials::CredentialService, servers::ServerService};
use mcp_common::AuditLogger;
use std::sync::Arc;

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
}
