//! SSE fallback transport for legacy MCP clients.
//!
//! Implements the two-endpoint SSE transport for clients that do not support
//! Streamable HTTP (the MCP 2025-03-26 primary transport).
//!
//! # Endpoints
//!
//! - `GET /sse/:slug` — establish a persistent SSE session.
//!   On connect, sends an `endpoint` SSE event:
//!   `data: /messages/{slug}?session_id=<uuid>`
//!
//! - `POST /messages/:slug?session_id=<uuid>` — send a JSON-RPC request.
//!   Returns HTTP 202 immediately; the JSON-RPC response is delivered over
//!   the open SSE stream.
//!
//! # Session lifecycle
//!
//! 1. Client connects to `GET /sse/:slug` with a valid Bearer token.
//! 2. Gateway generates a UUIDv4 session ID and registers it in the registry.
//! 3. Gateway sends `event: endpoint\ndata: /messages/{slug}?session_id={id}\n\n`.
//! 4. Client POSTs JSON-RPC requests to `/messages/{slug}?session_id={id}`.
//! 5. Gateway dispatches each request, sends the response as `data: <json>\n\n`.
//! 6. Session closes when:
//!    - Client disconnects (stream poll returns `None`).
//!    - 120 seconds of inactivity (background sweeper detects and closes).
//!    - Gateway shuts down (`is_shutting_down` flag set).

use std::convert::Infallible;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
};
use dashmap::DashMap;
use futures_util::stream;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::auth::{extract_token_prefix, AuthError, TokenValidator};
use crate::cache::ConfigCache;
use crate::connections::{CapacityError, ConnectionGuard, ConnectionTracker};
use crate::protocol::jsonrpc::{parse, Message, ParseResult};
use crate::router::Router as McpRouter;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Maximum concurrent SSE sessions (legacy transport).
pub const MAX_SSE_SESSIONS: usize = 10_000;

/// Inactivity timeout after which the session is closed and a cancellation
/// notification is sent to the client.
const SESSION_TIMEOUT_SECS: u64 = 120;

/// How often the background sweeper checks for idle sessions.
const SWEEP_INTERVAL_SECS: u64 = 30;

// ── SessionHandle ─────────────────────────────────────────────────────────────

/// In-registry handle for one active SSE session.
struct SessionHandle {
    /// Channel to push JSON-RPC response strings to the SSE stream.
    sender: mpsc::Sender<String>,
    /// Unix timestamp (seconds) of last activity. Updated on every POST.
    last_activity: Arc<AtomicU64>,
    /// MCP server slug (for routing incoming POST /messages/:slug).
    slug: String,
    /// Bearer token prefix for log correlation (may be None if auth_type=none).
    token_prefix: Option<String>,
}

// ── SessionRegistry ───────────────────────────────────────────────────────────

/// Thread-safe registry of active SSE sessions.
///
/// Keyed by UUIDv4 session ID. Backed by a `DashMap`. The background sweeper
/// task calls `sweep()` every [`SWEEP_INTERVAL_SECS`] to close idle sessions.
pub struct SessionRegistry {
    sessions: DashMap<Uuid, SessionHandle>,
}

impl SessionRegistry {
    /// Create a new, empty [`SessionRegistry`].
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            sessions: DashMap::new(),
        })
    }

    /// Current number of active sessions.
    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    /// Returns `true` if there are no active sessions.
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    /// Register a new session. Returns the `Receiver` half for the SSE stream.
    fn register(
        &self,
        session_id: Uuid,
        slug: String,
        token_prefix: Option<String>,
    ) -> (mpsc::Receiver<String>, Arc<AtomicU64>) {
        let (tx, rx) = mpsc::channel(64);
        let last_activity = Arc::new(AtomicU64::new(now_secs()));
        self.sessions.insert(
            session_id,
            SessionHandle {
                sender: tx,
                last_activity: Arc::clone(&last_activity),
                slug,
                token_prefix,
            },
        );
        (rx, last_activity)
    }

    /// Remove a session entry by ID (called on disconnect or timeout).
    fn remove(&self, session_id: &Uuid) {
        self.sessions.remove(session_id);
    }

    /// Look up a session and send a message to it.
    ///
    /// Returns `Ok(())` on success, `Err(SendError)` if the session is gone
    /// (channel closed) or does not exist.
    fn send(&self, session_id: &Uuid, message: String) -> Result<(), SessionSendError> {
        let entry = self
            .sessions
            .get(session_id)
            .ok_or(SessionSendError::NotFound)?;
        // Update activity timestamp.
        entry.last_activity.store(now_secs(), Ordering::Relaxed);
        entry
            .sender
            .try_send(message)
            .map_err(|_| SessionSendError::Closed)
    }

    /// Sweep for sessions that have been idle for > [`SESSION_TIMEOUT_SECS`].
    ///
    /// Sends a `notifications/cancelled` message and removes the session.
    fn sweep(&self) {
        let cutoff = now_secs().saturating_sub(SESSION_TIMEOUT_SECS);
        let mut timed_out: Vec<Uuid> = Vec::new();

        for entry in self.sessions.iter() {
            let last = entry.value().last_activity.load(Ordering::Relaxed);
            if last < cutoff {
                timed_out.push(*entry.key());
            }
        }

        for session_id in timed_out {
            // Send cancellation before removing so client can handle it.
            let cancel = json!({
                "jsonrpc": "2.0",
                "method": "notifications/cancelled",
                "params": { "reason": "session_timeout" }
            })
            .to_string();
            // Best-effort: ignore error if channel already closed.
            if let Some(entry) = self.sessions.get(&session_id) {
                let _ = entry.value().sender.try_send(cancel);
            }
            self.sessions.remove(&session_id);
            tracing::info!(
                session_id = %session_id,
                "SSE session closed due to inactivity"
            );
        }
    }
}

#[derive(Debug)]
enum SessionSendError {
    NotFound,
    Closed,
}

// ── SseFallbackState ──────────────────────────────────────────────────────────

/// Shared state for the SSE fallback transport.
pub struct SseFallbackState {
    /// In-memory config cache for O(1) slug → server-config lookup.
    pub cache: Arc<ConfigCache>,
    /// Argon2id Bearer token validator with 30-second result cache.
    pub validator: Arc<TokenValidator>,
    /// JSON-RPC method dispatcher.
    pub router: Arc<McpRouter>,
    /// Per-server and global connection-limit tracker.
    pub connection_tracker: Arc<ConnectionTracker>,
    /// Registry of active SSE sessions.
    pub registry: Arc<SessionRegistry>,
    /// Set to `true` when the gateway is draining for shutdown.
    pub is_shutting_down: Arc<AtomicBool>,
    /// Seconds between SSE keepalive heartbeat comments. 0 disables (test mode).
    pub keepalive_interval_secs: u64,
}

impl SseFallbackState {
    /// Construct a new [`SseFallbackState`] with production keepalive of 15 seconds.
    pub fn new(
        cache: Arc<ConfigCache>,
        validator: Arc<TokenValidator>,
        router: Arc<McpRouter>,
        connection_tracker: Arc<ConnectionTracker>,
        registry: Arc<SessionRegistry>,
        is_shutting_down: Arc<AtomicBool>,
    ) -> Arc<Self> {
        Arc::new(Self {
            cache,
            validator,
            router,
            connection_tracker,
            registry,
            is_shutting_down,
            keepalive_interval_secs: 15,
        })
    }
}

// ── Background sweeper ────────────────────────────────────────────────────────

/// Spawn the session sweeper task.
///
/// Runs every [`SWEEP_INTERVAL_SECS`] seconds and removes sessions that have
/// been idle for more than [`SESSION_TIMEOUT_SECS`] seconds.
pub fn spawn_session_sweeper(registry: Arc<SessionRegistry>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let interval = Duration::from_secs(SWEEP_INTERVAL_SECS);
        loop {
            tokio::time::sleep(interval).await;
            registry.sweep();
        }
    })
}

// ── Query params ──────────────────────────────────────────────────────────────

/// Query parameters for `POST /messages/:slug`.
#[derive(Deserialize)]
pub struct SessionQuery {
    /// Session ID returned in the `endpoint` SSE event.
    pub session_id: Option<Uuid>,
}

// ── GET /sse/:slug ────────────────────────────────────────────────────────────

/// `GET /sse/:slug` — open a new SSE session.
///
/// Authentication and connection limits are checked before the session is
/// created. On success, sends:
/// 1. `event: endpoint\ndata: /messages/{slug}?session_id={id}\n\n`
/// 2. Subsequent `data: <json-rpc-response>\n\n` events pushed by POST handlers.
/// 3. Eventual cancellation event if the session times out.
///
/// HTTP 15s keepalive comments are emitted automatically.
pub async fn sse_connect_handler(
    Path(slug): Path<String>,
    State(state): State<Arc<SseFallbackState>>,
    headers: HeaderMap,
) -> Response {
    // ── 0. Shutdown check ──────────────────────────────────────────────────
    if state.is_shutting_down.load(Ordering::SeqCst) {
        let mut resp = sse_json_error(StatusCode::SERVICE_UNAVAILABLE, "Gateway is shutting down");
        resp.headers_mut().insert(
            header::CONNECTION,
            HeaderValue::from_static("close"),
        );
        resp.headers_mut().insert(
            header::HeaderName::from_static("retry-after"),
            HeaderValue::from_static("5"),
        );
        return resp;
    }

    // ── 1. Session capacity check ──────────────────────────────────────────
    if state.registry.len() >= MAX_SSE_SESSIONS {
        let mut resp = sse_json_error(StatusCode::SERVICE_UNAVAILABLE, "SSE session limit reached");
        resp.headers_mut().insert(
            header::HeaderName::from_static("retry-after"),
            HeaderValue::from_static("5"),
        );
        return resp;
    }

    // ── 2. Slug resolution ─────────────────────────────────────────────────
    let config = match state
        .cache
        .slug_to_id(&slug)
        .and_then(|id| state.cache.get(id))
    {
        Some(c) => c,
        None => {
            return sse_json_error(StatusCode::NOT_FOUND, "Server not found");
        }
    };

    // ── 3. Bearer token authentication ─────────────────────────────────────
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
            let mut resp = sse_json_error(StatusCode::UNAUTHORIZED, "Unauthorized");
            resp.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                HeaderValue::from_static(r#"Bearer realm="mcp-gateway""#),
            );
            return resp;
        }
        Err(AuthError::ServerSuspended) => {
            return sse_json_error(StatusCode::FORBIDDEN, "Forbidden");
        }
    };

    // ── 4. Connection limit check (after auth) ─────────────────────────────
    let conn_guard = match state
        .connection_tracker
        .try_acquire(config.id, config.max_connections)
    {
        Ok(g) => g,
        Err(CapacityError::ServerLimitReached) => {
            let mut resp = sse_json_error(StatusCode::SERVICE_UNAVAILABLE, "Server at capacity");
            resp.headers_mut().insert(
                header::HeaderName::from_static("retry-after"),
                HeaderValue::from_static("1"),
            );
            return resp;
        }
        Err(CapacityError::GlobalLimitReached) => {
            let mut resp = sse_json_error(StatusCode::SERVICE_UNAVAILABLE, "Gateway at capacity");
            resp.headers_mut().insert(
                header::HeaderName::from_static("retry-after"),
                HeaderValue::from_static("1"),
            );
            return resp;
        }
    };

    // ── 5. Create session ──────────────────────────────────────────────────
    let session_id = Uuid::new_v4();
    let (rx, _last_activity) =
        state
            .registry
            .register(session_id, slug.clone(), token_prefix);

    tracing::debug!(
        session_id = %session_id,
        slug = %slug,
        "SSE session created"
    );

    // ── 6. Build SSE stream ────────────────────────────────────────────────
    //
    // State machine:
    //   Phase 0: emit the `endpoint` event with the messages URL.
    //   Phase 1: drain the mpsc receiver until closed (connection/timeout).
    //
    // `conn_guard` lives in the stream state for the full session lifetime.
    // `registry` reference kept to clean up on disconnect.
    let endpoint_url = format!("/messages/{slug}?session_id={session_id}");
    let registry = Arc::clone(&state.registry);

    enum StreamState {
        Endpoint {
            url: String,
            rx: mpsc::Receiver<String>,
            guard: ConnectionGuard,
            registry: Arc<SessionRegistry>,
            session_id: Uuid,
        },
        Messages {
            rx: mpsc::Receiver<String>,
            guard: ConnectionGuard,
            registry: Arc<SessionRegistry>,
            session_id: Uuid,
        },
    }

    let initial = StreamState::Endpoint {
        url: endpoint_url,
        rx,
        guard: conn_guard,
        registry,
        session_id,
    };

    let sse_stream = stream::unfold(initial, |state| async move {
        match state {
            StreamState::Endpoint {
                url,
                rx,
                guard,
                registry,
                session_id,
            } => {
                // First event: endpoint URL so client knows where to POST.
                let event = Event::default().event("endpoint").data(url);
                Some((
                    Ok::<Event, Infallible>(event),
                    StreamState::Messages {
                        rx,
                        guard,
                        registry,
                        session_id,
                    },
                ))
            }
            StreamState::Messages {
                mut rx,
                guard,
                registry,
                session_id,
            } => match rx.recv().await {
                Some(json_str) => {
                    let event = Event::default().data(json_str);
                    Some((
                        Ok(event),
                        StreamState::Messages {
                            rx,
                            guard,
                            registry,
                            session_id,
                        },
                    ))
                }
                None => {
                    // Channel closed: client disconnected or session timed out.
                    // Remove from registry and release the connection guard.
                    registry.remove(&session_id);
                    drop(guard);
                    tracing::debug!(session_id = %session_id, "SSE session stream ended");
                    None
                }
            },
        }
    });

    let keepalive = if state.keepalive_interval_secs > 0 {
        KeepAlive::new()
            .interval(Duration::from_secs(state.keepalive_interval_secs))
            .text("ping")
    } else {
        KeepAlive::new()
            .interval(Duration::from_secs(3600)) // effectively disabled in tests
            .text("ping")
    };

    let sse = Sse::new(sse_stream).keep_alive(keepalive);

    let mut resp = sse.into_response();
    let h = resp.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    resp
}

// ── POST /messages/:slug ──────────────────────────────────────────────────────

/// `POST /messages/:slug?session_id=<uuid>` — submit a JSON-RPC request.
///
/// Returns HTTP 202 immediately. The JSON-RPC response is delivered
/// asynchronously over the client's open SSE stream.
pub async fn sse_messages_handler(
    Path(slug): Path<String>,
    Query(query): Query<SessionQuery>,
    State(state): State<Arc<SseFallbackState>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    // ── 0. session_id required ─────────────────────────────────────────────
    let session_id = match query.session_id {
        Some(id) => id,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                build_plain_error("session_id query parameter is required"),
            )
                .into_response();
        }
    };

    // ── 1. Content-Type check ──────────────────────────────────────────────
    let is_json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|ct| ct.starts_with("application/json"))
        .unwrap_or(false);
    if !is_json {
        return (
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            build_plain_error("Content-Type must be application/json"),
        )
            .into_response();
    }

    // ── 2. Session exists? ─────────────────────────────────────────────────
    //
    // Check that the session is registered. This also verifies the slug
    // matches the session (the session stores the slug it was created for).
    {
        let entry = match state.registry.sessions.get(&session_id) {
            Some(e) => e,
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    build_plain_error("Unknown session_id"),
                )
                    .into_response();
            }
        };
        // Verify the slug in the URL matches the session.
        if entry.slug != slug {
            return (
                StatusCode::BAD_REQUEST,
                build_plain_error("session_id does not belong to this server slug"),
            )
                .into_response();
        }
    }

    // ── 3. Parse JSON-RPC ──────────────────────────────────────────────────
    let message = parse(&body);

    // ── 4. Dispatch and push response to SSE stream ────────────────────────
    //
    // We dispatch inline (async) and push the result to the SSE channel.
    // The POST handler returns 202 once the response is pushed.
    //
    // Token prefix is stored on the session handle at creation time; we use
    // it here for structured logging only.
    let token_prefix = state
        .registry
        .sessions
        .get(&session_id)
        .and_then(|e| e.value().token_prefix.clone());

    match message {
        Message::Single(parse_result) => {
            let is_notification = matches!(parse_result, ParseResult::Notification(_));
            let rpc_resp = dispatch_one(&state.router, &slug, parse_result, token_prefix).await;

            // Notifications produce a null-result response; don't send it back.
            if is_notification && rpc_resp.result.is_none() && rpc_resp.error.is_none() {
                return StatusCode::ACCEPTED.into_response();
            }

            let json_str = serialize_rpc(&rpc_resp);
            let _ = state.registry.send(&session_id, json_str);
        }
        Message::Batch(results) => {
            let mut responses: Vec<mcp_protocol::JsonRpcResponse> = Vec::new();
            for result in results {
                let skip = matches!(result, ParseResult::Notification(_));
                let rpc = dispatch_one(&state.router, &slug, result, token_prefix.clone()).await;
                if !skip {
                    responses.push(rpc);
                }
            }
            let json_str =
                serde_json::to_string(&responses).unwrap_or_else(|_| "[]".to_string());
            let _ = state.registry.send(&session_id, json_str);
        }
    }

    StatusCode::ACCEPTED.into_response()
}

// ── Router construction ───────────────────────────────────────────────────────

/// Build an axum [`Router`] that mounts the SSE fallback transport.
///
/// Routes:
/// - `GET /sse/:slug` — open SSE session
/// - `POST /messages/:slug` — submit request to active session
pub fn build_sse_router(state: Arc<SseFallbackState>) -> axum::Router {
    use axum::routing::{get, post};

    axum::Router::new()
        .route("/sse/:slug", get(sse_connect_handler))
        .route("/messages/:slug", post(sse_messages_handler))
        .with_state(state)
}

// ── Private helpers ───────────────────────────────────────────────────────────

/// Dispatch one `ParseResult` through the JSON-RPC router.
async fn dispatch_one(
    router: &McpRouter,
    slug: &str,
    result: ParseResult,
    token_prefix: Option<String>,
) -> mcp_protocol::JsonRpcResponse {
    match result {
        ParseResult::Request(req) | ParseResult::Notification(req) => {
            router.dispatch(slug, req, token_prefix).await
        }
        ParseResult::Error(resp) => resp,
    }
}

/// Serialise a `JsonRpcResponse` to a JSON string.
fn serialize_rpc(resp: &mcp_protocol::JsonRpcResponse) -> String {
    serde_json::to_string(resp).unwrap_or_else(|_| {
        r#"{"jsonrpc":"2.0","error":{"code":-32603,"message":"Internal serialization error"},"id":null}"#.to_string()
    })
}

/// Build a plain JSON error response for transport-level errors.
fn sse_json_error(status: StatusCode, message: &str) -> Response {
    let body = json!({
        "jsonrpc": "2.0",
        "error": { "code": -32000, "message": message },
        "id": null,
    })
    .to_string();
    let mut resp = (status, body).into_response();
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    resp
}

/// Plain string error body for HTTP 400/415.
fn build_plain_error(msg: &str) -> String {
    msg.to_string()
}

/// Current Unix timestamp in seconds.
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
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
    use crate::connections::ConnectionTracker;
    use crate::logging::{LogLevel, RequestLogger};
    use crate::router::{Router as McpRouter, UpstreamPipeline};
    use crate::sidecar::{SidecarPool, UpstreamExecutor};
    use crate::upstream::UpstreamRequestBuilder;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use chrono::Utc;
    use mcp_common::testing::MockUpstream;
    use serde_json::{json, Value};
    use std::path::PathBuf;
    use tower::ServiceExt;
    use uuid::Uuid;

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn make_server_config(
        name: &str,
        slug: &str,
        config_json: Value,
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

    fn simple_config_json(base_url: &str) -> Value {
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

    fn make_sse_state(config: Arc<ServerConfig>) -> Arc<SseFallbackState> {
        let pool =
            sqlx::PgPool::connect_lazy("postgres://localhost/test_sse").unwrap();
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

        Arc::new(SseFallbackState {
            cache,
            validator: Arc::new(TokenValidator::new()),
            router,
            connection_tracker: ConnectionTracker::new(10_000),
            registry: SessionRegistry::new(),
            is_shutting_down: Arc::new(AtomicBool::new(false)),
            keepalive_interval_secs: 0,
        })
    }

    // ── Tests ──────────────────────────────────────────────────────────────────

    /// GET /sse/:slug returns 404 for unknown slug.
    #[tokio::test]
    async fn unknown_slug_returns_404() {
        let config =
            make_server_config("Test", "known-slug", simple_config_json("http://localhost"), None);
        let state = make_sse_state(config);
        let app = build_sse_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/sse/unknown-slug")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    /// GET /sse/:slug returns 401 when server has a token and no auth header.
    #[tokio::test]
    async fn missing_auth_returns_401() {
        let (_raw, hash) = generate_token().expect("generate_token");
        let config = make_server_config(
            "Secure",
            "secure-sse",
            simple_config_json("http://localhost"),
            Some(hash),
        );
        let state = make_sse_state(config);
        let app = build_sse_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/sse/secure-sse")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(resp.headers().contains_key(header::WWW_AUTHENTICATE));
    }

    /// GET /sse/:slug with valid auth returns 200 (SSE text/event-stream).
    #[tokio::test]
    async fn sse_connect_returns_event_stream() {
        let (raw, hash) = generate_token().expect("generate_token");
        let config = make_server_config(
            "Open",
            "open-sse",
            simple_config_json("http://localhost"),
            Some(hash),
        );
        let state = make_sse_state(config);
        let app = build_sse_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/sse/open-sse")
            .header("authorization", format!("Bearer {raw}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp.headers().get(header::CONTENT_TYPE).unwrap();
        assert!(
            ct.to_str().unwrap().contains("text/event-stream"),
            "expected text/event-stream, got: {ct:?}"
        );
    }

    /// SSE stream contains the endpoint event as first data.
    #[tokio::test]
    async fn sse_stream_contains_endpoint_event() {
        let (raw, hash) = generate_token().expect("generate_token");
        let config = make_server_config(
            "EpTest",
            "ep-slug",
            simple_config_json("http://localhost"),
            Some(hash),
        );
        let state = make_sse_state(config);

        // Build transport router and bind to a real TCP port.
        let app = build_sse_router(Arc::clone(&state));
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], 0));
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            axum::serve(listener, app).await.ok();
        });

        // Connect and read first few bytes.
        let client = reqwest::Client::new();
        let resp = client
            .get(format!("http://127.0.0.1:{port}/sse/ep-slug"))
            .header("authorization", format!("Bearer {raw}"))
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), reqwest::StatusCode::OK);
        assert!(
            resp.headers()
                .get("content-type")
                .unwrap()
                .to_str()
                .unwrap()
                .contains("text/event-stream")
        );

        // Read enough bytes to capture the endpoint event.
        // The event looks like: "event: endpoint\ndata: /messages/ep-slug?session_id=...\n\n"
        let mut body_stream = resp.bytes_stream();
        use futures_util::StreamExt;
        let mut accumulated = String::new();
        while let Some(chunk) = body_stream.next().await {
            let chunk = chunk.unwrap();
            accumulated.push_str(&String::from_utf8_lossy(&chunk));
            if accumulated.contains("session_id=") {
                break;
            }
        }

        assert!(
            accumulated.contains("event: endpoint"),
            "expected 'event: endpoint' in SSE stream, got:\n{accumulated}"
        );
        assert!(
            accumulated.contains("/messages/ep-slug"),
            "expected endpoint URL in SSE stream, got:\n{accumulated}"
        );
        assert!(
            accumulated.contains("session_id="),
            "expected session_id in endpoint event, got:\n{accumulated}"
        );
    }

    /// POST /messages/:slug returns 400 for unknown session_id.
    #[tokio::test]
    async fn unknown_session_returns_400() {
        let config = make_server_config(
            "Test",
            "msg-slug",
            simple_config_json("http://localhost"),
            None,
        );
        let state = make_sse_state(config);
        let app = build_sse_router(state);

        let unknown_id = Uuid::new_v4();
        let req = Request::builder()
            .method("POST")
            .uri(format!("/messages/msg-slug?session_id={unknown_id}"))
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"jsonrpc":"2.0","method":"ping","id":1}).to_string(),
            ))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    /// POST /messages/:slug returns 415 for non-JSON content type.
    #[tokio::test]
    async fn messages_non_json_returns_415() {
        let config = make_server_config(
            "Test",
            "ct-slug",
            simple_config_json("http://localhost"),
            None,
        );
        let state = make_sse_state(config);

        // Register a fake session so we reach the content-type check.
        let session_id = Uuid::new_v4();
        let (_rx, _la) = state.registry.register(
            session_id,
            "ct-slug".to_string(),
            None,
        );

        let app = build_sse_router(state);

        let req = Request::builder()
            .method("POST")
            .uri(format!("/messages/ct-slug?session_id={session_id}"))
            .header("content-type", "text/plain")
            .body(Body::from("hello"))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }

    /// POST /messages/:slug returns 400 when session_id param is missing.
    #[tokio::test]
    async fn messages_missing_session_id_returns_400() {
        let config = make_server_config(
            "Test",
            "noid-slug",
            simple_config_json("http://localhost"),
            None,
        );
        let state = make_sse_state(config);
        let app = build_sse_router(state);

        let req = Request::builder()
            .method("POST")
            .uri("/messages/noid-slug")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"jsonrpc":"2.0","method":"ping","id":1}).to_string(),
            ))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    /// SSE sweeper removes idle sessions.
    #[tokio::test]
    async fn sweeper_removes_idle_sessions() {
        let registry = SessionRegistry::new();
        let session_id = Uuid::new_v4();
        let (_rx, last_activity) =
            registry.register(session_id, "test".to_string(), None);

        // Make the session appear 200s old.
        last_activity.store(
            now_secs().saturating_sub(SESSION_TIMEOUT_SECS + 80),
            Ordering::Relaxed,
        );

        assert_eq!(registry.len(), 1);
        registry.sweep();
        assert_eq!(registry.len(), 0);
    }

    /// Sweeper preserves active sessions.
    #[tokio::test]
    async fn sweeper_preserves_active_sessions() {
        let registry = SessionRegistry::new();
        let session_id = Uuid::new_v4();
        let (_rx, _la) = registry.register(session_id, "test".to_string(), None);

        // Session is brand new, last_activity = now.
        registry.sweep();
        assert_eq!(registry.len(), 1, "active session must not be swept");
    }

    /// SessionRegistry::send returns NotFound for unknown session.
    #[tokio::test]
    async fn registry_send_unknown_session() {
        let registry = SessionRegistry::new();
        let result = registry.send(&Uuid::new_v4(), "hello".to_string());
        assert!(
            matches!(result, Err(SessionSendError::NotFound)),
            "expected NotFound for unknown session"
        );
    }

    /// Full flow integration test: connect SSE, capture endpoint event,
    /// POST initialize, verify response on SSE stream, POST tools/list.
    #[tokio::test]
    async fn full_sse_flow_initialize_and_tools_list() {
        let mock = MockUpstream::start().await;
        mock.set_response_body(json!({"result": "ok"}));
        let base_url = format!("http://{}", mock.addr);
        let (raw, hash) = generate_token().expect("generate_token");

        let config = make_server_config(
            "FlowTest",
            "flow-slug",
            simple_config_json(&base_url),
            Some(hash),
        );
        let state = make_sse_state(config);
        let app = build_sse_router(Arc::clone(&state));

        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], 0));
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, app).await.ok();
        });

        let client = reqwest::Client::new();

        // 1. Connect SSE.
        let sse_resp = client
            .get(format!("http://127.0.0.1:{port}/sse/flow-slug"))
            .header("authorization", format!("Bearer {raw}"))
            .send()
            .await
            .unwrap();
        assert_eq!(sse_resp.status(), reqwest::StatusCode::OK);

        let mut body_stream = sse_resp.bytes_stream();
        use futures_util::StreamExt;

        // 2. Read the endpoint event and extract session_id.
        let mut accumulated = String::new();
        while let Some(chunk) = body_stream.next().await {
            accumulated.push_str(&String::from_utf8_lossy(&chunk.unwrap()));
            if accumulated.contains("session_id=") {
                break;
            }
        }

        // Parse session_id from: "data: /messages/flow-slug?session_id=<uuid>"
        let session_id = accumulated
            .lines()
            .find(|l| l.starts_with("data:"))
            .and_then(|l| l.split("session_id=").nth(1))
            .map(|s| s.trim().to_string())
            .expect("session_id not found in endpoint event");

        // 3. POST initialize via /messages.
        let init_resp = client
            .post(format!(
                "http://127.0.0.1:{port}/messages/flow-slug?session_id={session_id}"
            ))
            .header("content-type", "application/json")
            .body(
                json!({"jsonrpc":"2.0","method":"initialize","id":1,"params":{"protocolVersion":"2025-03-26","capabilities":{}}})
                    .to_string(),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(init_resp.status(), reqwest::StatusCode::ACCEPTED);

        // 4. Read initialize response from SSE stream.
        let mut init_json = String::new();
        while let Some(chunk) = body_stream.next().await {
            let text = String::from_utf8_lossy(&chunk.unwrap()).to_string();
            // collect data lines
            for line in text.lines() {
                if let Some(data) = line.strip_prefix("data: ") {
                    init_json = data.to_string();
                }
            }
            if !init_json.is_empty() {
                break;
            }
        }
        assert!(!init_json.is_empty(), "expected initialize response on SSE stream");
        let init_val: Value = serde_json::from_str(&init_json).expect("init response must be JSON");
        assert_eq!(init_val["jsonrpc"], "2.0");
        assert!(init_val.get("result").is_some(), "initialize must have result");
        assert_eq!(
            init_val["result"]["protocolVersion"],
            "2025-03-26",
            "protocolVersion must match"
        );

        // 5. POST tools/list.
        let tl_resp = client
            .post(format!(
                "http://127.0.0.1:{port}/messages/flow-slug?session_id={session_id}"
            ))
            .header("content-type", "application/json")
            .body(json!({"jsonrpc":"2.0","method":"tools/list","id":2}).to_string())
            .send()
            .await
            .unwrap();
        assert_eq!(tl_resp.status(), reqwest::StatusCode::ACCEPTED);

        // 6. Read tools/list response from SSE stream.
        let mut tl_json = String::new();
        while let Some(chunk) = body_stream.next().await {
            let text = String::from_utf8_lossy(&chunk.unwrap()).to_string();
            for line in text.lines() {
                if let Some(data) = line.strip_prefix("data: ") {
                    tl_json = data.to_string();
                }
            }
            if !tl_json.is_empty() {
                break;
            }
        }
        assert!(!tl_json.is_empty(), "expected tools/list response on SSE stream");
        let tl_val: Value =
            serde_json::from_str(&tl_json).expect("tools/list response must be JSON");
        assert_eq!(tl_val["jsonrpc"], "2.0");
        assert!(
            tl_val["result"]["tools"].is_array(),
            "tools/list result must have tools array"
        );
    }

    /// Shutdown flag causes GET /sse/:slug to return 503.
    #[tokio::test]
    async fn shutdown_returns_503() {
        let config = make_server_config(
            "Test",
            "shutdown-slug",
            simple_config_json("http://localhost"),
            None,
        );
        let state = make_sse_state(config);
        state.is_shutting_down.store(true, Ordering::SeqCst);
        let app = build_sse_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/sse/shutdown-slug")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
