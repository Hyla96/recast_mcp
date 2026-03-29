//! Health check handlers for liveness and readiness endpoints.
//!
//! Provides shared handler functions and types for `GET /health/live` and
//! `GET /health/ready` that all three services wire up on a router that
//! intentionally excludes the `TraceLayer` (no OTEL span per health request).
//!
//! # Usage
//!
//! ```rust,no_run
//! use axum::{routing::get, Extension, Router};
//! use mcp_common::health::{live_handler, pg_pool_checker, ready_handler, HealthState};
//!
//! # async fn example() {
//! let pool = sqlx::PgPool::connect_lazy("postgres://localhost/db").unwrap();
//! let state = HealthState {
//!     service: "my-service",
//!     version: env!("CARGO_PKG_VERSION"),
//!     db_checker: pg_pool_checker(pool),
//! };
//! let health_router: Router = Router::new()
//!     .route("/health/live", get(live_handler))
//!     .route("/health/ready", get(ready_handler))
//!     .layer(Extension(state));
//! # }
//! ```

use axum::{extract::Extension, http::StatusCode, response::IntoResponse, Json};
use serde::Serialize;
use std::{collections::HashMap, future::Future, pin::Pin, sync::Arc};
use tokio::time::{timeout, Duration};

/// Future returned by a [`DbCheckerFn`].
///
/// Resolves to `Ok(())` when the database is reachable, or `Err(message)` when it is not.
pub type DbCheckFuture = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;

/// Injectable database health checker.
///
/// In production, construct one via [`pg_pool_checker`].
/// In tests, inject an `Arc<|| Box::pin(async { Ok(()) })>` or `Arc<|| Box::pin(async { Err(...) })>`.
pub type DbCheckerFn = Arc<dyn Fn() -> DbCheckFuture + Send + Sync>;

/// Result of a single dependency health check.
#[derive(Debug, Serialize)]
pub struct CheckResult {
    /// `"ok"` when the dependency is reachable; `"error"` otherwise.
    pub status: &'static str,
    /// Human-readable error message when `status` is `"error"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Response body for `GET /health/live`.
#[derive(Debug, Serialize)]
pub struct LivenessResponse {
    /// Always `"ok"`.
    pub status: &'static str,
    /// Service name, e.g. `"mcp-gateway"`.
    pub service: String,
    /// Crate version, e.g. `"0.1.0"`.
    pub version: String,
}

/// Response body for `GET /health/ready`.
#[derive(Debug, Serialize)]
pub struct ReadinessResponse {
    /// `"ok"` if all dependency checks pass; `"degraded"` if any fail.
    pub status: &'static str,
    /// Per-dependency check results keyed by dependency name (e.g. `"database"`).
    pub checks: HashMap<String, CheckResult>,
}

/// Shared state injected into health check handlers via [`axum::Extension`].
///
/// Must be `Clone` so axum can clone it once per request.
#[derive(Clone)]
pub struct HealthState {
    /// Service name, e.g. `"mcp-gateway"`. Should be a `'static` string literal.
    pub service: &'static str,
    /// Crate version, typically `env!("CARGO_PKG_VERSION")`.
    pub version: &'static str,
    /// Injectable database health checker. Use [`pg_pool_checker`] in production.
    pub db_checker: DbCheckerFn,
}

/// Handler for `GET /health/live`.
///
/// Returns HTTP 200 immediately — signals that the process is alive and the
/// HTTP server is accepting connections. Does not check any dependencies.
pub async fn live_handler(Extension(state): Extension<HealthState>) -> impl IntoResponse {
    Json(LivenessResponse {
        status: "ok",
        service: state.service.to_string(),
        version: state.version.to_string(),
    })
}

/// Handler for `GET /health/ready`.
///
/// Checks all dependencies (currently PostgreSQL) with a 500 ms timeout each.
/// Returns HTTP 200 when all checks pass; HTTP 503 when any check fails.
/// The response body contains per-check results under a `"checks"` key.
pub async fn ready_handler(Extension(state): Extension<HealthState>) -> impl IntoResponse {
    let db_result = timeout(Duration::from_millis(500), (state.db_checker)()).await;

    let (db_status, db_message) = match db_result {
        Ok(Ok(())) => ("ok", None),
        Ok(Err(e)) => ("error", Some(e)),
        Err(_) => ("error", Some("timed out after 500ms".to_string())),
    };

    let mut checks = HashMap::new();
    checks.insert(
        "database".to_string(),
        CheckResult {
            status: db_status,
            message: db_message,
        },
    );

    let overall_status = if db_status == "ok" { "ok" } else { "degraded" };
    let http_status = if overall_status == "ok" {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    (
        http_status,
        Json(ReadinessResponse {
            status: overall_status,
            checks,
        }),
    )
}

/// Construct a [`DbCheckerFn`] that acquires a connection from the given `PgPool`.
///
/// Returns `Ok(())` when the pool successfully acquires a connection within the caller's
/// timeout, or `Err(message)` if the pool is exhausted or the database is unreachable.
pub fn pg_pool_checker(pool: sqlx::PgPool) -> DbCheckerFn {
    Arc::new(move || {
        let pool = pool.clone();
        Box::pin(async move {
            pool.acquire()
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        routing::get,
        Extension, Router,
    };
    use tower::ServiceExt;

    fn healthy_router() -> Router {
        let state = HealthState {
            service: "test-svc",
            version: "0.0.0",
            db_checker: Arc::new(|| Box::pin(async { Ok(()) })),
        };
        Router::new()
            .route("/health/live", get(live_handler))
            .route("/health/ready", get(ready_handler))
            .layer(Extension(state))
    }

    fn unhealthy_router() -> Router {
        let state = HealthState {
            service: "test-svc",
            version: "0.0.0",
            db_checker: Arc::new(|| {
                Box::pin(async { Err("connection refused".to_string()) })
            }),
        };
        Router::new()
            .route("/health/live", get(live_handler))
            .route("/health/ready", get(ready_handler))
            .layer(Extension(state))
    }

    #[tokio::test]
    async fn live_returns_200_with_json_body() {
        let app = healthy_router();
        let req = Request::builder()
            .uri("/health/live")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn ready_returns_200_when_db_healthy() {
        let app = healthy_router();
        let req = Request::builder()
            .uri("/health/ready")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn ready_returns_503_when_db_unhealthy() {
        let app = unhealthy_router();
        let req = Request::builder()
            .uri("/health/ready")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
