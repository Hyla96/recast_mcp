//! Credential Injector — shared library components.
//!
//! This crate is structured as both a binary (`main.rs`) and a library so that
//! integration tests in `tests/` can import application types and handlers
//! directly. The binary entry point in `main.rs` uses items exported here.

/// Shared application state threaded through all axum handlers.
pub mod app_state;

/// In-memory LRU credential cache.
pub mod cache;

/// Environment-validated runtime configuration.
pub mod config;

/// `POST /inject` handler and request/response types.
pub mod inject;

/// PostgreSQL `LISTEN / NOTIFY` listener for cache invalidation.
pub mod notify;

use std::sync::Arc;
use std::time::Duration;

use axum::{routing::post, Router};

use app_state::AppState;

/// Builds the axum [`Router`] for the credential injector.
///
/// The returned router has a single endpoint: `POST /inject`.
/// Health and metrics routes are added separately in `main.rs`.
///
/// The router must be served with
/// `into_make_service_with_connect_info::<SocketAddr>()` so that the
/// [`inject::inject_handler`] can extract the caller IP via [`axum::extract::ConnectInfo`].
pub fn build_inject_router(state: AppState) -> Router {
    Router::new()
        .route("/inject", post(inject::inject_handler))
        .with_state(state)
}

/// Constructs an [`AppState`] from its component parts.
///
/// # Errors
///
/// Returns a [`reqwest::Error`] if the HTTP client cannot be initialised (e.g.
/// the system TLS library is unavailable). This is a fatal startup error.
pub fn build_app_state(
    pool: sqlx::PgPool,
    crypto_key: Arc<mcp_crypto::CryptoKey>,
    audit_logger: mcp_common::AuditLogger,
    cache: Arc<crate::cache::CredentialCache>,
    allowed_ips: Vec<std::net::IpAddr>,
    shared_secret: String,
    upstream_timeout: Duration,
) -> Result<AppState, reqwest::Error> {
    let http_client = reqwest::Client::builder()
        .danger_accept_invalid_certs(false)
        .user_agent("mcp-gateway/0.1.0")
        .redirect(reqwest::redirect::Policy::limited(5))
        .timeout(upstream_timeout)
        .build()?;

    Ok(AppState {
        pool,
        crypto_key,
        audit_logger,
        cache,
        http_client,
        allowed_ips: Arc::new(allowed_ips),
        shared_secret: Arc::new(shared_secret),
        upstream_timeout,
        skip_ssrf: false, // always enforce SSRF in production
    })
}
