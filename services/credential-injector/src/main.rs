//! Credential Injector sidecar service.
//!
//! Exposes `POST /inject` over HTTP. The gateway calls this endpoint with a
//! [`mcp_credential_injector::inject::RequestSkeleton`] and receives the
//! upstream response after the sidecar has injected the stored credential.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::{routing::get, Extension, Router};
use mcp_common::{
    health::{live_handler, pg_pool_checker, ready_handler, HealthState},
    init_telemetry, metrics_handler, track_metrics, AuditLogger, FromEnv,
};
use mcp_credential_injector::{
    build_app_state, build_inject_router,
    cache::new_cache,
    config::Config,
    notify::spawn_notify_listener,
};
use mcp_crypto::CryptoKey;
use tower_http::trace::TraceLayer;

#[tokio::main]
async fn main() {
    // Validate configuration before initializing any subsystems.
    // Fail immediately with all missing/malformed variables listed.
    let cfg = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("credential-injector: {e}");
            std::process::exit(1);
        }
    };

    let _telemetry = match init_telemetry("mcp-credential-injector", env!("CARGO_PKG_VERSION")) {
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

    // Create a lazy pool — connects on first use, so startup is non-blocking.
    // /health/ready will return 503 until the DB is reachable.
    let db_pool = match sqlx::PgPool::connect_lazy(&cfg.database_url) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("failed to create database pool: {e}");
            std::process::exit(1);
        }
    };

    let crypto_key = Arc::new(CryptoKey::from_bytes(cfg.encryption_key));
    let audit_logger = AuditLogger::new(db_pool.clone());
    let cache = Arc::new(new_cache());
    let upstream_timeout = Duration::from_secs(cfg.upstream_timeout_secs);

    // Spawn the NOTIFY listener before building the state so the cache is
    // shared between the listener and the inject handler.
    spawn_notify_listener(cfg.database_url.clone(), Arc::clone(&cache));

    let state = match build_app_state(
        db_pool.clone(),
        crypto_key,
        audit_logger.clone(),
        Arc::clone(&cache),
        cfg.allowed_caller_ips,
        cfg.shared_secret,
        upstream_timeout,
    ) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("failed to build HTTP client: {e}");
            std::process::exit(1);
        }
    };

    tracing::info!(
        port = cfg.port,
        "starting credential injector"
    );

    let health_state = HealthState {
        service: "mcp-credential-injector",
        version: env!("CARGO_PKG_VERSION"),
        db_checker: pg_pool_checker(db_pool),
    };

    // Health routes are intentionally outside TraceLayer and metrics middleware
    // so they do not emit OTEL spans and do not skew request metrics.
    let health_router = Router::new()
        .route("/health/live", get(live_handler))
        .route("/health/ready", get(ready_handler))
        .layer(Extension(health_state));

    let api_router = Router::new()
        .route("/metrics", get(metrics_handler))
        .layer(Extension(prom_handle))
        .layer(TraceLayer::new_for_http())
        .layer(axum::middleware::from_fn(track_metrics));

    let inject_router = build_inject_router(state)
        .layer(TraceLayer::new_for_http())
        .layer(axum::middleware::from_fn(track_metrics));

    let app = Router::new()
        .merge(health_router)
        .merge(api_router)
        .merge(inject_router);

    let addr = SocketAddr::from(([0, 0, 0, 0], cfg.port));
    tracing::info!("listening on {}", addr);

    match tokio::net::TcpListener::bind(&addr).await {
        Ok(listener) => {
            if let Err(e) = axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            {
                tracing::error!("server error: {}", e);
                std::process::exit(1);
            }
        }
        Err(e) => {
            tracing::error!("failed to bind to {}: {}", addr, e);
            std::process::exit(1);
        }
    }

    // Drain the audit log before exiting.
    audit_logger.shutdown().await;
}
