//! Router assembly for the Platform API.
//!
//! Owns CORS configuration, the Prometheus metrics middleware and endpoint,
//! the 404 fallback handler, and the top-level [`build_router`] / internal
//! [`build_router_with_timeout`] functions.

use crate::app_state::AppState;
use crate::auth::clerk_jwt_middleware;
use crate::handlers::credentials::{
    create_credential_handler, delete_credential_handler, list_credentials_handler,
    rotate_credential_handler,
};
use crate::handlers::proxy::proxy_test_handler;
use crate::handlers::servers::{
    create_server_handler, delete_server_handler, get_server_handler, list_servers_handler,
    update_server_handler, validate_url_handler,
};
use crate::handlers::tokens::{create_token_handler, list_tokens_handler, revoke_token_handler};
use crate::handlers::users::me_handler;
use crate::handlers::webhooks::clerk_webhook_handler;
use crate::middleware::{panic_handler, rate_limit_stub, structured_logger};

use axum::{routing::get, Extension, Router};
use mcp_common::{
    fallback_handler, metrics_handler, track_metrics,
    health::{live_handler, pg_pool_checker, ready_handler, HealthState},
    middleware::request_id_middleware,
};
use metrics_exporter_prometheus::PrometheusHandle;
use std::time::Duration;
use tower_http::{
    catch_panic::CatchPanicLayer,
    compression::CompressionLayer,
    cors::CorsLayer,
    timeout::TimeoutLayer,
    trace::TraceLayer,
};

// ── CORS configuration ────────────────────────────────────────────────────────

/// Builds a [`CorsLayer`] from the list of allowed origins.
///
/// An empty `origins` slice means **permissive** (all origins, all methods,
/// all headers). A non-empty slice restricts to the listed origins.
pub(crate) fn build_cors(origins: &[String]) -> CorsLayer {
    if origins.is_empty() {
        return CorsLayer::permissive();
    }

    let allowed: Vec<axum::http::HeaderValue> = origins
        .iter()
        .filter_map(|o| o.parse().ok())
        .collect();

    if allowed.is_empty() {
        // All origins failed to parse — fall back to permissive.
        CorsLayer::permissive()
    } else {
        CorsLayer::new()
            .allow_origin(allowed)
            .allow_methods(tower_http::cors::Any)
            .allow_headers(tower_http::cors::Any)
    }
}

// ── Router assembly ───────────────────────────────────────────────────────────

/// Assembles the complete axum [`Router`] from state, middleware, and routes.
///
/// The middleware stack applied to all routes (outermost → innermost):
///
/// ```text
/// CatchPanic → RequestId → TraceLayer → StructuredLogger → MetricsLayer
/// → CorsLayer → CompressionLayer → TimeoutLayer(30s) → RateLimit(stub)
/// ```
///
/// JWT authentication (`clerk_jwt_middleware`) is applied as a `route_layer`
/// on the `/v1/*` sub-router only. This allows `/v1/webhooks/clerk` (TASK-017)
/// to be added later without auth, while all other `/v1/*` routes are protected.
///
/// Health routes (`/health/live`, `/health/ready`) are intentionally excluded
/// from this stack so they produce no OTEL spans and do not skew request metrics.
pub fn build_router(state: AppState, prom_handle: PrometheusHandle) -> Router {
    build_router_with_timeout(state, prom_handle, Duration::from_secs(30))
}

/// Internal router builder that accepts a configurable timeout — used in tests
/// to set a short timeout without modifying production configuration.
pub(crate) fn build_router_with_timeout(
    state: AppState,
    prom_handle: PrometheusHandle,
    request_timeout: Duration,
) -> Router {
    let cors = build_cors(&state.config.cors_origins);

    let health_state = HealthState {
        service: "mcp-api",
        version: env!("CARGO_PKG_VERSION"),
        db_checker: pg_pool_checker(state.pool.clone()),
    };

    // Health routes are outside the full middleware stack — no OTEL spans,
    // no metrics skew, and they respond even if the app is shutting down.
    let health_router = Router::new()
        .route("/health/live", get(live_handler))
        .route("/health/ready", get(ready_handler))
        .layer(Extension(health_state));

    // Unprotected v1 routes — no JWT auth middleware.
    // /v1/webhooks/clerk uses its own Svix signature verification.
    let v1_public = Router::new()
        .route("/v1/webhooks/clerk", axum::routing::post(clerk_webhook_handler));

    // Protected v1 routes — JWT auth enforced via route_layer.
    // route_layer applies only to the routes in THIS sub-router, so
    // /v1/webhooks/clerk (above) is never touched by clerk_jwt_middleware.
    let v1_protected = Router::new()
        .route("/v1/proxy/test", axum::routing::post(proxy_test_handler))
        .route("/v1/users/me", get(me_handler))
        // Server CRUD endpoints
        .route(
            "/v1/servers",
            get(list_servers_handler).post(create_server_handler),
        )
        .route(
            "/v1/servers/{id}",
            get(get_server_handler)
                .put(update_server_handler)
                .delete(delete_server_handler),
        )
        .route(
            "/v1/servers/{id}/validate-url",
            axum::routing::post(validate_url_handler),
        )
        // Credential endpoints
        .route(
            "/v1/servers/{server_id}/credentials",
            get(list_credentials_handler).post(create_credential_handler),
        )
        .route(
            "/v1/servers/{server_id}/credentials/{id}",
            axum::routing::put(rotate_credential_handler)
                .delete(delete_credential_handler),
        )
        // Server token endpoints
        .route(
            "/v1/servers/{server_id}/tokens",
            get(list_tokens_handler).post(create_token_handler),
        )
        .route(
            "/v1/servers/{server_id}/tokens/{id}",
            axum::routing::delete(revoke_token_handler),
        )
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            clerk_jwt_middleware,
        ));

    // API routes with the full middleware stack.
    // The LAST .layer() call is the OUTERMOST (first to process a request).
    let api_router = Router::new()
        .route("/metrics", get(metrics_handler))
        .merge(v1_public)
        .merge(v1_protected)
        .fallback(fallback_handler)
        .with_state(state)
        .layer(axum::middleware::from_fn(rate_limit_stub)) // stub — replaced in TASK-022
        .layer(TimeoutLayer::new(request_timeout))
        .layer(CompressionLayer::new())
        .layer(cors)
        .layer(axum::middleware::from_fn(track_metrics))
        .layer(axum::middleware::from_fn(structured_logger))
        .layer(TraceLayer::new_for_http())
        .layer(axum::middleware::from_fn(request_id_middleware))
        .layer(CatchPanicLayer::custom(panic_handler))
        .layer(Extension(prom_handle));

    Router::new().merge(health_router).merge(api_router)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
        routing::get,
        Router,
    };
    use mcp_common::health::{DbCheckerFn, HealthState};
    use tower::ServiceExt;

    // ── Helper: minimal health router (no DB) ──────────────────────────────────

    fn make_health_router(db_checker: DbCheckerFn) -> Router {
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

    // ── Health endpoint tests ──────────────────────────────────────────────────

    #[tokio::test]
    async fn health_live_returns_200() {
        let checker: DbCheckerFn = std::sync::Arc::new(|| Box::pin(async { Ok(()) }));
        let app = make_health_router(checker);
        let req = Request::builder()
            .uri("/health/live")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn health_ready_returns_200_when_db_healthy() {
        let checker: DbCheckerFn = std::sync::Arc::new(|| Box::pin(async { Ok(()) }));
        let app = make_health_router(checker);
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
            std::sync::Arc::new(|| Box::pin(async { Err("connection refused".to_string()) }));
        let app = make_health_router(checker);
        let req = Request::builder()
            .uri("/health/ready")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    // ── Helper: lightweight API router for middleware tests ────────────────────
    //
    // Builds the full middleware stack around a minimal set of routes so tests
    // can exercise middleware behaviour without a real database connection.
    // Uses a short 200 ms timeout so the timeout test completes quickly.

    fn make_test_api_router() -> Router {
        Router::new()
            .route("/ok", get(|| async { (StatusCode::OK, "ok") }))
            .route(
                "/panic",
                get(|| async {
                    panic!("intentional test panic");
                    // Unreachable — provides a concrete IntoResponse type for the async block.
                    #[allow(unreachable_code)]
                    ""
                }),
            )
            .route(
                "/slow",
                get(|| async {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    "never"
                }),
            )
            .fallback(fallback_handler)
            // Auth is applied per-route via route_layer, not globally.
            // This test router exercises the global middleware stack only.
            .layer(axum::middleware::from_fn(rate_limit_stub))
            .layer(TimeoutLayer::new(Duration::from_millis(200)))
            .layer(CompressionLayer::new())
            .layer(CorsLayer::permissive())
            .layer(axum::middleware::from_fn(structured_logger))
            .layer(TraceLayer::new_for_http())
            .layer(axum::middleware::from_fn(request_id_middleware))
            .layer(CatchPanicLayer::custom(panic_handler))
    }

    // ── Fallback 404 test ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn fallback_returns_404_with_json_body() {
        let app = make_test_api_router();
        let req = Request::builder()
            .uri("/")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);

        let body_bytes = to_bytes(res.into_body(), 8192).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert!(json.get("error").is_some(), "body must have an 'error' key");
        assert_eq!(json["error"]["code"], "not_found");
    }

    // ── CatchPanicLayer test ───────────────────────────────────────────────────

    #[tokio::test]
    async fn catch_panic_returns_500_json() {
        let app = make_test_api_router();
        let req = Request::builder()
            .uri("/panic")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let ct = res.headers().get("content-type").unwrap();
        assert!(ct.to_str().unwrap().contains("application/json"));

        let body_bytes = to_bytes(res.into_body(), 8192).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert!(json.get("error").is_some(), "body must have an 'error' key");
        assert_eq!(json["error"]["code"], "internal_server_error");
        assert!(json["error"].get("request_id").is_some());
    }

    // ── X-Request-ID header test ───────────────────────────────────────────────

    #[tokio::test]
    async fn request_id_header_present_on_success() {
        let app = make_test_api_router();
        let req = Request::builder()
            .uri("/ok")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert!(
            res.headers().contains_key("x-request-id"),
            "X-Request-ID header must be present"
        );
    }

    #[tokio::test]
    async fn request_id_header_present_on_404() {
        let app = make_test_api_router();
        let req = Request::builder()
            .uri("/does-not-exist")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        // AppError::into_response sets X-Request-ID on all error responses.
        assert!(
            res.headers().contains_key("x-request-id"),
            "X-Request-ID header must be present on 404"
        );
    }

    // ── Timeout test ───────────────────────────────────────────────────────────
    //
    // tower_http::timeout::TimeoutLayer returns 408 Request Timeout with an
    // empty body when the inner service does not respond in time.

    #[tokio::test]
    async fn timeout_returns_408_for_slow_handler() {
        // The test router sets a 200 ms timeout; /slow sleeps 60 s.
        let app = make_test_api_router();
        let req = Request::builder()
            .uri("/slow")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::REQUEST_TIMEOUT);
    }

    // ── CORS test ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn cors_header_present_on_preflight() {
        let app = make_test_api_router();
        let req = Request::builder()
            .method("OPTIONS")
            .uri("/ok")
            .header("origin", "https://example.com")
            .header("access-control-request-method", "GET")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert!(
            res.headers().contains_key("access-control-allow-origin"),
            "CORS allow-origin header must be present"
        );
    }

    // ── Graceful shutdown integration test ────────────────────────────────────
    //
    // Starts a real TCP server, sends a slow request, triggers shutdown via a
    // oneshot channel (simulating SIGTERM), and verifies the in-flight request
    // completes before the server exits.

    #[tokio::test]
    async fn graceful_shutdown_drains_inflight_request() {
        // Build a simple router with a handler that takes 300 ms.
        // No timeout layer — we don't want the 200 ms test timeout to fire.
        let app = Router::new().route(
            "/slow-req",
            get(|| async {
                tokio::time::sleep(Duration::from_millis(300)).await;
                (StatusCode::OK, "done")
            }),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        // Spawn the server.
        let server_task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .await
                .ok();
        });

        // Give the server a moment to start listening.
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Fire a slow request in the background.
        let request_task = tokio::spawn(async move {
            let client = reqwest::Client::new();
            client
                .get(format!("http://{addr}/slow-req"))
                .send()
                .await
                .map(|r| r.status().as_u16())
                .unwrap_or(0)
        });

        // Trigger shutdown after the request is in-flight (but before it completes).
        tokio::time::sleep(Duration::from_millis(80)).await;
        shutdown_tx.send(()).ok();

        // The in-flight request should still complete successfully.
        let status = tokio::time::timeout(Duration::from_secs(5), request_task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(status, 200, "in-flight request must complete before shutdown");

        // Server should stop cleanly.
        tokio::time::timeout(Duration::from_secs(5), server_task)
            .await
            .unwrap()
            .ok();
    }

    // ── build_cors helper tests ────────────────────────────────────────────────

    #[test]
    fn build_cors_empty_origins_is_permissive() {
        // No panic — permissive CORS is returned for an empty slice.
        let _layer = build_cors(&[]);
    }

    #[test]
    fn build_cors_with_valid_origin() {
        let origins = vec!["https://example.com".to_string()];
        let _layer = build_cors(&origins);
    }

    #[test]
    fn build_cors_with_all_invalid_origins_falls_back_to_permissive() {
        let origins = vec!["not a valid origin %%".to_string()];
        // Should not panic; falls back to permissive.
        let _layer = build_cors(&origins);
    }
}
