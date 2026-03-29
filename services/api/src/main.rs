//! Platform API service.

mod config;

use axum::{
    http::header,
    response::IntoResponse,
    routing::get,
    Extension, Router,
};
use config::Config;
use mcp_common::{
    health::{live_handler, pg_pool_checker, ready_handler, HealthState},
    init_telemetry, FromEnv,
};
use metrics_exporter_prometheus::PrometheusHandle;
use std::{net::SocketAddr, time::Instant};
use tower_http::trace::TraceLayer;

/// Axum middleware function that records `http_requests_total` and
/// `http_request_duration_seconds` Prometheus metrics for every request.
async fn track_metrics(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let method = req.method().to_string();
    let path = req.uri().path().to_string();
    let start = Instant::now();

    let response = next.run(req).await;

    let duration = start.elapsed().as_secs_f64();
    let status = response.status().as_u16().to_string();

    metrics::counter!(
        "http_requests_total",
        "method" => method.clone(),
        "status" => status,
        "path" => path.clone()
    )
    .increment(1);

    metrics::histogram!(
        "http_request_duration_seconds",
        "method" => method,
        "path" => path
    )
    .record(duration);

    response
}

/// Handler for `GET /metrics` — returns Prometheus-format metrics.
async fn metrics_handler(
    Extension(handle): Extension<PrometheusHandle>,
) -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")],
        handle.render(),
    )
}

#[tokio::main]
async fn main() {
    // Validate configuration before initializing any subsystems.
    // Fail immediately with all missing/malformed variables listed.
    let cfg = match Config::from_env() {
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

    // Create a lazy pool — connects on first use, so startup is non-blocking.
    // /health/ready will return 503 until the DB is reachable.
    let db_pool = match sqlx::PgPool::connect_lazy(&cfg.database_url) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("failed to create database pool: {e}");
            std::process::exit(1);
        }
    };

    tracing::info!(port = cfg.port, database_url = cfg.database_url, "starting platform api");

    let health_state = HealthState {
        service: "mcp-api",
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

    let app = Router::new().merge(health_router).merge(api_router);

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
            service: "mcp-api",
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
