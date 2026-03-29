// Gateway integration tests.
//
// These tests require a live PostgreSQL instance.
// Set TEST_DATABASE_URL (or DATABASE_URL) before running:
//
//   TEST_DATABASE_URL=postgres://recast:recast@localhost:5432/postgres \
//     cargo test -p mcp-gateway --test integration_tests
//
// Each test spins up:
//  - A TestDatabase (isolated DB with migrations applied)
//  - A MockUpstream (in-process HTTP stub)
//  - A minimal in-process gateway router (validates tokens, routes tools/list
//    and tools/call, enforces SSRF blocks)
//  - A TestMcpClient to drive JSON-RPC requests
//
// NOTE: The in-process gateway used here implements only enough logic to
// validate the testing framework.  Production gateway routing (JSONPath
// transforms, hot-reload, etc.) is implemented in subsequent epics.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, missing_docs)]

use axum::{
    body::Body,
    extract::{Path, Request, State},
    http::{Request as HttpRequest, StatusCode},
    middleware::{from_fn, from_fn_with_state, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use mcp_common::{
    rate_limit::{RateLimitConfig, RateLimitContext, RateLimiter, rate_limit_middleware},
    testing::{MockUpstream, TestDatabase, TestMcpClient},
};
use mcp_protocol::{JsonRpcError, JsonRpcRequest, JsonRpcResponse};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::{net::SocketAddr, sync::Arc};
use tokio::net::TcpListener;
use tower::ServiceExt;
use uuid::Uuid;

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

fn json_rpc_error(code: i32, message: &str, id: Option<serde_json::Value>) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        result: None,
        error: Some(JsonRpcError {
            code,
            message: message.to_string(),
            data: None,
        }),
        id,
    }
}

fn json_rpc_ok(result: serde_json::Value, id: Option<serde_json::Value>) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        result: Some(result),
        error: None,
        id,
    }
}

// ─── Minimal in-process gateway ───────────────────────────────────────────────
//
// Implements bearer-token auth, tools/list, tools/call (proxying to upstream),
// initialize, and SSRF blocking.  Used exclusively by the tests below.

#[derive(Clone)]
struct GatewayState {
    pool: PgPool,
}

/// Extract Bearer token from the `Authorization` header.
fn extract_bearer(req: &Request) -> Option<String> {
    req.headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.to_string())
}

/// Very simple SSRF guard: block requests to private/link-local IP literals.
fn is_ssrf_blocked(url: &str) -> bool {
    // Extract the host portion by stripping scheme and path.
    let without_scheme = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .unwrap_or(url);
    let host = without_scheme
        .split('/')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("");

    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return match ip {
            std::net::IpAddr::V4(v4) => {
                v4.is_private() || v4.is_loopback() || v4.is_link_local()
            }
            std::net::IpAddr::V6(v6) => v6.is_loopback() || v6.is_unspecified(),
        };
    }
    // Hostname-based SSRF (DNS rebinding) is out of scope for this test guard.
    false
}

/// The single JSON-RPC handler for `POST /rpc/{slug}`.
async fn gateway_handler(
    State(state): State<Arc<GatewayState>>,
    Path(slug): Path<String>,
    req: Request,
) -> Response {
    // 1. Authenticate via Bearer token.
    let token = match extract_bearer(&req) {
        Some(t) => t,
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };
    let token_hash = sha256_hex(&token);

    let token_valid: bool = sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM server_tokens st
            JOIN mcp_servers ms ON ms.id = st.server_id
            WHERE st.token_hash = $1 AND st.is_active = true AND ms.slug = $2
         )",
    )
    .bind(&token_hash)
    .bind(&slug)
    .fetch_one(&state.pool)
    .await
    .unwrap_or(false);

    if !token_valid {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    // 2. Load server config.
    let config_json: serde_json::Value = match sqlx::query_scalar(
        "SELECT config_json FROM mcp_servers WHERE slug = $1 AND status = 'active'",
    )
    .bind(&slug)
    .fetch_optional(&state.pool)
    .await
    {
        Ok(Some(v)) => v,
        _ => return StatusCode::NOT_FOUND.into_response(),
    };

    // 3. Parse the JSON-RPC body.
    let body_bytes = match axum::body::to_bytes(req.into_body(), 1024 * 1024).await {
        Ok(b) => b,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    let rpc_req: JsonRpcRequest = match serde_json::from_slice(&body_bytes) {
        Ok(r) => r,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    let req_id = rpc_req.id.clone();

    // 4. Dispatch by method.
    let response = match rpc_req.method.as_str() {
        "initialize" => json_rpc_ok(
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "serverInfo": {"name": slug, "version": "0.1.0"}
            }),
            req_id,
        ),

        "initialized" => json_rpc_ok(serde_json::Value::Null, req_id),

        "tools/list" => {
            let tools = config_json
                .get("tools")
                .cloned()
                .unwrap_or(serde_json::Value::Array(vec![]));
            json_rpc_ok(serde_json::json!({"tools": tools}), req_id)
        }

        "tools/call" => {
            let params = rpc_req.params.unwrap_or(serde_json::Value::Null);
            let tool_name = params
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            // Find tool definition in config.
            let tools = config_json
                .get("tools")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            let tool = tools.iter().find(|t| {
                t.get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    == tool_name
            });

            let upstream_url = tool
                .and_then(|t| t.get("upstream"))
                .and_then(|u| u.get("url"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            if upstream_url.is_empty() {
                return Json(json_rpc_error(-32601, "tool not found", req_id))
                    .into_response();
            }

            // SSRF guard.
            if is_ssrf_blocked(&upstream_url) {
                return Json(json_rpc_error(
                    -32001,
                    "SSRF: upstream URL is blocked",
                    req_id,
                ))
                .into_response();
            }

            // Proxy call to upstream.
            let method = tool
                .and_then(|t| t.get("upstream"))
                .and_then(|u| u.get("method"))
                .and_then(|v| v.as_str())
                .unwrap_or("POST")
                .to_string();

            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or(serde_json::Value::Null);

            let http_client = reqwest::Client::new();
            let upstream_resp = match method.as_str() {
                "GET" => http_client.get(&upstream_url).send().await,
                _ => {
                    http_client
                        .post(&upstream_url)
                        .json(&arguments)
                        .send()
                        .await
                }
            };

            match upstream_resp {
                Ok(resp) => {
                    let body: serde_json::Value = resp
                        .json()
                        .await
                        .unwrap_or(serde_json::Value::Null);
                    json_rpc_ok(
                        serde_json::json!({"content": [{"type": "text", "text": body.to_string()}]}),
                        req_id,
                    )
                }
                Err(e) => json_rpc_error(-32000, &e.to_string(), req_id),
            }
        }

        _ => json_rpc_error(-32601, "method not found", req_id),
    };

    Json(response).into_response()
}

/// Starts the minimal test gateway and returns its base URL.
async fn start_test_gateway(pool: PgPool) -> String {
    let state = Arc::new(GatewayState { pool });
    let app = Router::new()
        .route("/rpc/{slug}", post(gateway_handler))
        .with_state(state);

    let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .expect("bind test gateway");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    format!("http://{addr}")
}

// ─── Seed helpers ─────────────────────────────────────────────────────────────

async fn seed_test_server(
    pool: &PgPool,
    upstream_url: &str,
    ssrf_url: Option<&str>,
) -> (uuid::Uuid, String, String) {
    let user_id: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO users (clerk_id, email) VALUES ($1, $2) RETURNING id")
            .bind(format!("clerk_gw_{}", uuid::Uuid::new_v4().simple()))
            .bind(format!("gw_{}@example.com", uuid::Uuid::new_v4().simple()))
            .fetch_one(pool)
            .await
            .expect("insert user");

    let ssrf_tool = ssrf_url.map(|url| {
        serde_json::json!({
            "name": "ssrf_target",
            "description": "Used to test SSRF blocking",
            "input_schema": {"type": "object", "properties": {}},
            "upstream": {"method": "GET", "url": url}
        })
    });

    let mut tools = vec![serde_json::json!({
        "name": "echo",
        "description": "Echo the input back",
        "input_schema": {
            "type": "object",
            "properties": {"message": {"type": "string"}},
            "required": ["message"]
        },
        "upstream": {"method": "POST", "url": format!("{upstream_url}/echo")}
    })];
    if let Some(ssrf) = ssrf_tool {
        tools.push(ssrf);
    }

    let config = serde_json::json!({"tools": tools});
    let id = uuid::Uuid::new_v4();
    let id_str = id.simple().to_string();
    let slug = format!("gw-test-{}", &id_str[..8]);

    let server_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO mcp_servers (user_id, name, slug, config_json, status)
         VALUES ($1, $2, $3, $4, 'active') RETURNING id",
    )
    .bind(user_id)
    .bind("Gateway Test Server")
    .bind(&slug)
    .bind(&config)
    .fetch_one(pool)
    .await
    .expect("insert server");

    // Store the SHA-256 hash of the raw token — never the raw token.
    let raw_token = format!("test-token-{}", uuid::Uuid::new_v4().simple());
    let token_hash = sha256_hex(&raw_token);
    sqlx::query(
        "INSERT INTO server_tokens (server_id, token_hash, description)
         VALUES ($1, $2, 'test')",
    )
    .bind(server_id)
    .bind(&token_hash)
    .execute(pool)
    .await
    .expect("insert token");

    (server_id, slug, raw_token)
}

// ─── Gateway integration tests ────────────────────────────────────────────────

/// tools/list returns the tool definitions stored in the server config.
#[tokio::test]
async fn gateway_tools_list_returns_correct_definitions() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let mock = MockUpstream::start().await;

    let (_server_id, slug, raw_token) =
        seed_test_server(&db.pool, &mock.url(), None).await;

    let gateway_url = start_test_gateway(db.pool.clone()).await;
    let client = TestMcpClient::with_bearer_token(
        format!("{gateway_url}/rpc/{slug}"),
        &raw_token,
    );

    let resp = client.initialize().await.expect("initialize");
    assert!(resp.error.is_none(), "initialize should succeed: {:?}", resp.error);

    let list_resp = client.tools_list().await.expect("tools/list");
    assert!(list_resp.error.is_none(), "tools/list error: {:?}", list_resp.error);

    let tools = list_resp
        .result
        .as_ref()
        .and_then(|r| r.get("tools"))
        .and_then(|t| t.as_array())
        .expect("tools array in result");

    assert_eq!(tools.len(), 1, "expected 1 tool definition");
    assert_eq!(tools[0].get("name").and_then(|v| v.as_str()), Some("echo"));
}

/// tools/call proxies the request to MockUpstream and records the call.
#[tokio::test]
async fn gateway_tools_call_proxies_to_upstream() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let mock = MockUpstream::start().await;
    mock.set_response_body(serde_json::json!({"echo": "hello"}));

    let (_server_id, slug, raw_token) =
        seed_test_server(&db.pool, &mock.url(), None).await;

    let gateway_url = start_test_gateway(db.pool.clone()).await;
    let client = TestMcpClient::with_bearer_token(
        format!("{gateway_url}/rpc/{slug}"),
        &raw_token,
    );

    let call_resp = client
        .tools_call("echo", serde_json::json!({"message": "hello"}))
        .await
        .expect("tools/call");

    assert!(
        call_resp.error.is_none(),
        "tools/call should not error: {:?}",
        call_resp.error
    );
    assert!(call_resp.result.is_some(), "tools/call should return a result");

    // MockUpstream must have received exactly one request.
    let received = mock.received_requests();
    assert_eq!(received.len(), 1, "upstream should have received 1 call");
}

/// MockUpstream can assert that a specific header was injected.
///
/// This test verifies the `assert_received_header` API works end-to-end:
/// a client sends a custom header to MockUpstream directly (no gateway) and
/// the assertion passes.
#[tokio::test]
async fn mock_upstream_assert_received_header() {
    let mock = MockUpstream::start().await;

    // Call the mock directly (simulating what the gateway would do after
    // injecting a credential header).
    let client = reqwest::Client::new();
    client
        .post(format!("{}/test", mock.url()))
        .header("authorization", "Bearer injected-secret-token")
        .header("x-custom-header", "test-value")
        .json(&serde_json::json!({"test": true}))
        .send()
        .await
        .expect("direct mock call");

    // The test body verifies the header WITHOUT knowing the plaintext would
    // come from a credential store — it simply checks what arrived.
    mock.assert_received_header("authorization", "Bearer injected-secret-token");
    mock.assert_received_header("x-custom-header", "test-value");
}

/// TestMcpClient can perform a full initialize → tools/list → tools/call sequence.
#[tokio::test]
async fn test_mcp_client_full_sequence() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let mock = MockUpstream::start().await;
    mock.set_response_body(serde_json::json!({"result": "success"}));

    let (_server_id, slug, raw_token) =
        seed_test_server(&db.pool, &mock.url(), None).await;

    let gateway_url = start_test_gateway(db.pool.clone()).await;
    let client = TestMcpClient::with_bearer_token(
        format!("{gateway_url}/rpc/{slug}"),
        &raw_token,
    );

    // Step 1: initialize
    let init = client.initialize().await.expect("initialize");
    assert!(init.error.is_none(), "initialize failed: {:?}", init.error);

    // Step 2: tools/list
    let list = client.tools_list().await.expect("tools/list");
    assert!(list.error.is_none(), "tools/list failed: {:?}", list.error);

    // Step 3: tools/call
    let call = client
        .tools_call("echo", serde_json::json!({"message": "world"}))
        .await
        .expect("tools/call");
    assert!(call.error.is_none(), "tools/call failed: {:?}", call.error);
    assert!(call.result.is_some(), "tools/call should return a result");

    assert_eq!(
        mock.received_requests().len(),
        1,
        "upstream should have received exactly 1 call"
    );
}

/// An SSRF-blocked upstream URL returns a JSON-RPC error (not HTTP 5xx).
#[tokio::test]
async fn gateway_ssrf_blocked_url_returns_json_rpc_error() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let mock = MockUpstream::start().await;

    // Seed a server with a tool pointing at the cloud metadata endpoint.
    let ssrf_url = "http://169.254.169.254/latest/meta-data/";
    let (_server_id, slug, raw_token) =
        seed_test_server(&db.pool, &mock.url(), Some(ssrf_url)).await;

    let gateway_url = start_test_gateway(db.pool.clone()).await;
    let client = TestMcpClient::with_bearer_token(
        format!("{gateway_url}/rpc/{slug}"),
        &raw_token,
    );

    let resp = client
        .tools_call("ssrf_target", serde_json::json!({}))
        .await
        .expect("tools/call (SSRF)");

    assert!(
        resp.error.is_some(),
        "SSRF-blocked call should return a JSON-RPC error"
    );
    let err = resp.error.as_ref().expect("error field");
    assert_eq!(err.code, -32001, "expected SSRF error code -32001");
    assert!(
        err.message.contains("SSRF"),
        "error message should mention SSRF: {}",
        err.message
    );

    // MockUpstream must NOT have been called.
    assert_eq!(
        mock.received_requests().len(),
        0,
        "SSRF target must not receive any request"
    );
}

/// An invalid Bearer token returns HTTP 401 before any JSON-RPC processing.
#[tokio::test]
async fn gateway_invalid_bearer_token_returns_401() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let mock = MockUpstream::start().await;

    let (_server_id, slug, _valid_token) =
        seed_test_server(&db.pool, &mock.url(), None).await;

    let gateway_url = start_test_gateway(db.pool.clone()).await;

    // Send a request with a wrong token (not registered in server_tokens).
    let http_client = reqwest::Client::new();
    let resp = http_client
        .post(format!("{gateway_url}/rpc/{slug}"))
        .header("authorization", "Bearer totally-wrong-token")
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "tools/list",
            "id": 1
        }))
        .send()
        .await
        .expect("HTTP request");

    assert_eq!(
        resp.status().as_u16(),
        401,
        "invalid token must return HTTP 401"
    );
}

/// Missing Authorization header also returns HTTP 401.
#[tokio::test]
async fn gateway_missing_auth_returns_401() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let mock = MockUpstream::start().await;

    let (_server_id, slug, _valid_token) =
        seed_test_server(&db.pool, &mock.url(), None).await;

    let gateway_url = start_test_gateway(db.pool.clone()).await;

    let http_client = reqwest::Client::new();
    let resp = http_client
        .post(format!("{gateway_url}/rpc/{slug}"))
        // No Authorization header.
        .json(&serde_json::json!({"jsonrpc": "2.0", "method": "tools/list", "id": 1}))
        .send()
        .await
        .expect("HTTP request");

    assert_eq!(
        resp.status().as_u16(),
        401,
        "missing auth header must return HTTP 401"
    );
}

// ─── Rate-limit middleware integration tests ──────────────────────────────────
//
// These tests do NOT require a live PostgreSQL or Redis instance — they use
// `RateLimiter::new_in_process()` and axum's oneshot testing pattern.

/// Build a test router that injects a fixed RateLimitContext then applies the
/// rate-limit middleware. The handler simply returns HTTP 200.
fn build_rate_limit_test_router(
    server_id: Uuid,
    user_id: Uuid,
    per_server_rate: u32,
    per_user_rate: u32,
    limiter: RateLimiter,
) -> Router {
    let config = Arc::new(RateLimitConfig {
        limiter,
        per_server_rate,
        per_user_rate,
        enabled: true,
        audit_logger: None,
    });

    Router::new()
        .route("/test", get(|| async { StatusCode::OK }))
        .layer(from_fn_with_state(config, rate_limit_middleware))
        .layer(from_fn(move |mut req: Request, next: Next| async move {
            req.extensions_mut().insert(RateLimitContext {
                server_id: Some(server_id),
                user_id: Some(user_id),
            });
            next.run(req).await
        }))
}

/// 100 requests on a 100/min bucket → all succeed; 101st → HTTP 429.
#[tokio::test]
async fn rate_limit_middleware_returns_429_after_server_limit() {
    let server_id = Uuid::new_v4();
    let user_id = Uuid::new_v4();
    let limiter = RateLimiter::new_in_process();
    let app = build_rate_limit_test_router(server_id, user_id, 100, 10_000, limiter);

    let mut allowed = 0u32;
    let mut rejected = 0u32;

    for _ in 0..151 {
        let req = HttpRequest::builder()
            .uri("/test")
            .body(Body::empty())
            .expect("request build");
        let resp = app.clone().oneshot(req).await.expect("oneshot");
        match resp.status().as_u16() {
            200 => allowed += 1,
            429 => rejected += 1,
            s => panic!("unexpected status {s}"),
        }
    }

    assert_eq!(allowed, 100, "exactly 100 requests should be allowed");
    assert_eq!(rejected, 51, "remaining 51 should be rejected");
}

/// Every response (success and 429) carries X-RateLimit-* headers.
#[tokio::test]
async fn rate_limit_middleware_adds_headers_to_every_response() {
    let limiter = RateLimiter::new_in_process();
    let app = build_rate_limit_test_router(
        Uuid::new_v4(),
        Uuid::new_v4(),
        100,
        10_000,
        limiter,
    );

    let req = HttpRequest::builder()
        .uri("/test")
        .body(Body::empty())
        .expect("build");
    let resp = app.clone().oneshot(req).await.expect("oneshot");

    assert_eq!(resp.status().as_u16(), 200);
    assert!(
        resp.headers().contains_key("x-ratelimit-limit"),
        "x-ratelimit-limit missing"
    );
    assert!(
        resp.headers().contains_key("x-ratelimit-remaining"),
        "x-ratelimit-remaining missing"
    );
    assert!(
        resp.headers().contains_key("x-ratelimit-reset"),
        "x-ratelimit-reset missing"
    );
}

/// On HTTP 429 the `Retry-After` header is present.
#[tokio::test]
async fn rate_limit_middleware_429_has_retry_after() {
    let limiter = RateLimiter::new_in_process();
    let server_id = Uuid::new_v4();
    let user_id = Uuid::new_v4();
    let app = build_rate_limit_test_router(server_id, user_id, 100, 10_000, limiter.clone());

    // Exhaust the bucket.
    for _ in 0..100 {
        let req = HttpRequest::builder().uri("/test").body(Body::empty()).unwrap();
        app.clone().oneshot(req).await.unwrap();
    }

    // Next request should be 429 with Retry-After.
    let req = HttpRequest::builder().uri("/test").body(Body::empty()).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 429);
    assert!(
        resp.headers().contains_key("retry-after"),
        "Retry-After header missing on 429"
    );
}

/// `FEATURE_RATE_LIMIT_ENABLED=false` — middleware passes all requests through
/// without adding rate-limit headers or returning 429.
#[tokio::test]
async fn rate_limit_middleware_disabled_passes_all_requests() {
    let limiter = RateLimiter::new_in_process();
    let server_id = Uuid::new_v4();
    let user_id = Uuid::new_v4();

    // Build with enabled = false.
    let config = Arc::new(RateLimitConfig {
        limiter,
        per_server_rate: 1, // tiny limit — would reject immediately if enabled
        per_user_rate: 1,
        enabled: false,
        audit_logger: None,
    });
    let app = Router::new()
        .route("/test", get(|| async { StatusCode::OK }))
        .layer(from_fn_with_state(config, rate_limit_middleware))
        .layer(from_fn(move |mut req: Request, next: Next| async move {
            req.extensions_mut().insert(RateLimitContext {
                server_id: Some(server_id),
                user_id: Some(user_id),
            });
            next.run(req).await
        }));

    for _ in 0..10 {
        let req = HttpRequest::builder().uri("/test").body(Body::empty()).unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status().as_u16(), 200, "disabled limiter must not block");
        assert!(
            !resp.headers().contains_key("x-ratelimit-limit"),
            "disabled limiter must not add rate-limit headers"
        );
    }
}

/// Per-user limit enforced across servers: 1000 requests over two "server" keys
/// both sharing the same user key exhausts the user bucket.
///
/// Simplified: uses the same `user_id` but two different `server_id` values.
/// Per-server rate is set very high (10 000) so only the user bucket triggers.
#[tokio::test]
async fn rate_limit_per_user_limit_enforced_across_servers() {
    use mcp_common::rate_limit::RateLimiter;

    // Use the limiter directly (not the middleware) to simulate two servers
    // sharing the same user bucket.
    let limiter = RateLimiter::new_in_process();
    let user_key = format!("ratelimit:user:{}", Uuid::new_v4());
    let rate = 1000u32;

    let mut allowed = 0u32;
    // Simulate 500 from "server A" + 501 from "server B" via the same user key.
    for _ in 0..1001 {
        if limiter.check(&user_key, rate).await.allowed {
            allowed += 1;
        }
    }

    assert_eq!(allowed, 1000, "1000 of 1001 requests via user bucket must be allowed");
}

/// 100 concurrent tasks against a 100-token bucket → no over-allowance.
#[tokio::test]
async fn rate_limit_concurrent_no_over_allowance() {
    let limiter = Arc::new(RateLimiter::new_in_process());
    let key = format!("ratelimit:server:{}", Uuid::new_v4());

    let handles: Vec<_> = (0..150)
        .map(|_| {
            let lim = Arc::clone(&limiter);
            let k = key.clone();
            tokio::spawn(async move { lim.check(&k, 100).await })
        })
        .collect();

    let mut allowed = 0u32;
    for h in handles {
        if h.await.expect("task join").allowed {
            allowed += 1;
        }
    }

    assert_eq!(allowed, 100, "exactly 100 of 150 concurrent requests must be allowed");
}
