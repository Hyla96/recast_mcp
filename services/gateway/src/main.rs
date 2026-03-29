//! MCP Gateway service.

use axum::{
    http::header,
    response::IntoResponse,
    routing::get,
    Extension, Router,
};
use mcp_common::init_telemetry;
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

    tracing::info!("starting gateway");

    let app = Router::new()
        .route("/health/live", get(|| async { "ok" }))
        .route("/metrics", get(metrics_handler))
        .layer(Extension(prom_handle))
        .layer(TraceLayer::new_for_http())
        .layer(axum::middleware::from_fn(track_metrics));

    let addr = SocketAddr::from(([0, 0, 0, 0], 3000));
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
