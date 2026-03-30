//! Shared axum middleware for all services.

use axum::{extract::Request, http::header, middleware::Next, response::{IntoResponse, Response}};
use metrics_exporter_prometheus::PrometheusHandle;
use ulid::Ulid;

/// A per-request identifier stored as a ULID string.
///
/// Inserted into request extensions by [`request_id_middleware`] so that
/// route handlers can extract it via `Extension<RequestId>` if they need to
/// embed it in non-error response bodies.
#[derive(Clone, Debug)]
pub struct RequestId(pub String);

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Axum middleware that generates a ULID per request.
///
/// The generated ULID is:
/// - Stored in the request's [`axum::extract::Extension`] as [`RequestId`] so
///   handlers can access it.
/// - Written to the `X-Request-ID` response header for **successful responses**
///   (i.e. those that do not already carry an `X-Request-ID` header).
///
/// Error responses produced by [`crate::AppError::into_response`] generate
/// their own ULID and set `X-Request-ID` directly, so the middleware skips
/// overwriting the header in that case — ensuring the header and body
/// `request_id` field always match.
///
/// # Usage
///
/// ```rust,ignore
/// use axum::{middleware, Router};
/// use mcp_common::middleware::request_id_middleware;
///
/// let app = Router::new()
///     /* routes */
///     .layer(middleware::from_fn(request_id_middleware));
/// ```
pub async fn request_id_middleware(mut req: Request, next: Next) -> Response {
    let request_id = Ulid::new().to_string();
    req.extensions_mut().insert(RequestId(request_id.clone()));

    let mut response = next.run(req).await;

    // Only inject the header when the response does not already carry one.
    // AppError::into_response() sets its own X-Request-ID so it is skipped here,
    // guaranteeing that the header and the body request_id field always match.
    if !response.headers().contains_key("x-request-id") {
        if let Ok(value) = request_id.parse() {
            response.headers_mut().insert("x-request-id", value);
        }
    }

    response
}

// ── Prometheus metrics middleware ─────────────────────────────────────────────

/// Axum `from_fn` middleware that records `http_requests_total` and
/// `http_request_duration_seconds` Prometheus metrics for every request.
///
/// Uses [`axum::extract::MatchedPath`] for the `path` label to avoid
/// unbounded label cardinality. Requests that hit no registered route (404s)
/// use the static label value `"unmatched"`.
pub async fn track_metrics(req: Request, next: Next) -> Response {
    let method = req.method().to_string();
    // Use the route template (e.g. `/v1/servers/:id`) rather than the raw URI
    // so that path parameters (UUIDs, slugs, etc.) don't create unbounded
    // Prometheus time series.
    let path = req
        .extensions()
        .get::<axum::extract::MatchedPath>()
        .map(|mp| mp.as_str().to_owned())
        .unwrap_or_else(|| "unmatched".to_owned());
    let start = std::time::Instant::now();

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

// ── Prometheus scrape endpoint ────────────────────────────────────────────────

/// Handler for `GET /metrics` — returns Prometheus-format metrics.
///
/// Mount this route **outside** the health router and add
/// `Extension(prom_handle)` to the router's layer stack.
pub async fn metrics_handler(
    axum::Extension(handle): axum::Extension<PrometheusHandle>,
) -> impl axum::response::IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")],
        handle.render(),
    )
}

// ── 404 fallback ──────────────────────────────────────────────────────────────

/// Fallback handler — returns `404 Not Found` with the standard JSON error body
/// for any route that does not match a registered path.
pub async fn fallback_handler() -> impl axum::response::IntoResponse {
    crate::AppError::NotFound("the requested resource does not exist".to_string())
        .into_response()
}
