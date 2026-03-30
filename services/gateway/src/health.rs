//! Gateway-specific health check and metrics endpoints.
//!
//! Provides `/healthz/live`, `/healthz/ready`, and `/metrics` handlers tailored
//! to the gateway's multi-instance design. Unlike the shared `mcp_common` health
//! handlers, these endpoints include `instance_id` and check all three gateway
//! readiness conditions:
//!
//! 1. Config cache initial load complete (`cache.is_loaded()`).
//! 2. PostgreSQL LISTEN connection established (`listen_connected` flag).
//! 3. Sidecar socket pool has ≥ 1 healthy connection (`sidecar_pool.is_healthy()`).
//!
//! # Metrics security
//!
//! `GET /metrics` checks the `Authorization: Bearer <token>` header against the
//! static `METRICS_TOKEN` env var. If `METRICS_TOKEN` is unset or empty, the
//! endpoint is accessible without authentication (development mode only).

use axum::{
    extract::Extension,
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use metrics_exporter_prometheus::PrometheusHandle;
use serde::Serialize;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use crate::cache::ConfigCache;
use crate::sidecar::SidecarPool;

// ── State ─────────────────────────────────────────────────────────────────────

/// Shared state for the gateway health and metrics handlers.
///
/// Construct once at startup and inject via `Extension`.
#[derive(Clone)]
pub struct GatewayHealthState {
    /// Unique identifier for this gateway instance (UUIDv4 generated at startup).
    pub instance_id: String,
    /// In-memory config cache — used to read `is_loaded()` and `entry_count()`.
    pub cache: Arc<ConfigCache>,
    /// `true` while the PostgreSQL LISTEN subscription is active.
    pub listen_connected: Arc<AtomicBool>,
    /// Sidecar socket pool — probed for health with a 200 ms timeout.
    pub sidecar_pool: Arc<SidecarPool>,
    /// Optional static token required in `Authorization: Bearer` header for
    /// `GET /metrics`. `None` means the endpoint is open (dev mode).
    pub metrics_token: Option<String>,
}

// ── Response types ─────────────────────────────────────────────────────────────

/// Response body for `GET /healthz/live`.
#[derive(Debug, Serialize)]
pub struct GatewayLiveResponse {
    /// Always `"ok"`.
    pub status: &'static str,
    /// This instance's UUIDv4 identifier.
    pub instance_id: String,
}

/// Response body for `GET /healthz/ready` when all checks pass.
#[derive(Debug, Serialize)]
pub struct GatewayReadyResponse {
    /// `"ready"` when all checks pass.
    pub status: &'static str,
    /// This instance's UUIDv4 identifier.
    pub instance_id: String,
    /// Number of server configs currently held in the in-memory cache.
    pub cache_entries: u64,
}

/// Response body for `GET /healthz/ready` when any check fails.
#[derive(Debug, Serialize)]
pub struct GatewayNotReadyResponse {
    /// Always `"not_ready"`.
    pub status: &'static str,
    /// This instance's UUIDv4 identifier.
    pub instance_id: String,
    /// Human-readable explanation of which check failed.
    pub reason: String,
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// Handler for `GET /healthz/live`.
///
/// Returns HTTP 200 immediately — signals that the process is alive and the HTTP
/// server is accepting requests. Does not check any dependencies.
/// Does **not** produce an OTEL span (route must be mounted outside `TraceLayer`).
pub async fn gateway_live_handler(
    Extension(state): Extension<GatewayHealthState>,
) -> impl IntoResponse {
    Json(GatewayLiveResponse {
        status: "ok",
        instance_id: state.instance_id,
    })
}

/// Handler for `GET /healthz/ready`.
///
/// Checks all three gateway readiness conditions (cache loaded, LISTEN connected,
/// sidecar healthy). Returns HTTP 200 on success; HTTP 503 on any failure.
/// The response always includes `instance_id`.
///
/// Does **not** produce an OTEL span (route must be mounted outside `TraceLayer`).
pub async fn gateway_ready_handler(
    Extension(state): Extension<GatewayHealthState>,
) -> impl IntoResponse {
    // (a) Config cache initial load complete.
    if !state.cache.is_loaded() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "status": "not_ready",
                "instance_id": state.instance_id,
                "reason": "config cache not yet loaded"
            })),
        );
    }

    // (b) PostgreSQL LISTEN connection established.
    if !state.listen_connected.load(Ordering::Acquire) {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "status": "not_ready",
                "instance_id": state.instance_id,
                "reason": "PostgreSQL LISTEN connection not established"
            })),
        );
    }

    // (c) Sidecar socket pool has ≥ 1 healthy connection.
    if !state.sidecar_pool.is_healthy().await {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "status": "not_ready",
                "instance_id": state.instance_id,
                "reason": "sidecar socket unreachable"
            })),
        );
    }

    let cache_entries = state.cache.entry_count();
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "status": "ready",
            "instance_id": state.instance_id,
            "cache_entries": cache_entries
        })),
    )
}

/// Handler for `GET /metrics`.
///
/// Returns Prometheus-format metrics. If `METRICS_TOKEN` is configured, the
/// request must carry a matching `Authorization: Bearer <token>` header or the
/// handler returns HTTP 401.
///
/// The handler uses axum `State` for the `PrometheusHandle` so it can be mounted
/// on a router that already has `Extension(GatewayHealthState)`.
pub async fn gateway_metrics_handler(
    headers: HeaderMap,
    Extension(state): Extension<GatewayHealthState>,
    Extension(handle): Extension<PrometheusHandle>,
) -> impl IntoResponse {
    // Token check — only when a token is configured.
    if let Some(expected) = &state.metrics_token {
        let provided = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "));

        match provided {
            Some(token) if token == expected => {}
            _ => {
                return (
                    StatusCode::UNAUTHORIZED,
                    [(header::WWW_AUTHENTICATE, "Bearer realm=\"metrics\"")],
                    "Unauthorized",
                )
                    .into_response();
            }
        }
    }

    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        handle.render(),
    )
        .into_response()
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo
)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        routing::get,
        Extension, Router,
    };
    use std::path::PathBuf;
    use tower::ServiceExt;

    /// Build a `GatewayHealthState` for testing.
    ///
    /// - `cache_loaded`: if `true`, calls `mark_loaded_for_testing()` so
    ///   `cache.is_loaded()` returns `true` without a real DB.
    /// - `listen_connected`: seeds the LISTEN flag.
    /// - `metrics_token`: optional static token for the `/metrics` endpoint.
    fn make_state(
        cache_loaded: bool,
        listen_connected: bool,
        metrics_token: Option<String>,
    ) -> GatewayHealthState {
        let pool = sqlx::PgPool::connect_lazy("postgres://localhost/test")
            .expect("lazy pool");
        let cache = Arc::new(crate::cache::ConfigCache::new(pool));
        if cache_loaded {
            cache.mark_loaded_for_testing();
        }
        let listen_connected_flag = Arc::new(AtomicBool::new(listen_connected));
        // Use a non-existent socket path so the sidecar health check always fails.
        let sidecar_pool = SidecarPool::new(PathBuf::from("/tmp/non-existent-test.sock"));

        GatewayHealthState {
            instance_id: "test-instance-id".to_string(),
            cache,
            listen_connected: listen_connected_flag,
            sidecar_pool,
            metrics_token,
        }
    }

    fn make_router(state: GatewayHealthState) -> Router {
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        Router::new()
            .route("/healthz/live", get(gateway_live_handler))
            .route("/healthz/ready", get(gateway_ready_handler))
            .route("/metrics", get(gateway_metrics_handler))
            .layer(Extension(state))
            .layer(Extension(handle))
    }

    // ── /healthz/live ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn live_returns_200() {
        let app = make_router(make_state(false, false, None));
        let res = app
            .oneshot(Request::get("/healthz/live").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn live_response_contains_instance_id() {
        let app = make_router(make_state(false, false, None));
        let res = app
            .oneshot(Request::get("/healthz/live").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = axum::body::to_bytes(res.into_body(), 4096).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "ok");
        assert_eq!(json["instance_id"], "test-instance-id");
    }

    // ── /healthz/ready — cache not loaded ─────────────────────────────────────

    #[tokio::test]
    async fn ready_returns_503_when_cache_not_loaded() {
        let app = make_router(make_state(false, true, None));
        let res = app
            .oneshot(Request::get("/healthz/ready").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = axum::body::to_bytes(res.into_body(), 4096).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "not_ready");
        assert!(
            json["reason"].as_str().unwrap().contains("cache"),
            "expected reason to mention 'cache', got: {}",
            json["reason"]
        );
    }

    // ── /healthz/ready — listen not connected ─────────────────────────────────

    #[tokio::test]
    async fn ready_returns_503_when_listen_not_connected() {
        // cache_loaded=true but listen_connected=false.
        let app = make_router(make_state(true, false, None));
        let res = app
            .oneshot(Request::get("/healthz/ready").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = axum::body::to_bytes(res.into_body(), 4096).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "not_ready");
        assert!(
            json["reason"].as_str().unwrap().contains("LISTEN"),
            "expected reason to mention 'LISTEN', got: {}",
            json["reason"]
        );
    }

    // ── /healthz/ready — sidecar unhealthy ────────────────────────────────────

    #[tokio::test]
    async fn ready_returns_503_when_sidecar_unreachable() {
        // cache_loaded=true, listen_connected=true, sidecar at non-existent path.
        let app = make_router(make_state(true, true, None));
        let res = app
            .oneshot(Request::get("/healthz/ready").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = axum::body::to_bytes(res.into_body(), 4096).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "not_ready");
        assert!(
            json["reason"].as_str().unwrap().contains("sidecar"),
            "expected reason to mention 'sidecar', got: {}",
            json["reason"]
        );
    }

    // ── /healthz/ready — instance_id in 503 ──────────────────────────────────

    #[tokio::test]
    async fn ready_503_includes_instance_id() {
        let app = make_router(make_state(false, false, None));
        let res = app
            .oneshot(Request::get("/healthz/ready").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = axum::body::to_bytes(res.into_body(), 4096).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["instance_id"], "test-instance-id");
    }

    // ── /metrics — no token configured ────────────────────────────────────────

    #[tokio::test]
    async fn metrics_accessible_without_token_when_none_configured() {
        let app = make_router(make_state(false, false, None));
        let res = app
            .oneshot(Request::get("/metrics").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    // ── /metrics — correct token ───────────────────────────────────────────────

    #[tokio::test]
    async fn metrics_accessible_with_correct_token() {
        let app = make_router(make_state(false, false, Some("secret-token".to_string())));
        let res = app
            .oneshot(
                Request::get("/metrics")
                    .header("Authorization", "Bearer secret-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    // ── /metrics — wrong token ─────────────────────────────────────────────────

    #[tokio::test]
    async fn metrics_returns_401_with_wrong_token() {
        let app = make_router(make_state(false, false, Some("secret-token".to_string())));
        let res = app
            .oneshot(
                Request::get("/metrics")
                    .header("Authorization", "Bearer wrong-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    // ── /metrics — missing token ───────────────────────────────────────────────

    #[tokio::test]
    async fn metrics_returns_401_with_missing_token() {
        let app = make_router(make_state(false, false, Some("secret-token".to_string())));
        let res = app
            .oneshot(Request::get("/metrics").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    // ── /metrics — content type ────────────────────────────────────────────────

    #[tokio::test]
    async fn metrics_content_type_is_prometheus_text() {
        let app = make_router(make_state(false, false, None));
        let res = app
            .oneshot(Request::get("/metrics").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let ct = res
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(ct.contains("text/plain"), "expected text/plain, got {ct}");
    }

    // ── /metrics — www-authenticate header on 401 ──────────────────────────────

    #[tokio::test]
    async fn metrics_401_includes_www_authenticate_header() {
        let app = make_router(make_state(false, false, Some("s3cr3t".to_string())));
        let res = app
            .oneshot(Request::get("/metrics").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        let www_auth = res
            .headers()
            .get("www-authenticate")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(www_auth.contains("Bearer"), "expected WWW-Authenticate: Bearer, got {www_auth}");
    }
}
