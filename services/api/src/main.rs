//! Platform API service.
//!
//! Entry point for the Recast MCP Platform API. Owns the application startup
//! lifecycle: telemetry init, config validation, database pool creation, router
//! assembly, and graceful shutdown sequencing.

// Module declarations live in lib.rs so integration tests can import them.
// The binary imports everything from the lib crate (same package).
use mcp_api::app_state::{AppState, SsrfValidatorFn};
use mcp_api::auth::JwksCache;
use mcp_api::config::ApiConfig;
use mcp_api::credentials::CredentialService;
use mcp_api::router::build_router;
use mcp_api::servers::ServerService;
use mcp_api::shutdown::shutdown_signal;

use mcp_common::{init_telemetry, validate_url_with_dns, AuditLogger, FromEnv};
use mcp_crypto::CryptoKey;
use sqlx::postgres::PgPoolOptions;
use std::{net::SocketAddr, sync::Arc, time::Duration};

// ── main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    // Validate configuration before initialising any subsystems.
    // Fail immediately with all missing/malformed variables listed.
    let cfg = match ApiConfig::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("platform-api: {e}");
            std::process::exit(1);
        }
    };

    let _telemetry = match init_telemetry("mcp-api", env!("CARGO_PKG_VERSION")) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("failed to initialize telemetry: {e}");
            std::process::exit(1);
        }
    };

    let prom_handle = match metrics_exporter_prometheus::PrometheusBuilder::new()
        .install_recorder()
    {
        Ok(h) => h,
        Err(e) => {
            tracing::error!("failed to install prometheus recorder: {e}");
            std::process::exit(1);
        }
    };

    // Create a pool with explicit sizing and timeout settings.
    // `connect_lazy` defers the first physical connection until a query is issued,
    // so startup is non-blocking. /health/ready returns 503 until the DB is reachable.
    let pool = match PgPoolOptions::new()
        .max_connections(20)
        .min_connections(2)
        .acquire_timeout(Duration::from_secs(5))
        .idle_timeout(Duration::from_secs(300))
        .max_lifetime(Duration::from_secs(1800))
        .connect_lazy(&cfg.database_url)
    {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("failed to create database pool: {e}");
            std::process::exit(1);
        }
    };

    let audit_logger = AuditLogger::new(pool.clone());
    let jwks_cache = JwksCache::new(&cfg.clerk_jwks_url);

    let crypto_key = match CryptoKey::from_hex(&cfg.encryption_key) {
        Ok(k) => Arc::new(k),
        Err(e) => {
            tracing::error!("invalid MCP_ENCRYPTION_KEY: {e}");
            std::process::exit(1);
        }
    };
    let credential_service =
        CredentialService::new(pool.clone(), crypto_key, audit_logger.clone());

    tracing::info!(
        port = cfg.port,
        cors_origins = ?cfg.cors_origins,
        "starting platform api"
    );

    let port = cfg.port;
    let server_service = ServerService::new(
        pool.clone(),
        audit_logger.clone(),
        cfg.gateway_base_url.clone(),
    );

    // Shared HTTP client for outbound proxy test requests.
    // TCP connect timeout: 10 s. Per-request read timeout is enforced in the
    // proxy handler via tokio::select! so timeouts are distinguishable from
    // connectivity errors.
    let http_client = match reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("failed to build HTTP client: {e}");
            std::process::exit(1);
        }
    };

    // Production SSRF validator: Phase 1 blocklist + async DNS resolution.
    let ssrf_validator: SsrfValidatorFn = Arc::new(|url: url::Url| {
        Box::pin(async move { validate_url_with_dns(&url).await })
    });

    let state = AppState {
        pool: pool.clone(),
        config: Arc::new(cfg),
        audit_logger: audit_logger.clone(),
        jwks_cache,
        credential_service,
        server_service,
        http_client,
        ssrf_validator,
        proxy_timeout: Duration::from_secs(30),
    };

    let app = build_router(state, prom_handle);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));

    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("failed to bind to {addr}: {e}");
            std::process::exit(1);
        }
    };

    tracing::info!("listening on {}", addr);

    // Run the server; stop accepting new connections on SIGTERM/SIGINT.
    // In-flight requests are given up to 30 s to complete before we force shutdown.
    let serve = axum::serve(listener, app).with_graceful_shutdown(shutdown_signal());

    match tokio::time::timeout(Duration::from_secs(35), serve).await {
        Ok(Ok(())) => tracing::info!("server drained gracefully"),
        Ok(Err(e)) => tracing::error!("server error: {e}"),
        Err(_) => tracing::warn!("graceful shutdown drain timeout after 30 s — forcing close"),
    }

    // ── Post-shutdown cleanup sequence ────────────────────────────────────────

    // 1. Flush the audit log — give it 5 s before abandoning remaining events.
    tracing::info!("flushing audit log");
    tokio::time::timeout(Duration::from_secs(5), audit_logger.shutdown())
        .await
        .ok();

    // 2. Close the database pool — waits for borrowed connections to be returned.
    tracing::info!("closing database pool");
    pool.close().await;

    // 3. TelemetryGuard drops here, which flushes the OTLP trace exporter.
    tracing::info!("telemetry flushed — goodbye");
}
