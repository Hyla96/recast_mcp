//! Streamable HTTP transport for the MCP gateway.
//!
//! Implements the MCP 2025-03-26 Streamable HTTP transport:
//!
//! - `POST /mcp/:slug` — single JSON-RPC request/response or SSE stream
//! - CORS handled via `tower_http::cors::CorsLayer::permissive()` on the router layer
//!
//! # Request flow
//!
//! 1. Validate `Content-Type: application/json` → HTTP 415 otherwise.
//! 2. Resolve server slug from config cache → HTTP 404 (JSON-RPC envelope) if absent.
//! 3. Validate `Authorization: Bearer <token>` → HTTP 401/403 (JSON-RPC envelope) on failure.
//! 4. Parse body via `protocol::jsonrpc::parse`.
//! 5. Dispatch via `router::Router::dispatch`.
//! 6. Return:
//!    - Non-streaming (`Accept` does not contain `text/event-stream`): JSON response.
//!    - Streaming (`Accept` contains `text/event-stream`): SSE stream with the JSON-RPC
//!      response as the first `data:` event, kept alive for `sse_keepalive_secs` with
//!      15-second heartbeats.
//!
//! All responses carry `Cache-Control: no-store`. CORS headers are added by the layer.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
};
use futures_util::stream;
use mcp_protocol::JsonRpcResponse;
use serde_json::json;

use crate::auth::{extract_token_prefix, AuthError, TokenValidator};
use crate::cache::ConfigCache;
use crate::connections::{CapacityError, ConnectionGuard, ConnectionTracker};
use crate::protocol::jsonrpc::{parse, Message, ParseResult};
use crate::router::Router as McpRouter;

// ── TransportState ────────────────────────────────────────────────────────────

/// Shared state for the Streamable HTTP transport.
///
/// Construct once at startup and wrap in `Arc`:
///
/// ```ignore
/// let state = TransportState::new(cache, validator, router);
/// let transport_router = build_transport_router(state);
/// ```
pub struct TransportState {
    /// In-memory config cache for O(1) slug → server-config lookup.
    pub cache: Arc<ConfigCache>,
    /// Argon2id Bearer token validator with 30-second result cache.
    pub validator: Arc<TokenValidator>,
    /// JSON-RPC method dispatcher.
    pub router: Arc<McpRouter>,
    /// Per-server and global connection-limit tracker.
    pub connection_tracker: Arc<ConnectionTracker>,
    /// Seconds to hold an SSE connection alive after the initial response event.
    ///
    /// Production default: 60. Set to 0 in tests to avoid sleeping.
    pub sse_keepalive_secs: u64,
}

impl TransportState {
    /// Construct a new `TransportState` with the production SSE keepalive of 60 seconds.
    pub fn new(
        cache: Arc<ConfigCache>,
        validator: Arc<TokenValidator>,
        router: Arc<McpRouter>,
        connection_tracker: Arc<ConnectionTracker>,
    ) -> Arc<Self> {
        Arc::new(Self {
            cache,
            validator,
            router,
            connection_tracker,
            sse_keepalive_secs: 60,
        })
    }
}

// ── Handler ───────────────────────────────────────────────────────────────────

/// `POST /mcp/:slug` — Streamable HTTP MCP endpoint.
///
/// All responses are HTTP 200 for valid JSON-RPC, regardless of the JSON-RPC
/// error payload inside (spec-compliant). HTTP 4xx codes are used only for
/// transport-level errors (auth, slug resolution, content-type).
pub async fn mcp_post_handler(
    Path(slug): Path<String>,
    State(state): State<Arc<TransportState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // ── 1. Content-Type check ──────────────────────────────────────────────
    if !is_json_content_type(&headers) {
        let mut resp = (
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Content-Type must be application/json\n",
        )
            .into_response();
        let h = resp.headers_mut();
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        h.insert(
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            HeaderValue::from_static("*"),
        );
        return resp;
    }

    // ── 2. Slug resolution ────────────────────────────────────────────────
    let config = match state
        .cache
        .slug_to_id(&slug)
        .and_then(|id| state.cache.get(id))
    {
        Some(c) => c,
        None => {
            return build_json_error(StatusCode::NOT_FOUND, -32001, "Server not found");
        }
    };

    // ── 3. Bearer token authentication ────────────────────────────────────
    let auth_header = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    let token_prefix = match state.validator.validate_request(auth_header, &config).await {
        Ok(()) => auth_header
            .and_then(|h| h.strip_prefix("Bearer "))
            .map(extract_token_prefix),
        Err(
            AuthError::MissingHeader
            | AuthError::MalformedHeader
            | AuthError::InvalidToken,
        ) => {
            let mut resp = build_json_error(StatusCode::UNAUTHORIZED, -32000, "Unauthorized");
            resp.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                HeaderValue::from_static(r#"Bearer realm="mcp-gateway""#),
            );
            return resp;
        }
        Err(AuthError::ServerSuspended) => {
            return build_json_error(StatusCode::FORBIDDEN, -32000, "Forbidden");
        }
    };

    // ── 4. Connection limit check (after auth) ────────────────────────────
    //
    // ConnectionGuard decrements the counters on drop (including on panic
    // unwind), providing the same guarantee as `scopeguard::defer!`.
    let conn_guard = match state
        .connection_tracker
        .try_acquire(config.id, config.max_connections)
    {
        Ok(g) => g,
        Err(CapacityError::ServerLimitReached) => {
            let mut resp = build_json_error(
                StatusCode::SERVICE_UNAVAILABLE,
                -32005,
                "Server at capacity",
            );
            resp.headers_mut().insert(
                header::HeaderName::from_static("retry-after"),
                HeaderValue::from_static("1"),
            );
            return resp;
        }
        Err(CapacityError::GlobalLimitReached) => {
            let mut resp = build_json_error(
                StatusCode::SERVICE_UNAVAILABLE,
                -32005,
                "Gateway at capacity",
            );
            resp.headers_mut().insert(
                header::HeaderName::from_static("retry-after"),
                HeaderValue::from_static("1"),
            );
            return resp;
        }
    };

    // ── 5. Parse JSON-RPC body ────────────────────────────────────────────
    let message = parse(&body);

    // ── 6. SSE preference ─────────────────────────────────────────────────
    let wants_sse = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.contains("text/event-stream"))
        .unwrap_or(false);

    // ── 7. Dispatch and build response ────────────────────────────────────
    //
    // `conn_guard` is held for the entire response lifetime:
    //   - JSON responses: dropped when this function returns.
    //   - SSE responses: moved into the stream state and dropped when the
    //     stream ends (client disconnect or keepalive timeout).
    match message {
        Message::Single(parse_result) => {
            let skip_response = matches!(parse_result, ParseResult::Notification(_));
            let rpc_response =
                dispatch_result(&state.router, &slug, parse_result, token_prefix).await;

            // Pure notification with no result/error field → HTTP 202, no body.
            if skip_response
                && rpc_response.result.is_none()
                && rpc_response.error.is_none()
            {
                // conn_guard drops here (notification, no persistent connection).
                let _guard = conn_guard;
                let mut resp = StatusCode::ACCEPTED.into_response();
                let h = resp.headers_mut();
                h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
                h.insert(
                    header::ACCESS_CONTROL_ALLOW_ORIGIN,
                    HeaderValue::from_static("*"),
                );
                return resp;
            }

            let json_str = serialize_rpc(&rpc_response);
            if wants_sse {
                // Guard moves into the SSE stream — released when the stream ends.
                build_sse_response(json_str, state.sse_keepalive_secs, conn_guard)
            } else {
                // Guard drops when this function returns (after response is sent).
                let _guard = conn_guard;
                build_json_response(StatusCode::OK, json_str)
            }
        }
        Message::Batch(parse_results) => {
            let mut responses: Vec<JsonRpcResponse> = Vec::new();
            for result in parse_results {
                let skip = matches!(result, ParseResult::Notification(_));
                let rpc_response =
                    dispatch_result(&state.router, &slug, result, token_prefix.clone()).await;
                if !skip {
                    responses.push(rpc_response);
                }
            }
            let json_str = serde_json::to_string(&responses)
                .unwrap_or_else(|_| "[]".to_string());
            if wants_sse {
                build_sse_response(json_str, state.sse_keepalive_secs, conn_guard)
            } else {
                let _guard = conn_guard;
                build_json_response(StatusCode::OK, json_str)
            }
        }
    }
}

// ── Router construction ───────────────────────────────────────────────────────

/// `OPTIONS /mcp/:slug` — CORS preflight handler.
///
/// Responds with appropriate `Access-Control-*` headers for preflight requests
/// from browsers. The allowed origin, methods, and headers are set permissively
/// (`*`) so any MCP client can connect.
pub async fn mcp_options_handler() -> Response {
    Response::builder()
        .status(StatusCode::NO_CONTENT)
        .header("access-control-allow-origin", "*")
        .header("access-control-allow-methods", "POST, OPTIONS")
        .header("access-control-allow-headers", "content-type, authorization, accept")
        .header("access-control-max-age", "86400")
        .header(header::CACHE_CONTROL, "no-store")
        .body(axum::body::Body::empty())
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// Build an axum [`Router`] that mounts the MCP Streamable HTTP transport.
///
/// Routes:
/// - `POST /mcp/:slug` — MCP endpoint (JSON or SSE)
/// - `OPTIONS /mcp/:slug` — CORS preflight
///
/// CORS response headers (`Access-Control-Allow-Origin: *`) are added by both
/// the POST handler (via [`build_json_response`] / [`build_sse_response`]) and
/// the OPTIONS handler. This avoids a `CorsLayer` middleware that can interfere
/// with routing for non-CORS requests.
pub fn build_transport_router(state: Arc<TransportState>) -> axum::Router {
    use axum::routing::post;

    axum::Router::new()
        .route(
            "/mcp/:slug",
            post(mcp_post_handler).options(mcp_options_handler),
        )
        .with_state(state)
}

// ── Private helpers ───────────────────────────────────────────────────────────

/// Dispatch one `ParseResult` through the JSON-RPC router.
async fn dispatch_result(
    router: &McpRouter,
    slug: &str,
    result: ParseResult,
    token_prefix: Option<String>,
) -> JsonRpcResponse {
    match result {
        ParseResult::Request(req) | ParseResult::Notification(req) => {
            router.dispatch(slug, req, token_prefix).await
        }
        ParseResult::Error(resp) => resp,
    }
}

/// Build a JSON-RPC error envelope response with the given HTTP status code.
///
/// The body is a minimal JSON-RPC 2.0 error object: `{jsonrpc, error, id:null}`.
fn build_json_error(status: StatusCode, code: i32, message: &str) -> Response {
    let body = json!({
        "jsonrpc": "2.0",
        "error": { "code": code, "message": message },
        "id": null,
    })
    .to_string();
    build_json_response(status, body)
}

/// Build a `Content-Type: application/json` response with cache and CORS headers.
fn build_json_response(status: StatusCode, body: String) -> Response {
    let mut resp = (status, body).into_response();
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    );
    h.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    resp
}

/// Build an SSE response.
///
/// Sends `json_str` as the first `data:` event (immediately), then holds the
/// connection open for `keepalive_secs` seconds with 15-second heartbeat
/// comments (`KeepAlive`), then closes.
///
/// Set `keepalive_secs = 0` in tests to avoid any sleep and allow body
/// collection to complete immediately after the first event.
///
/// `guard` is moved into the stream state and released when the stream ends
/// (either after the keepalive timeout or when the client disconnects).
fn build_sse_response(
    json_str: String,
    keepalive_secs: u64,
    guard: ConnectionGuard,
) -> Response {
    // State machine: Some(data) → yield event, transition to None(secs).
    //                None       → sleep keepalive_secs, then end stream.
    // The guard lives in the state tuple for the full stream lifetime.
    let sse_stream = stream::unfold(
        (Some(json_str), keepalive_secs, guard),
        |(data_opt, secs, guard)| async move {
            match data_opt {
                Some(data) => {
                    let event = Event::default().data(data);
                    Some((Ok::<Event, Infallible>(event), (None, secs, guard)))
                }
                None => {
                    if secs > 0 {
                        tokio::time::sleep(Duration::from_secs(secs)).await;
                    }
                    // Stream ends; guard drops here, decrementing connection count.
                    drop(guard);
                    None
                }
            }
        },
    );

    let sse = Sse::new(sse_stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    );

    let mut resp = sse.into_response();
    let h = resp.headers_mut();
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    );
    h.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    resp
}

/// Serialise a `JsonRpcResponse` to a JSON string.
///
/// Falls back to a hardcoded internal-error envelope on serialisation failure
/// (should be unreachable for well-typed structs).
fn serialize_rpc(resp: &JsonRpcResponse) -> String {
    serde_json::to_string(resp).unwrap_or_else(|_| {
        r#"{"jsonrpc":"2.0","error":{"code":-32603,"message":"Internal serialization error"},"id":null}"#
            .to_string()
    })
}

/// Return `true` if the `Content-Type` header starts with `application/json`.
fn is_json_content_type(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|ct| ct.starts_with("application/json"))
        .unwrap_or(false)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::disallowed_methods
    )]

    use super::*;
    use crate::auth::generate_token;
    use crate::cache::{ConfigCache, ServerConfig};
    use crate::circuit_breaker::CircuitBreakerRegistry;
    use crate::logging::{LogLevel, RequestLogger};
    use crate::router::{Router as McpRouter, UpstreamPipeline};
    use crate::sidecar::{SidecarPool, UpstreamExecutor};
    use crate::upstream::UpstreamRequestBuilder;
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
    };
    use chrono::Utc;
    use mcp_common::testing::MockUpstream;
    use serde_json::json;
    use std::path::PathBuf;
    use tower::ServiceExt;
    use uuid::Uuid;

    // ── Test helpers ──────────────────────────────────────────────────────────

    /// Build a minimal `ServerConfig` for testing.
    fn make_server_config(
        name: &str,
        slug: &str,
        config_json: serde_json::Value,
        token_hash: Option<String>,
    ) -> Arc<ServerConfig> {
        Arc::new(ServerConfig {
            id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            name: name.to_string(),
            slug: slug.to_string(),
            description: None,
            config_json,
            status: "active".to_string(),
            config_version: 1,
            token_hash,
            token_prefix: None,
            max_connections: 50,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        })
    }

    /// Minimal `config_json` with a single GET tool and `auth_type: "none"`.
    fn simple_config_json(base_url: &str) -> serde_json::Value {
        json!({
            "base_url": base_url,
            "auth_type": "none",
            "tools": [{
                "name": "get_weather",
                "description": "Fetch current weather",
                "http_method": "GET",
                "path_template": "/weather",
                "query_params": [],
                "parameters": []
            }]
        })
    }

    /// Build a `TransportState` with a pre-populated cache.
    ///
    /// `sse_keepalive_secs = 0` to avoid any sleep in SSE tests.
    fn make_transport_state(
        config: Arc<ServerConfig>,
        sse_keepalive_secs: u64,
    ) -> Arc<TransportState> {
        use crate::connections::ConnectionTracker;
        let pool = sqlx::PgPool::connect_lazy("postgres://localhost/test_transport").unwrap();
        let cache = Arc::new(ConfigCache::new(pool));
        cache.upsert(Arc::clone(&config));

        let sidecar_pool = SidecarPool::new(PathBuf::from("/nonexistent/sidecar.sock"));
        let circuit_registry = CircuitBreakerRegistry::new();
        let http_client = reqwest::Client::new();
        let executor = Arc::new(UpstreamExecutor::new(
            sidecar_pool,
            http_client,
            circuit_registry,
        ));
        let request_builder = UpstreamRequestBuilder::with_allow_http(true);
        let upstream = UpstreamPipeline::new(executor, request_builder);

        let logger = RequestLogger::new(Uuid::new_v4().to_string(), LogLevel::Info);
        let router = Arc::new(McpRouter::new(Arc::clone(&cache), upstream, logger));

        Arc::new(TransportState {
            cache,
            validator: Arc::new(TokenValidator::new()),
            router,
            connection_tracker: ConnectionTracker::new(10_000),
            sse_keepalive_secs,
        })
    }

    /// Build a POST /mcp/:slug request with Content-Type: application/json.
    fn make_post(
        slug: &str,
        body: serde_json::Value,
        auth: Option<&str>,
    ) -> Request<Body> {
        let mut builder = Request::builder()
            .method("POST")
            .uri(format!("/mcp/{slug}"))
            .header("content-type", "application/json");
        if let Some(a) = auth {
            builder = builder.header("authorization", a);
        }
        builder.body(Body::from(body.to_string())).unwrap()
    }

    // ── Minimal routing smoke test ────────────────────────────────────────────

    /// Verify cache and slug lookup works correctly in test state.
    #[tokio::test]
    async fn slug_lookup_works_in_test_state() {
        let config =
            make_server_config("Test", "my-slug", simple_config_json("http://localhost"), None);
        let state = make_transport_state(Arc::clone(&config), 0);

        let id_opt = state.cache.slug_to_id("my-slug");
        assert!(id_opt.is_some(), "slug_to_id must return the server id");

        let config_opt = state.cache.get(id_opt.unwrap());
        assert!(config_opt.is_some(), "get must return the config");
    }

    /// Verify that slug lookup and auth flow work correctly through the full handler.
    ///
    /// This test uses a server WITH a token_hash (like the auth tests).
    /// It verifies that slug resolution succeeds even when a token_hash is configured.
    #[tokio::test]
    async fn slug_lookup_works_with_token_hash() {
        // Generate a token (CPU-bound, 2ms).
        let (_raw, hash) = generate_token().expect("generate_token");

        let config = make_server_config(
            "Token Server",
            "token-slug",
            simple_config_json("http://localhost"),
            Some(hash),
        );
        let state = make_transport_state(Arc::clone(&config), 0);

        // Verify the slug is in the cache.
        let id_opt = state.cache.slug_to_id("token-slug");
        assert!(id_opt.is_some(), "slug_to_id must work even with token_hash configured");

        // Verify the config is retrievable.
        let config_opt = state.cache.get(id_opt.unwrap());
        assert!(config_opt.is_some(), "get must return the config with token_hash");

        // Verify the handler returns 401 (not 404) for a request with no auth header.
        let app = build_transport_router(state);
        let req = make_post(
            "token-slug",
            json!({"jsonrpc":"2.0","method":"initialize","id":1}),
            None,
        );
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "handler must return 401 (auth failed), not 404 (slug not found)"
        );
    }

    /// Verify that axum routes POST /mcp/:slug requests correctly using oneshot.
    #[tokio::test]
    async fn route_matches_without_state() {
        use axum::{extract::Path as AxumPath, routing::post as routing_post};

        async fn echo(AxumPath(s): AxumPath<String>) -> String {
            s
        }

        let app = axum::Router::<()>::new().route("/mcp/:slug", routing_post(echo));
        let req = Request::builder()
            .method("POST")
            .uri("/mcp/hello")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "minimal route must match POST /mcp/{{slug}}"
        );
    }

    /// Verify auth returns 401 through a real TCP server (not oneshot).
    ///
    /// If this passes but `missing_auth_returns_401_with_www_authenticate` fails,
    /// the issue is with `oneshot` + Argon2 interaction.
    #[tokio::test]
    async fn auth_returns_401_via_tcp() {
        let (_raw, hash) = generate_token().expect("generate_token");
        let config = make_server_config(
            "Secure TCP",
            "secure-tcp",
            simple_config_json("http://localhost"),
            Some(hash),
        );
        let state = make_transport_state(config, 0);
        let app = build_transport_router(state);

        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], 0));
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            axum::serve(listener, app).await.ok();
        });

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://127.0.0.1:{port}/mcp/secure-tcp"))
            .header("content-type", "application/json")
            .body(json!({"jsonrpc":"2.0","method":"initialize","id":1}).to_string())
            .send()
            .await
            .unwrap();

        assert_eq!(
            resp.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "TCP-served request must return 401"
        );
    }

    /// Minimal: builds transport state inline (no make_transport_state helper).
    #[tokio::test]
    async fn minimal_inline_state_returns_404_or_401() {
        // Build cache inline.
        let pool = sqlx::PgPool::connect_lazy("postgres://localhost/test_minimal").unwrap();
        let cache = Arc::new(ConfigCache::new(pool));

        // Build config with a specific known slug.
        let config = Arc::new(ServerConfig {
            id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            name: "Inline".to_string(),
            slug: "inline-slug".to_string(),
            description: None,
            config_json: simple_config_json("http://localhost"),
            status: "active".to_string(),
            config_version: 1,
            token_hash: None,  // no auth for simplicity
            token_prefix: None,
            max_connections: 50,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        });
        cache.upsert(Arc::clone(&config));

        // Verify slug is in cache.
        assert!(cache.slug_to_id("inline-slug").is_some(), "inline slug must be in cache");

        // Build a minimal state without a real sidecar.
        let sidecar_pool = SidecarPool::new(PathBuf::from("/nonexistent.sock"));
        let circuit_registry = CircuitBreakerRegistry::new();
        let http_client = reqwest::Client::new();
        let executor = Arc::new(UpstreamExecutor::new(
            sidecar_pool,
            http_client,
            circuit_registry,
        ));
        let upstream = UpstreamPipeline::new(executor, UpstreamRequestBuilder::with_allow_http(true));
        let logger = RequestLogger::new(Uuid::new_v4().to_string(), LogLevel::Info);
        let mcp_router = Arc::new(McpRouter::new(Arc::clone(&cache), upstream, logger));

        let state = Arc::new(TransportState {
            cache,
            validator: Arc::new(TokenValidator::new()),
            router: mcp_router,
            connection_tracker: crate::connections::ConnectionTracker::new(10_000),
            sse_keepalive_secs: 0,
        });

        let app = build_transport_router(state);
        let req = Request::builder()
            .method("POST")
            .uri("/mcp/inline-slug")
            .header("content-type", "application/json")
            .body(Body::from(json!({"jsonrpc":"2.0","method":"initialize","id":1}).to_string()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        // Without token configured, auth returns InvalidToken → 401.
        // If this returns 404, slug lookup is broken.
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "inline state must return 401 (no token configured), not 404"
        );
    }

    // ── Transport-level error tests ───────────────────────────────────────────

    #[tokio::test]
    async fn wrong_content_type_returns_415() {
        let config =
            make_server_config("Test", "test", simple_config_json("http://localhost"), None);
        let state = make_transport_state(config, 0);
        let app = build_transport_router(state);

        let req = Request::builder()
            .method("POST")
            .uri("/mcp/test")
            .header("content-type", "text/plain")
            .body(Body::from("{}"))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }

    #[tokio::test]
    async fn unknown_slug_returns_404_with_jsonrpc_envelope() {
        let config =
            make_server_config("Test", "test", simple_config_json("http://localhost"), None);
        let state = make_transport_state(config, 0);
        let app = build_transport_router(state);

        let req = make_post(
            "nonexistent",
            json!({"jsonrpc":"2.0","method":"initialize","id":1}),
            None,
        );
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        let body =
            to_bytes(resp.into_body(), 65_536).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["jsonrpc"], "2.0");
        assert!(json["error"]["code"].is_number());
        assert_eq!(json["error"]["message"], "Server not found");
    }

    #[tokio::test]
    async fn missing_auth_returns_401_with_www_authenticate() {
        let (_raw, hash) = generate_token().expect("generate_token");
        let config = make_server_config(
            "Secure",
            "secure",
            simple_config_json("http://localhost"),
            Some(hash),
        );
        let state = make_transport_state(config, 0);
        let app = build_transport_router(state);

        // No Authorization header.
        let req = make_post(
            "secure",
            json!({"jsonrpc":"2.0","method":"initialize","id":1}),
            None,
        );
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(
            resp.headers().contains_key("www-authenticate"),
            "WWW-Authenticate header must be present on 401"
        );
    }

    #[tokio::test]
    async fn wrong_token_returns_401() {
        let (_raw, hash) = generate_token().expect("generate_token");
        let config = make_server_config(
            "Secure",
            "secure2",
            simple_config_json("http://localhost"),
            Some(hash),
        );
        let state = make_transport_state(config, 0);
        let app = build_transport_router(state);

        let req = make_post(
            "secure2",
            json!({"jsonrpc":"2.0","method":"initialize","id":1}),
            Some("Bearer AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
        );
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn suspended_server_returns_403() {
        let (raw, hash) = generate_token().expect("generate_token");
        let mut config = Arc::unwrap_or_clone(make_server_config(
            "Suspended",
            "suspended",
            simple_config_json("http://localhost"),
            Some(hash),
        ));
        config.status = "suspended".to_string();
        let config = Arc::new(config);

        let state = make_transport_state(config, 0);
        let app = build_transport_router(state);

        let auth = format!("Bearer {raw}");
        let req = make_post(
            "suspended",
            json!({"jsonrpc":"2.0","method":"initialize","id":1}),
            Some(&auth),
        );
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    // ── JSON-RPC method tests ─────────────────────────────────────────────────

    #[tokio::test]
    async fn initialize_returns_protocol_version_and_server_info() {
        let (raw, hash) = generate_token().expect("generate_token");
        let config = make_server_config(
            "My Weather API",
            "weather",
            simple_config_json("http://localhost"),
            Some(hash),
        );
        let state = make_transport_state(config, 0);
        let app = build_transport_router(state);

        let auth = format!("Bearer {raw}");
        let req = make_post(
            "weather",
            json!({"jsonrpc":"2.0","method":"initialize","id":1}),
            Some(&auth),
        );
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok()),
            Some("application/json"),
        );

        let body = to_bytes(resp.into_body(), 65_536).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(json["result"]["serverInfo"]["name"], "My Weather API");
        assert!(json["result"]["capabilities"]["tools"].is_object());
    }

    #[tokio::test]
    async fn tools_list_returns_tools_array() {
        let (raw, hash) = generate_token().expect("generate_token");
        let config = make_server_config(
            "Tools Test",
            "tools-test",
            simple_config_json("http://localhost"),
            Some(hash),
        );
        let state = make_transport_state(config, 0);
        let app = build_transport_router(state);

        let auth = format!("Bearer {raw}");
        let req = make_post(
            "tools-test",
            json!({"jsonrpc":"2.0","method":"tools/list","id":2}),
            Some(&auth),
        );
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = to_bytes(resp.into_body(), 65_536).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let tools = json["result"]["tools"].as_array().expect("tools must be array");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "get_weather");
    }

    #[tokio::test]
    async fn tools_call_returns_content_array() {
        let mock = MockUpstream::start().await;
        mock.set_response_body(json!({"temperature": 22, "unit": "celsius"}));

        let (raw, hash) = generate_token().expect("generate_token");
        let config_json = json!({
            "base_url": format!("http://{}", mock.addr),
            "auth_type": "none",
            "tools": [{
                "name": "get_weather",
                "description": "Fetch current weather",
                "http_method": "GET",
                "path_template": "/weather",
                "query_params": [],
                "parameters": []
            }]
        });
        let config =
            make_server_config("Weather", "weather-call", config_json, Some(hash));
        let state = make_transport_state(config, 0);
        let app = build_transport_router(state);

        let auth = format!("Bearer {raw}");
        let req = make_post(
            "weather-call",
            json!({"jsonrpc":"2.0","method":"tools/call","id":3,"params":{"name":"get_weather","arguments":{}}}),
            Some(&auth),
        );
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = to_bytes(resp.into_body(), 65_536).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let content = json["result"]["content"]
            .as_array()
            .expect("content must be array");
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
        let text = content[0]["text"].as_str().expect("text must be string");
        let parsed: serde_json::Value =
            serde_json::from_str(text).expect("text must be valid JSON");
        assert_eq!(parsed["temperature"], 22);

        // Verify the mock received exactly one request.
        let reqs = mock.received_requests();
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].path, "/weather");
    }

    // ── SSE transport tests ───────────────────────────────────────────────────

    #[tokio::test]
    async fn sse_response_has_correct_content_type_and_framing() {
        let (raw, hash) = generate_token().expect("generate_token");
        let config = make_server_config(
            "SSE Test",
            "sse-test",
            simple_config_json("http://localhost"),
            Some(hash),
        );
        // keepalive_secs=0: stream ends immediately after the first event so
        // to_bytes() collects the body without blocking for 60 seconds.
        let state = make_transport_state(config, 0);
        let app = build_transport_router(state);

        let auth = format!("Bearer {raw}");
        let req = Request::builder()
            .method("POST")
            .uri("/mcp/sse-test")
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .header("authorization", &auth)
            .body(Body::from(
                json!({"jsonrpc":"2.0","method":"initialize","id":1}).to_string(),
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            ct.contains("text/event-stream"),
            "Content-Type must contain text/event-stream; got: {ct}"
        );

        let body = to_bytes(resp.into_body(), 65_536).await.unwrap();
        let body_str = std::str::from_utf8(&body).expect("SSE body must be UTF-8");

        // SSE events are framed as "data: <json>\n\n".
        assert!(
            body_str.starts_with("data: "),
            "SSE body must start with 'data: '; got: {body_str:?}"
        );
        assert!(
            body_str.contains("protocolVersion"),
            "SSE body must contain the JSON-RPC initialize response; got: {body_str:?}"
        );
        // Verify double-newline terminator.
        assert!(
            body_str.contains("\n\n"),
            "SSE events must be terminated with double newline; got: {body_str:?}"
        );
    }

    // ── CORS tests ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn cors_preflight_options_is_handled() {
        let config =
            make_server_config("Test", "cors-test", simple_config_json("http://localhost"), None);
        let state = make_transport_state(config, 0);
        let app = build_transport_router(state);

        let req = Request::builder()
            .method("OPTIONS")
            .uri("/mcp/cors-test")
            .header("origin", "https://example.com")
            .header("access-control-request-method", "POST")
            .header("access-control-request-headers", "content-type, authorization")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();

        assert!(
            resp.headers()
                .contains_key("access-control-allow-origin"),
            "CORS origin header must be present on OPTIONS preflight response"
        );
    }

    #[tokio::test]
    async fn cors_origin_present_on_json_response() {
        let config =
            make_server_config("Test", "cors-json", simple_config_json("http://localhost"), None);
        let state = make_transport_state(config, 0);
        let app = build_transport_router(state);

        // Trigger a 404 (unknown slug) — the CorsLayer should still add CORS headers.
        let req = Request::builder()
            .method("POST")
            .uri("/mcp/cors-json")
            .header("content-type", "application/json")
            .header("origin", "https://example.com")
            .body(Body::from(json!({"jsonrpc":"2.0","method":"ping","id":1}).to_string()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();

        // 401 because no token configured — but CORS header must still be present.
        assert!(
            resp.headers()
                .contains_key("access-control-allow-origin"),
            "CORS origin header must be present on all responses"
        );
    }

    // ── Cache-Control tests ───────────────────────────────────────────────────

    #[tokio::test]
    async fn cache_control_no_store_on_404() {
        let config =
            make_server_config("Test", "cc-test", simple_config_json("http://localhost"), None);
        let state = make_transport_state(config, 0);
        let app = build_transport_router(state);

        let req = make_post(
            "nonexistent-slug",
            json!({"jsonrpc":"2.0","method":"ping","id":1}),
            None,
        );
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            resp.headers()
                .get("cache-control")
                .and_then(|v| v.to_str().ok()),
            Some("no-store"),
            "Cache-Control must be no-store on error responses"
        );
    }

    #[tokio::test]
    async fn cache_control_no_store_on_200() {
        let (raw, hash) = generate_token().expect("generate_token");
        let config = make_server_config(
            "CC Test",
            "cc-200",
            simple_config_json("http://localhost"),
            Some(hash),
        );
        let state = make_transport_state(config, 0);
        let app = build_transport_router(state);

        let auth = format!("Bearer {raw}");
        let req = make_post(
            "cc-200",
            json!({"jsonrpc":"2.0","method":"ping","id":1}),
            Some(&auth),
        );
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get("cache-control")
                .and_then(|v| v.to_str().ok()),
            Some("no-store"),
            "Cache-Control must be no-store on 200 responses"
        );
    }

    // ── Complete integration flow ─────────────────────────────────────────────

    /// Three-request flow: initialize → tools/list → tools/call.
    #[tokio::test]
    async fn complete_initialize_tools_list_tools_call_flow() {
        let mock = MockUpstream::start().await;
        mock.set_response_body(json!({"result": "ok", "data": 42}));

        let (raw, hash) = generate_token().expect("generate_token");
        let config_json = json!({
            "base_url": format!("http://{}", mock.addr),
            "auth_type": "none",
            "tools": [{
                "name": "do_thing",
                "description": "Perform an action",
                "http_method": "GET",
                "path_template": "/do",
                "query_params": [],
                "parameters": []
            }]
        });
        let config =
            make_server_config("Integration API", "integ-api", config_json, Some(hash));
        let state = make_transport_state(Arc::clone(&config), 0);
        let auth = format!("Bearer {raw}");

        // ── Request 1: initialize ──────────────────────────────────────────
        {
            let app = build_transport_router(Arc::clone(&state));
            let req = make_post(
                "integ-api",
                json!({"jsonrpc":"2.0","method":"initialize","id":1}),
                Some(&auth),
            );
            let resp = app.oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "initialize must return 200");
            let body = to_bytes(resp.into_body(), 65_536).await.unwrap();
            let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(
                json["result"]["protocolVersion"], "2025-03-26",
                "initialize must return protocolVersion 2025-03-26"
            );
            assert_eq!(
                json["result"]["serverInfo"]["name"], "Integration API",
                "initialize serverInfo name must match config"
            );
            assert!(
                json["result"]["capabilities"]["tools"].is_object(),
                "initialize capabilities must include tools"
            );
        }

        // ── Request 2: tools/list ──────────────────────────────────────────
        {
            let app = build_transport_router(Arc::clone(&state));
            let req = make_post(
                "integ-api",
                json!({"jsonrpc":"2.0","method":"tools/list","id":2}),
                Some(&auth),
            );
            let resp = app.oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "tools/list must return 200");
            let body = to_bytes(resp.into_body(), 65_536).await.unwrap();
            let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
            let tools = json["result"]["tools"]
                .as_array()
                .expect("tools must be an array");
            assert!(!tools.is_empty(), "tools/list must return at least one tool");
            assert_eq!(tools[0]["name"], "do_thing");
        }

        // ── Request 3: tools/call ──────────────────────────────────────────
        {
            let app = build_transport_router(Arc::clone(&state));
            let req = make_post(
                "integ-api",
                json!({"jsonrpc":"2.0","method":"tools/call","id":3,"params":{"name":"do_thing","arguments":{}}}),
                Some(&auth),
            );
            let resp = app.oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "tools/call must return 200");
            let body = to_bytes(resp.into_body(), 65_536).await.unwrap();
            let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert!(
                json["result"]["content"].is_array(),
                "tools/call result must have a content array; got: {json}"
            );
        }
    }

    // ── Connection-limit tests ────────────────────────────────────────────────

    /// Build a `TransportState` with a pre-populated cache and custom `ConnectionTracker`.
    fn make_state_with_tracker(
        config: Arc<ServerConfig>,
        tracker: Arc<crate::connections::ConnectionTracker>,
    ) -> Arc<TransportState> {
        let pool =
            sqlx::PgPool::connect_lazy("postgres://localhost/test_conn_limit").unwrap();
        let cache = Arc::new(ConfigCache::new(pool));
        cache.upsert(Arc::clone(&config));

        let sidecar_pool = SidecarPool::new(PathBuf::from("/nonexistent/sidecar.sock"));
        let circuit_registry = CircuitBreakerRegistry::new();
        let http_client = reqwest::Client::new();
        let executor = Arc::new(UpstreamExecutor::new(
            sidecar_pool,
            http_client,
            circuit_registry,
        ));
        let request_builder = UpstreamRequestBuilder::with_allow_http(true);
        let upstream = UpstreamPipeline::new(executor, request_builder);

        let logger = RequestLogger::new(Uuid::new_v4().to_string(), LogLevel::Info);
        let router = Arc::new(McpRouter::new(Arc::clone(&cache), upstream, logger));

        Arc::new(TransportState {
            cache,
            validator: Arc::new(TokenValidator::new()),
            router,
            connection_tracker: tracker,
            sse_keepalive_secs: 0,
        })
    }

    /// A request that goes past auth but has `max_connections = 0` (always 503).
    #[tokio::test]
    async fn server_at_capacity_returns_503_with_retry_after() {
        use crate::auth::generate_token;
        use crate::connections::ConnectionTracker;

        // Auth must pass before the connection-limit check runs.
        let (raw, hash) = generate_token().expect("generate_token");
        let config = Arc::new({
            let mut c = Arc::unwrap_or_clone(make_server_config(
                "Cap Test",
                "cap-test",
                simple_config_json("http://localhost"),
                Some(hash),
            ));
            c.max_connections = 0; // always refuse — limit hit immediately
            c
        });

        // Global limit is generous; per-server limit of 0 should trigger 503.
        let tracker = ConnectionTracker::new(10_000);
        let state = make_state_with_tracker(config, tracker);
        let app = build_transport_router(state);

        let auth = format!("Bearer {raw}");
        let req = make_post(
            "cap-test",
            json!({"jsonrpc":"2.0","method":"ping","id":1}),
            Some(&auth),
        );
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "must return 503 when per-server connection limit is exhausted"
        );
        assert_eq!(
            resp.headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok()),
            Some("1"),
            "Retry-After: 1 must be present on 503 capacity response"
        );

        // Body must be a JSON-RPC error envelope (not HTML).
        let body = to_bytes(resp.into_body(), 4_096).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"]["code"], -32005, "error code must be -32005");
        assert!(
            json["error"]["message"].as_str().is_some(),
            "error message must be present"
        );
    }

    /// Global limit of 0 also triggers 503.
    #[tokio::test]
    async fn global_capacity_returns_503() {
        use crate::auth::generate_token;
        use crate::connections::ConnectionTracker;

        let (raw, hash) = generate_token().expect("generate_token");
        let config = make_server_config(
            "Global Cap",
            "global-cap",
            simple_config_json("http://localhost"),
            Some(hash),
        );

        let tracker = ConnectionTracker::new(0); // global limit of zero
        let state = make_state_with_tracker(config, tracker);
        let app = build_transport_router(state);

        let auth = format!("Bearer {raw}");
        let req = make_post(
            "global-cap",
            json!({"jsonrpc":"2.0","method":"ping","id":1}),
            Some(&auth),
        );
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
