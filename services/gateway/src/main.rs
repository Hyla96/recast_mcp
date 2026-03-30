//! MCP Gateway service.

mod config;
pub mod auth;
pub mod cache;
pub mod circuit_breaker;
pub mod connections;
pub mod hot_reload;
pub mod logging;
pub mod protocol;
pub mod router;
pub mod sidecar;
pub mod tool_schema;
pub mod transform;
pub mod transport;
pub mod upstream;
pub mod util;

use auth::TokenValidator;
use axum::{routing::get, Extension, Router};
use cache::ConfigCache;
use circuit_breaker::CircuitBreakerRegistry;
use config::Config;
use connections::ConnectionTracker;
use hot_reload::ConfigSyncTask;
use logging::{LogLevel, RequestLogger};
use mcp_common::{
    health::{live_handler, pg_pool_checker, ready_handler, HealthState},
    init_telemetry, metrics_handler, track_metrics, FromEnv,
};
use router::{Router as McpRouter, UpstreamPipeline};
use sidecar::{SidecarPool, UpstreamExecutor};
use std::{net::SocketAddr, path::PathBuf, sync::Arc};
use tower_http::trace::TraceLayer;
use transport::{build_transport_router, TransportState};
use upstream::UpstreamRequestBuilder;
use uuid::Uuid;

#[tokio::main]
async fn main() {
    // Validate configuration before initializing any subsystems.
    // Fail immediately with all missing/malformed variables listed.
    let cfg = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("gateway: {e}");
            std::process::exit(1);
        }
    };

    let _telemetry = match init_telemetry("mcp-gateway", env!("CARGO_PKG_VERSION")) {
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

    tracing::info!(
        port = cfg.port,
        database_url = cfg.database_url,
        injector_socket_path = cfg.injector_socket_path,
        "starting gateway"
    );

    // Pre-warm the in-memory config cache from PostgreSQL.
    let cache = Arc::new(ConfigCache::new(db_pool.clone()));
    match cache.load_all().await {
        Ok(n) => tracing::info!(entries = n, "config cache loaded"),
        Err(e) => {
            tracing::error!(error = %e, "failed to load config cache at startup");
            std::process::exit(1);
        }
    }

    // Start the hot-reload listener. Uses a dedicated PgListener connection
    // separate from the shared request pool.
    let sync_task =
        ConfigSyncTask::new(cfg.database_url.clone(), db_pool.clone(), Arc::clone(&cache));
    // Detach the handle — the task runs for the lifetime of the process.
    let _sync_handle = sync_task.start();

    // ── Build upstream pipeline ───────────────────────────────────────────────
    //
    // S-027: sidecar IPC pool + direct reqwest executor.
    // S-026: request builder (reads GATEWAY_ALLOW_HTTP from env).
    let sidecar_pool = SidecarPool::new(PathBuf::from(&cfg.injector_socket_path));
    let circuit_registry = CircuitBreakerRegistry::new();
    let http_client = reqwest::Client::new();
    let executor = Arc::new(UpstreamExecutor::new(
        sidecar_pool,
        http_client,
        circuit_registry,
    ));
    let request_builder = UpstreamRequestBuilder::new();
    let upstream = UpstreamPipeline::new(executor, request_builder);

    // ── Build JSON-RPC router ─────────────────────────────────────────────────
    //
    // Each instance gets a unique UUID included in every structured log line.
    let instance_id = Uuid::new_v4().to_string();
    let log_level = LogLevel::from_str_or_default(&cfg.log_level);
    let logger = RequestLogger::new(instance_id.clone(), log_level);

    let mcp_router = Arc::new(McpRouter::new(Arc::clone(&cache), upstream, logger));

    // ── Build connection tracker ──────────────────────────────────────────────
    let connection_tracker = ConnectionTracker::new(cfg.gateway_max_connections);

    // ── Build Streamable HTTP transport ───────────────────────────────────────
    let validator = Arc::new(TokenValidator::new());
    let transport_state = TransportState::new(
        Arc::clone(&cache),
        validator,
        mcp_router,
        connection_tracker,
    );
    let mcp_transport = build_transport_router(transport_state);

    let health_state = HealthState {
        service: "mcp-gateway",
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

    let app = Router::new()
        .merge(health_router)
        .merge(api_router)
        .merge(mcp_transport);

    let addr = SocketAddr::from(([0, 0, 0, 0], cfg.port));
    tracing::info!("listening on {}", addr);

    match tokio::net::TcpListener::bind(&addr).await {
        Ok(listener) => {
            if let Err(e) = axum::serve(listener, app).await {
                tracing::error!("server error: {}", e);
                std::process::exit(1);
            }
        }
        Err(e) => {
            tracing::error!("failed to bind to {}: {}", addr, e);
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        routing::get,
        Extension, Router,
    };
    use mcp_common::health::{live_handler, ready_handler, DbCheckerFn, HealthState};
    use std::sync::Arc;
    use tower::ServiceExt;

    fn make_test_router(db_checker: DbCheckerFn) -> Router {
        let state = HealthState {
            service: "mcp-gateway",
            version: "0.0.0",
            db_checker,
        };
        Router::new()
            .route("/health/live", get(live_handler))
            .route("/health/ready", get(ready_handler))
            .layer(Extension(state))
    }

    #[tokio::test]
    async fn health_ready_returns_200_when_db_healthy() {
        let checker: DbCheckerFn = Arc::new(|| Box::pin(async { Ok(()) }));
        let app = make_test_router(checker);
        let req = Request::builder()
            .uri("/health/ready")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn health_ready_returns_503_when_db_unhealthy() {
        let checker: DbCheckerFn =
            Arc::new(|| Box::pin(async { Err("connection refused".to_string()) }));
        let app = make_test_router(checker);
        let req = Request::builder()
            .uri("/health/ready")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
