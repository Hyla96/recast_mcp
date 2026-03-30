//! MCP Gateway service.

pub mod auth;
pub mod cache;
pub mod circuit_breaker;
mod config;
pub mod connections;
pub mod hot_reload;
pub mod logging;
pub mod protocol;
pub mod router;
pub mod shutdown;
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
use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Instant,
};
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

    let prom_handle = match metrics_exporter_prometheus::PrometheusBuilder::new().install_recorder()
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

    // ── Shutdown flag ─────────────────────────────────────────────────────────
    //
    // Shared between the transport handler (returns 503 + Connection: close
    // when set) and the health readiness probe (returns 503 to signal the LB).
    let is_shutting_down = Arc::new(AtomicBool::new(false));

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
    let sync_task = ConfigSyncTask::new(
        cfg.database_url.clone(),
        db_pool.clone(),
        Arc::clone(&cache),
    );
    // Keep the handle so we can abort the task during graceful shutdown.
    let sync_handle = sync_task.start();

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

    let mcp_router = Arc::new(McpRouter::new(
        Arc::clone(&cache),
        upstream,
        Arc::clone(&logger),
    ));

    // ── Build connection tracker ──────────────────────────────────────────────
    let connection_tracker = ConnectionTracker::new(cfg.gateway_max_connections);

    // ── Build Streamable HTTP transport ───────────────────────────────────────
    let validator = Arc::new(TokenValidator::new());
    let transport_state = TransportState::new(
        Arc::clone(&cache),
        validator,
        mcp_router,
        Arc::clone(&connection_tracker),
        Arc::clone(&is_shutting_down),
    );
    let mcp_transport = build_transport_router(transport_state);

    // Wrap the db checker so `/health/ready` returns 503 during the LB drain
    // window, signalling the load balancer to stop routing new traffic here.
    let ready_db_checker =
        shutdown::make_shutdown_db_checker(pg_pool_checker(db_pool), Arc::clone(&is_shutting_down));

    let health_state = HealthState {
        service: "mcp-gateway",
        version: env!("CARGO_PKG_VERSION"),
        db_checker: ready_db_checker,
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

    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("failed to bind to {}: {}", addr, e);
            std::process::exit(1);
        }
    };

    // ── Graceful shutdown future ──────────────────────────────────────────────
    //
    // When the shutdown future resolves:
    //   (a) is_shutting_down → true: new MCP requests get 503 + Connection:close.
    //   (b) /health/ready → 503: load balancer stops routing new traffic.
    //   (c) 5-second LB drain window elapses.
    //
    // After the future resolves, axum stops accepting new TCP connections and
    // waits for all in-flight handlers to complete before serve().await returns.
    let is_shutting_down_clone = Arc::clone(&is_shutting_down);
    let shutdown_future = async move {
        shutdown::await_shutdown_signal().await;

        let t_signal = Instant::now();
        is_shutting_down_clone.store(true, Ordering::SeqCst);
        tracing::info!(
            phase = "shutdown_initiated",
            "shutdown initiated; rejecting new MCP connections and signalling LB"
        );

        // Phase B: 5-second LB drain window.
        // The load balancer detects the 503 readiness probe and stops routing
        // new traffic to this instance during this window.
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        tracing::info!(
            phase = "lb_drain_complete",
            elapsed_ms = t_signal.elapsed().as_millis() as u64,
            "LB drain window complete; stopping TCP accept loop"
        );
    };

    if let Err(e) = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_future)
        .await
    {
        tracing::error!(error = %e, "server error during shutdown");
        std::process::exit(1);
    }

    // ── Post-serve shutdown sequence (Phases C → exiting) ────────────────────
    //
    // By the time serve().await returns, axum has already drained its HTTP
    // connections. We still call run_shutdown_sequence() to:
    //   (c) ConnectionTracker::drain() — safety check with 30-second timeout.
    //   (f) Abort the hot-reload PgListener task.
    //   (d/e) Flush the async log writer channel.
    shutdown::run_shutdown_sequence(connection_tracker, logger, sync_handle).await;
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
