//! Lightweight HTTP stub server that records all incoming requests.
// Testing utilities intentionally panic on setup failure — that is always a
// test configuration error, not a recoverable runtime condition.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//!
//! Bind a [`MockUpstream`] to an OS-assigned port, then point the system under
//! test at [`MockUpstream::url()`]. After the interaction, inspect captured
//! requests with [`MockUpstream::received_requests()`] or assert headers with
//! [`MockUpstream::assert_received_header()`].

use axum::{body::Body, extract::Request, response::Response, routing::any, Router};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::net::TcpListener;

/// A single HTTP request captured by the mock upstream.
#[derive(Debug, Clone)]
pub struct RecordedRequest {
    /// HTTP method (e.g. `"GET"`, `"POST"`).
    pub method: String,
    /// Request path (e.g. `"/weather"`).
    pub path: String,
    /// All request headers as lowercase name → value strings.
    pub headers: HashMap<String, String>,
    /// Raw request body bytes.
    pub body: Vec<u8>,
}

/// Shared state threaded into the axum handler closure.
#[derive(Clone)]
struct UpstreamState {
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    response_body: Arc<Mutex<String>>,
}

/// A lightweight in-process HTTP stub server.
///
/// Start it with [`MockUpstream::start()`], point the system under test at
/// [`MockUpstream::url()`], then inspect what was received.
///
/// The server is gracefully shut down when this value is dropped.
pub struct MockUpstream {
    /// The address the server is listening on.
    pub addr: std::net::SocketAddr,
    state: UpstreamState,
    /// Dropping the sender signals the server to shut down.
    _shutdown: tokio::sync::oneshot::Sender<()>,
}

impl MockUpstream {
    /// Starts the mock server on an OS-assigned port.
    ///
    /// # Panics
    ///
    /// Panics if the OS cannot allocate a port (extremely unlikely in tests).
    pub async fn start() -> Self {
        let state = UpstreamState {
            requests: Arc::new(Mutex::new(Vec::new())),
            response_body: Arc::new(Mutex::new(r#"{"ok":true}"#.to_string())),
        };

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("mock upstream: failed to bind port");
        let addr = listener
            .local_addr()
            .expect("mock upstream: failed to get local address");

        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let handler_state = state.clone();

        tokio::spawn(async move {
            let app = Router::new().route(
                "/*path",
                any(move |req: Request| {
                    let s = handler_state.clone();
                    async move {
                        let method = req.method().to_string();
                        let path = req.uri().path().to_string();
                        let headers: HashMap<String, String> = req
                            .headers()
                            .iter()
                            .filter_map(|(k, v)| {
                                v.to_str()
                                    .ok()
                                    .map(|v| (k.to_string(), v.to_string()))
                            })
                            .collect();
                        // Consume the body — limit to 1 MiB.
                        let body_bytes =
                            axum::body::to_bytes(req.into_body(), 1024 * 1024)
                                .await
                                .unwrap_or_default()
                                .to_vec();

                        let recorded = RecordedRequest {
                            method,
                            path,
                            headers,
                            body: body_bytes,
                        };
                        {
                            let mut guard = s
                                .requests
                                .lock()
                                .unwrap_or_else(|e| e.into_inner());
                            guard.push(recorded);
                        }

                        let body_str = s
                            .response_body
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .clone();

                        Response::builder()
                            .status(200)
                            .header("content-type", "application/json")
                            .body(Body::from(body_str))
                            .unwrap_or_else(|_| Response::new(Body::empty()))
                    }
                }),
            );

            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = rx.await;
                })
                .await
                .ok();
        });

        Self {
            addr,
            state,
            _shutdown: tx,
        }
    }

    /// Returns the base URL of this mock server (`http://127.0.0.1:{port}`).
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Replaces the fixed JSON response body returned for every request.
    ///
    /// Call this before exercising the system under test to inject a specific
    /// upstream response.
    pub fn set_response_body(&self, body: serde_json::Value) {
        let serialized =
            serde_json::to_string(&body).unwrap_or_else(|_| r#"{"ok":true}"#.to_string());
        let mut guard = self
            .state
            .response_body
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *guard = serialized;
    }

    /// Returns a snapshot of all requests received so far.
    pub fn received_requests(&self) -> Vec<RecordedRequest> {
        self.state
            .requests
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Asserts that at least one received request included the given header
    /// (case-insensitive name) with an exact value match.
    ///
    /// # Panics
    ///
    /// Panics with a diagnostic message if no matching request is found.
    pub fn assert_received_header(&self, name: &str, expected_value: &str) {
        let lower_name = name.to_lowercase();
        let requests = self.received_requests();
        let found = requests.iter().any(|req| {
            req.headers
                .get(&lower_name)
                .map(|v| v == expected_value)
                .unwrap_or(false)
        });
        assert!(
            found,
            "MockUpstream: expected header '{name}: {expected_value}' in at least one request.\n\
             Received {} request(s). Headers in last request: {:?}",
            requests.len(),
            requests.last().map(|r| &r.headers),
        );
    }

    /// Clears all recorded requests.
    pub fn clear_requests(&self) {
        let mut guard = self
            .state
            .requests
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        guard.clear();
    }
}
