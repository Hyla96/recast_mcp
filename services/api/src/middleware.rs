//! Platform API-specific middleware layers.
//!
//! This module contains middleware that is particular to the Platform API service.
//! Reusable, service-agnostic middleware lives in [`mcp_common::middleware`].

use axum::{
    extract::Request,
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use mcp_common::middleware::RequestId;

// ── Structured request logger ─────────────────────────────────────────────────

/// Axum `from_fn` middleware that emits one structured log event per request.
///
/// The log event carries: `request_id`, `method`, `path`, `status`, `latency_ms`.
/// With the JSON tracing subscriber configured in `mcp_common::init_telemetry`,
/// this produces a machine-readable JSON line on stdout for every HTTP request.
///
/// The `request_id` is read from the `RequestId` extension installed by
/// [`mcp_common::middleware::request_id_middleware`], which must be placed *outer*
/// to this middleware in the stack.
pub async fn structured_logger(req: Request, next: Next) -> Response {
    let request_id = req
        .extensions()
        .get::<RequestId>()
        .map(|r| r.0.clone())
        .unwrap_or_default();
    let method = req.method().to_string();
    let path = req.uri().path().to_string();
    let start = std::time::Instant::now();

    let response = next.run(req).await;

    let latency_ms = start.elapsed().as_millis();
    let status = response.status().as_u16();

    tracing::info!(
        request_id = %request_id,
        method = %method,
        path = %path,
        status = status,
        latency_ms = latency_ms,
        "request completed"
    );

    response
}

// ── Rate limit stub (replaced in TASK-022) ───────────────────────────────────

/// Placeholder rate-limiting middleware.
///
/// This no-op layer will be replaced with a Redis token-bucket implementation
/// in TASK-022. It currently passes every request through unconditionally.
pub async fn rate_limit_stub(req: Request, next: Next) -> Response {
    next.run(req).await
}

// ── Panic handler ─────────────────────────────────────────────────────────────

/// Handler for [`tower_http::catch_panic::CatchPanicLayer`].
///
/// Returns a JSON `500 Internal Server Error` response instead of the default
/// plain-text response that axum would otherwise produce. A fresh UUID is
/// embedded as `request_id` so the panic can be correlated with other log lines
/// for the same request.
pub fn panic_handler(_err: Box<dyn std::any::Any + Send>) -> Response {
    let request_id = uuid::Uuid::new_v4().to_string();
    let body = serde_json::json!({
        "error": {
            "code": "internal_server_error",
            "message": "An internal error occurred.",
            "request_id": request_id
        }
    })
    .to_string();

    tracing::error!(request_id = %request_id, "request handler panicked");

    (
        StatusCode::INTERNAL_SERVER_ERROR,
        [(header::CONTENT_TYPE, "application/json")],
        body,
    )
        .into_response()
}
