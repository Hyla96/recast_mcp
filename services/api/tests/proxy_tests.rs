// Platform API — Proxy test endpoint integration tests.
//
// Tests in this file exercise `POST /v1/proxy/test`, covering:
//   - Happy path (2xx upstream response, JSON body)
//   - Auth injection: bearer, api_key (header), api_key (query), basic
//   - SSRF block (private IP in request URL)
//   - Timeout (upstream accepts but never responds within proxy_timeout)
//   - Connectivity error (nothing listening on target port)
//   - Unauthenticated requests (no JWT)
//   - Response body truncation at 100 KB
//
// Required environment variable:
//   TEST_DATABASE_URL=postgres://recast:recast@localhost:5432/postgres \
//     cargo test -p mcp-api --test proxy_tests
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    missing_docs
)]

mod helpers;

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use helpers::{make_jwt, make_state_with_jwks};
use mcp_api::{
    app_state::{AppState, SsrfValidatorFn},
    auth::clerk_jwt_middleware,
    handlers::proxy::proxy_test_handler,
};
use mcp_common::{testing::TestDatabase, validate_url_with_dns};
use std::sync::Arc;
use tokio::net::TcpListener;
use tower::ServiceExt;

// ── Router builder ────────────────────────────────────────────────────────────

/// Minimal router wiring just the proxy test endpoint behind JWT auth.
fn make_proxy_router(state: AppState) -> Router {
    let protected = Router::new()
        .route("/v1/proxy/test", axum::routing::post(proxy_test_handler))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            clerk_jwt_middleware,
        ));

    Router::new().merge(protected).with_state(state)
}

/// Builds a POST /v1/proxy/test request with a Bearer JWT and a JSON body.
fn proxy_request(jwt: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/v1/proxy/test")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {jwt}"))
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap()
}

/// Reads the full response body and deserializes it as JSON.
async fn json_body(res: axum::response::Response) -> serde_json::Value {
    let bytes = to_bytes(res.into_body(), 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).expect("response is not JSON")
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Happy path: upstream returns 200 with a JSON body.
#[tokio::test]
async fn proxy_test_happy_path_json() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let (state, jwks_mock) = make_state_with_jwks(db.pool.clone()).await;

    // Start an upstream mock that returns a JSON response.
    let upstream = mcp_common::testing::MockUpstream::start().await;
    upstream.set_response_body(serde_json::json!({ "user": { "id": 1, "name": "Alice" } }));

    let app = make_proxy_router(state);
    let jwt = make_jwt("user_proxy_01", "proxy@example.com");

    let res = app
        .oneshot(proxy_request(
            &jwt,
            serde_json::json!({
                "url": format!("{}/users/1", upstream.url()),
                "method": "GET",
                "auth": { "type": "none" },
            }),
        ))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    let body = json_body(res).await;
    assert_eq!(body["status"], 200);
    assert!(body["body"].is_object(), "should have JSON body field");
    assert_eq!(body["body"]["user"]["name"], "Alice");

    drop(jwks_mock);
}

/// Auth injection: Bearer token forwarded in Authorization header.
#[tokio::test]
async fn proxy_test_bearer_auth_injected() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let (state, jwks_mock) = make_state_with_jwks(db.pool.clone()).await;

    let upstream = mcp_common::testing::MockUpstream::start().await;
    upstream.set_response_body(serde_json::json!({ "ok": true }));

    let app = make_proxy_router(state);
    let jwt = make_jwt("user_proxy_bearer", "bearer@example.com");

    let res = app
        .oneshot(proxy_request(
            &jwt,
            serde_json::json!({
                "url": upstream.url(),
                "method": "GET",
                "auth": { "type": "bearer", "token": "my-secret-token" },
            }),
        ))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    upstream.assert_received_header("authorization", "Bearer my-secret-token");

    drop(jwks_mock);
}

/// Auth injection: API key sent as a custom header.
#[tokio::test]
async fn proxy_test_api_key_header_injected() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let (state, jwks_mock) = make_state_with_jwks(db.pool.clone()).await;

    let upstream = mcp_common::testing::MockUpstream::start().await;
    upstream.set_response_body(serde_json::json!({ "ok": true }));

    let app = make_proxy_router(state);
    let jwt = make_jwt("user_proxy_apikey_h", "apikey_h@example.com");

    let res = app
        .oneshot(proxy_request(
            &jwt,
            serde_json::json!({
                "url": upstream.url(),
                "method": "GET",
                "auth": {
                    "type": "api_key",
                    "placement": "header",
                    "key_name": "X-Api-Key",
                    "key_value": "super-secret"
                },
            }),
        ))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    upstream.assert_received_header("x-api-key", "super-secret");

    drop(jwks_mock);
}

/// Auth injection: API key appended as a query parameter (after SSRF check).
#[tokio::test]
async fn proxy_test_api_key_query_injected() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let (state, jwks_mock) = make_state_with_jwks(db.pool.clone()).await;

    let upstream = mcp_common::testing::MockUpstream::start().await;
    upstream.set_response_body(serde_json::json!({ "ok": true }));

    let app = make_proxy_router(state);
    let jwt = make_jwt("user_proxy_apikey_q", "apikey_q@example.com");

    let res = app
        .oneshot(proxy_request(
            &jwt,
            serde_json::json!({
                "url": upstream.url(),
                "method": "GET",
                "auth": {
                    "type": "api_key",
                    "placement": "query",
                    "key_name": "api_key",
                    "key_value": "qsecret"
                },
            }),
        ))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    // The api key must appear in the recorded request path query string.
    let requests = upstream.received_requests();
    assert!(!requests.is_empty());
    let recorded_path = &requests.first().unwrap().path;
    assert!(
        recorded_path.contains("api_key=qsecret"),
        "expected api_key in query string, got: {recorded_path}"
    );

    drop(jwks_mock);
}

/// Auth injection: HTTP Basic auth encoded correctly.
#[tokio::test]
async fn proxy_test_basic_auth_injected() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let (state, jwks_mock) = make_state_with_jwks(db.pool.clone()).await;

    let upstream = mcp_common::testing::MockUpstream::start().await;
    upstream.set_response_body(serde_json::json!({ "ok": true }));

    let app = make_proxy_router(state);
    let jwt = make_jwt("user_proxy_basic", "basic@example.com");

    let res = app
        .oneshot(proxy_request(
            &jwt,
            serde_json::json!({
                "url": upstream.url(),
                "method": "GET",
                "auth": {
                    "type": "basic",
                    "username": "alice",
                    "password": "s3cr3t"
                },
            }),
        ))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    // Basic auth is "alice:s3cr3t" base64-encoded → "YWxpY2U6czNjcjN0"
    upstream.assert_received_header("authorization", "Basic YWxpY2U6czNjcjN0");

    drop(jwks_mock);
}

/// SSRF block: private IP in request URL returns 422 ssrf_blocked.
///
/// This test overrides the SSRF validator to the real production one so that
/// the private IP check fires even though other tests use passthrough.
#[tokio::test]
async fn proxy_test_ssrf_blocked_private_ip() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let (mut state, jwks_mock) = make_state_with_jwks(db.pool.clone()).await;

    // Replace the passthrough validator with the real production validator.
    let real_ssrf: SsrfValidatorFn = Arc::new(|url: url::Url| {
        Box::pin(async move { validate_url_with_dns(&url).await })
    });
    state.ssrf_validator = real_ssrf;

    let app = make_proxy_router(state);
    let jwt = make_jwt("user_proxy_ssrf", "ssrf@example.com");

    let res = app
        .oneshot(proxy_request(
            &jwt,
            serde_json::json!({
                "url": "http://192.168.1.1/api",
                "method": "GET",
                "auth": { "type": "none" },
            }),
        ))
        .await
        .unwrap();

    // SSRF block returns 422 via AppError::SsrfBlocked.
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = json_body(res).await;
    assert_eq!(body["error"]["code"], "ssrf_blocked");

    drop(jwks_mock);
}

/// Timeout: upstream accepts TCP connection but never sends a response.
///
/// The test binds a listener, accepts connections, but never writes.
/// The proxy_timeout (150 ms in test state) fires and returns `{ outcome: "timeout" }`.
#[tokio::test]
async fn proxy_test_timeout() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let (state, jwks_mock) = make_state_with_jwks(db.pool.clone()).await;
    // proxy_timeout is 150 ms from make_state_with_jwks.

    // Create a "black hole" listener: accepts connections but never responds.
    let black_hole = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = black_hole.local_addr().unwrap().port();

    // Accept but don't respond — keep the connection open until the test ends.
    tokio::spawn(async move {
        if let Ok((_stream, _)) = black_hole.accept().await {
            // Hold the stream alive for a long time (test will finish first).
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        }
    });

    let app = make_proxy_router(state);
    let jwt = make_jwt("user_proxy_timeout", "timeout@example.com");

    let res = app
        .oneshot(proxy_request(
            &jwt,
            serde_json::json!({
                "url": format!("http://127.0.0.1:{port}/slow"),
                "method": "GET",
                "auth": { "type": "none" },
            }),
        ))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    let body = json_body(res).await;
    assert_eq!(body["outcome"], "timeout");

    drop(jwks_mock);
}

/// Connectivity error: nothing listening on the target port.
#[tokio::test]
async fn proxy_test_connectivity_error() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let (state, jwks_mock) = make_state_with_jwks(db.pool.clone()).await;

    // Bind and immediately drop a listener to get a free port that is not listening.
    let ephemeral = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dead_port = ephemeral.local_addr().unwrap().port();
    drop(ephemeral);
    // Nothing is listening on `dead_port` now.

    let app = make_proxy_router(state);
    let jwt = make_jwt("user_proxy_conn_err", "conn_err@example.com");

    let res = app
        .oneshot(proxy_request(
            &jwt,
            serde_json::json!({
                "url": format!("http://127.0.0.1:{dead_port}/data"),
                "method": "GET",
                "auth": { "type": "none" },
            }),
        ))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    let body = json_body(res).await;
    assert_eq!(body["outcome"], "connectivity_error");
    assert!(body["host"].as_str().is_some(), "host field must be present");

    drop(jwks_mock);
}

/// Unauthenticated request (no JWT) returns 401.
#[tokio::test]
async fn proxy_test_unauthenticated_returns_401() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let (state, _jwks_mock) = make_state_with_jwks(db.pool.clone()).await;

    let app = make_proxy_router(state);

    let req = Request::builder()
        .method("POST")
        .uri("/v1/proxy/test")
        .header("content-type", "application/json")
        // No Authorization header.
        .body(Body::from(
            serde_json::json!({
                "url": "http://example.com/api",
                "method": "GET",
                "auth": { "type": "none" },
            })
            .to_string(),
        ))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

/// Body truncation: upstream returns >100 KB; response is capped and
/// `X-Recast-Truncated: true` is set.
#[tokio::test]
async fn proxy_test_body_truncation() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let (state, jwks_mock) = make_state_with_jwks(db.pool.clone()).await;

    // Build an upstream that returns a large JSON string (> 100 KB).
    let large_string = "x".repeat(110 * 1024); // 110 KB
    let large_body = serde_json::json!({ "data": large_string });

    let upstream = mcp_common::testing::MockUpstream::start().await;
    upstream.set_response_body(large_body);

    let app = make_proxy_router(state);
    let jwt = make_jwt("user_proxy_trunc", "trunc@example.com");

    let res = app
        .oneshot(proxy_request(
            &jwt,
            serde_json::json!({
                "url": upstream.url(),
                "method": "GET",
                "auth": { "type": "none" },
            }),
        ))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers().get("x-recast-truncated").map(|v| v.to_str().unwrap_or("")),
        Some("true"),
        "X-Recast-Truncated header must be 'true'"
    );

    drop(jwks_mock);
}

/// Invalid URL format returns 400 validation_error.
#[tokio::test]
async fn proxy_test_invalid_url_returns_validation_error() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let (state, jwks_mock) = make_state_with_jwks(db.pool.clone()).await;

    let app = make_proxy_router(state);
    let jwt = make_jwt("user_proxy_invalid_url", "invalid_url@example.com");

    let res = app
        .oneshot(proxy_request(
            &jwt,
            serde_json::json!({
                "url": "not-a-valid-url",
                "method": "GET",
                "auth": { "type": "none" },
            }),
        ))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = json_body(res).await;
    assert_eq!(body["error"]["code"], "validation_error");

    drop(jwks_mock);
}

/// Request body >100 KB returns 400 validation_error.
#[tokio::test]
async fn proxy_test_request_body_too_large_returns_validation_error() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let (state, jwks_mock) = make_state_with_jwks(db.pool.clone()).await;

    let upstream = mcp_common::testing::MockUpstream::start().await;
    upstream.set_response_body(serde_json::json!({ "ok": true }));

    let app = make_proxy_router(state);
    let jwt = make_jwt("user_proxy_body_size", "body_size@example.com");

    let oversized_body = "x".repeat(110 * 1024); // 110 KB
    let res = app
        .oneshot(proxy_request(
            &jwt,
            serde_json::json!({
                "url": upstream.url(),
                "method": "POST",
                "auth": { "type": "none" },
                "body": oversized_body,
            }),
        ))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = json_body(res).await;
    assert_eq!(body["error"]["code"], "validation_error");

    drop(jwks_mock);
}

/// Path parameter substitution: `{id}` replaced with the value in path_params.
#[tokio::test]
async fn proxy_test_path_param_substitution() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let (state, jwks_mock) = make_state_with_jwks(db.pool.clone()).await;

    let upstream = mcp_common::testing::MockUpstream::start().await;
    upstream.set_response_body(serde_json::json!({ "ok": true }));

    let app = make_proxy_router(state);
    let jwt = make_jwt("user_proxy_path", "path@example.com");

    let res = app
        .oneshot(proxy_request(
            &jwt,
            serde_json::json!({
                "url": format!("{}/users/{{id}}", upstream.url()),
                "method": "GET",
                "path_params": { "id": "42" },
                "auth": { "type": "none" },
            }),
        ))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    let requests = upstream.received_requests();
    assert!(!requests.is_empty());
    let path = &requests.first().unwrap().path;
    assert!(path.contains("/users/42"), "expected path /users/42, got: {path}");

    drop(jwks_mock);
}

/// Auth config with unknown fields returns 422 (deny_unknown_fields).
#[tokio::test]
async fn proxy_test_unknown_auth_field_returns_422() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let (state, jwks_mock) = make_state_with_jwks(db.pool.clone()).await;

    let upstream = mcp_common::testing::MockUpstream::start().await;
    upstream.set_response_body(serde_json::json!({ "ok": true }));

    let app = make_proxy_router(state);
    let jwt = make_jwt("user_proxy_unk_field", "unk_field@example.com");

    let res = app
        .oneshot(proxy_request(
            &jwt,
            serde_json::json!({
                "url": upstream.url(),
                "method": "GET",
                "auth": {
                    "type": "bearer",
                    "token": "abc",
                    "unknown_field": "should_fail"
                },
            }),
        ))
        .await
        .unwrap();

    // axum's Json extractor returns 422 for unknown fields with deny_unknown_fields.
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);

    drop(jwks_mock);
}
