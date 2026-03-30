//! Credential injector sidecar IPC and upstream execution pipeline.
//!
//! # IPC Protocol
//!
//! Framing: `[u32 big-endian byte count][UTF-8 JSON payload]`
//!
//! **Request** (gateway → sidecar):
//! ```json
//! {
//!   "server_id": "<uuid>",
//!   "request": {
//!     "method": "GET",
//!     "url": "https://api.example.com/v1/resource",
//!     "headers": { "Content-Type": "application/json" },
//!     "body": "<standard-base64>" | null
//!   }
//! }
//! ```
//!
//! **Response** (sidecar → gateway):
//! ```json
//! {
//!   "status": 200,
//!   "headers": { "content-type": "application/json" },
//!   "body": "<standard-base64>",
//!   "latency_ms": 42
//! }
//! ```
//!
//! The sidecar injects `Authorization` (or other auth) headers before forwarding
//! the request upstream. The gateway never sees credential values.
//!
//! For `auth_type = "none"` servers the gateway calls the upstream directly via
//! `reqwest::Client`, bypassing the sidecar entirely.

use crate::circuit_breaker::{CircuitBreakerRegistry, CircuitError};
use crate::upstream::{AuthType, UpstreamRequest};
use base64ct::{Base64, Encoding};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use thiserror::Error;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    sync::Mutex,
};
use uuid::Uuid;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Maximum concurrent sidecar connections in the pool.
const POOL_MAX: usize = 32;

/// Safety-net timeout for the full IPC round-trip (sidecar enforces upstream
/// timeout independently at its configured value).
const IPC_TIMEOUT_SECS: u64 = 35;

// ── IPC wire types ────────────────────────────────────────────────────────────

/// HTTP request forwarded to the credential injector sidecar.
#[derive(Debug, Serialize, Deserialize)]
struct IpcHttpRequest {
    method: String,
    url: String,
    headers: HashMap<String, String>,
    /// Standard base64-encoded body bytes, or `null` for bodyless requests.
    body: Option<String>,
}

/// Full IPC request envelope (gateway → sidecar).
#[derive(Debug, Serialize)]
struct IpcRequest {
    server_id: Uuid,
    request: IpcHttpRequest,
}

/// Response from the sidecar after credential injection + upstream call.
#[derive(Debug, Deserialize)]
struct IpcResponse {
    status: u16,
    headers: HashMap<String, String>,
    /// Standard base64-encoded response body bytes.
    body: String,
    latency_ms: u64,
}

// ── Public output types ───────────────────────────────────────────────────────

/// Decoded upstream HTTP response, ready for the transformation engine.
#[derive(Debug)]
pub struct UpstreamResponse {
    /// HTTP status code.
    pub status: u16,
    /// Decoded response body bytes.
    pub body: Vec<u8>,
    /// End-to-end latency in milliseconds.
    pub latency_ms: u64,
    /// Response headers (lower-cased names).
    pub headers: HashMap<String, String>,
}

// ── Error types ───────────────────────────────────────────────────────────────

/// Errors returned by [`UpstreamExecutor::execute`].
///
/// JSON-RPC code mapping (used by the router):
/// - [`CircuitOpen`]       → `-32004`
/// - [`SidecarUnavailable`] → `-32002`, `reason=credential_service_unreachable`
/// - [`Timeout`]           → `-32002`, `message=Upstream timeout`
/// - [`UpstreamError`]     → `-32003`
#[derive(Debug, Error, PartialEq)]
pub enum ExecuteError {
    /// Circuit breaker is open; fast-fail.
    #[error("circuit open; retry after {retry_after_ms}ms")]
    CircuitOpen {
        /// Milliseconds the caller should wait before retrying.
        retry_after_ms: u64,
    },

    /// Sidecar socket unreachable (not found, refused, or protocol error).
    #[error("credential service unreachable")]
    SidecarUnavailable,

    /// IPC or direct upstream request timed out (35 s safety net).
    #[error("upstream timeout")]
    Timeout,

    /// Upstream returned a non-2xx HTTP status.
    #[error("upstream error: HTTP {status}")]
    UpstreamError {
        /// The HTTP status code returned by the upstream.
        status: u16,
    },
}

// ── Connection pool ───────────────────────────────────────────────────────────

/// A checked-out Unix socket connection.
///
/// Returns to the pool automatically on `Drop` unless [`discard`] was called.
///
/// [`discard`]: PooledConnection::discard
pub struct PooledConnection {
    stream: Option<UnixStream>,
    return_tx: tokio::sync::mpsc::UnboundedSender<UnixStream>,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

impl PooledConnection {
    /// Mutable access to the underlying socket.
    ///
    /// # Panics
    ///
    /// Panics if called after [`discard`].
    ///
    /// [`discard`]: PooledConnection::discard
    pub fn stream_mut(&mut self) -> &mut UnixStream {
        // SAFETY: stream is always Some unless discard() was called first.
        // This invariant is maintained by the module; external callers do not
        // call stream_mut() after discard().
        #[allow(clippy::expect_used)]
        self.stream
            .as_mut()
            .expect("pooled connection already discarded")
    }

    /// Prevent this connection from being returned to the pool on `Drop`.
    /// Call after any I/O error on the stream.
    pub fn discard(&mut self) {
        self.stream = None;
    }
}

impl Drop for PooledConnection {
    fn drop(&mut self) {
        if let Some(stream) = self.stream.take() {
            // Best-effort return; silently drop if channel is closed.
            let _ = self.return_tx.send(stream);
        }
        // _permit drops here, releasing one semaphore slot.
    }
}

/// Unix domain socket connection pool for the credential injector sidecar.
///
/// Connections are created lazily, up to [`POOL_MAX`] concurrently.
/// Healthy connections are returned to the idle pool; broken ones are discarded.
pub struct SidecarPool {
    socket_path: PathBuf,
    idle_tx: tokio::sync::mpsc::UnboundedSender<UnixStream>,
    idle_rx: Mutex<tokio::sync::mpsc::UnboundedReceiver<UnixStream>>,
    semaphore: Arc<tokio::sync::Semaphore>,
}

impl SidecarPool {
    /// Create a new pool pointing at `socket_path`.
    pub fn new(socket_path: PathBuf) -> Arc<Self> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        Arc::new(Self {
            socket_path,
            idle_tx: tx,
            idle_rx: Mutex::new(rx),
            semaphore: Arc::new(tokio::sync::Semaphore::new(POOL_MAX)),
        })
    }

    /// Check whether the sidecar socket is reachable.
    ///
    /// Attempts a new TCP/Unix connection to the socket with a 200 ms timeout.
    /// Returns `true` if the connection succeeds (even if immediately closed).
    /// This is used by the `/healthz/ready` probe.
    pub async fn is_healthy(&self) -> bool {
        let path = self.socket_path.clone();
        let connect = tokio::net::UnixStream::connect(&path);
        matches!(
            tokio::time::timeout(Duration::from_millis(200), connect).await,
            Ok(Ok(_))
        )
    }

    /// Acquire a connection. Reuses an idle one or opens a new socket.
    ///
    /// Blocks when `POOL_MAX` connections are already in use.
    pub async fn acquire(&self) -> Result<PooledConnection, ExecuteError> {
        let permit = Arc::clone(&self.semaphore)
            .acquire_owned()
            .await
            .map_err(|_| ExecuteError::SidecarUnavailable)?;

        // Try to reuse an idle connection first.
        let maybe_idle = {
            let mut rx = self.idle_rx.lock().await;
            rx.try_recv().ok()
        };

        let stream = match maybe_idle {
            Some(s) => s,
            None => UnixStream::connect(&self.socket_path).await.map_err(|e| {
                tracing::warn!(
                    socket_path = %self.socket_path.display(),
                    error = %e,
                    "sidecar socket connection failed"
                );
                ExecuteError::SidecarUnavailable
            })?,
        };

        Ok(PooledConnection {
            stream: Some(stream),
            return_tx: self.idle_tx.clone(),
            _permit: permit,
        })
    }
}

// ── IPC framing ───────────────────────────────────────────────────────────────

/// Write a length-prefixed frame: `[u32 BE length][payload]`.
async fn send_framed(stream: &mut UnixStream, payload: &[u8]) -> std::io::Result<()> {
    let len = u32::try_from(payload.len()).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "IPC payload too large")
    })?;
    stream.write_all(&len.to_be_bytes()).await?;
    stream.write_all(payload).await?;
    Ok(())
}

/// Read a length-prefixed frame: `[u32 BE length][payload]`.
async fn recv_framed(stream: &mut UnixStream) -> std::io::Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await?;
    Ok(buf)
}

/// Send a request frame then receive a response frame.
async fn send_recv(stream: &mut UnixStream, payload: &[u8]) -> std::io::Result<Vec<u8>> {
    send_framed(stream, payload).await?;
    recv_framed(stream).await
}

// ── UpstreamExecutor ──────────────────────────────────────────────────────────

/// Routes upstream calls through the credential injector sidecar (auth required)
/// or directly via `reqwest` (`auth_type = none`).
/// Integrates with the per-server circuit breaker on every call.
pub struct UpstreamExecutor {
    pool: Arc<SidecarPool>,
    http_client: reqwest::Client,
    circuit_registry: Arc<CircuitBreakerRegistry>,
}

impl UpstreamExecutor {
    /// Create a new executor.
    pub fn new(
        pool: Arc<SidecarPool>,
        http_client: reqwest::Client,
        circuit_registry: Arc<CircuitBreakerRegistry>,
    ) -> Self {
        Self {
            pool,
            http_client,
            circuit_registry,
        }
    }

    /// Execute one upstream call.
    ///
    /// Steps:
    /// 1. Check circuit breaker; return [`ExecuteError::CircuitOpen`] if open.
    /// 2. Route: `auth_type=None` → direct reqwest; else → sidecar IPC.
    /// 3. Notify circuit breaker of the outcome.
    pub async fn execute(
        &self,
        server_id: Uuid,
        req: UpstreamRequest,
        auth_type: &AuthType,
    ) -> Result<UpstreamResponse, ExecuteError> {
        // Step 1: circuit breaker check.
        let breaker = self.circuit_registry.get(server_id);
        if let Err(CircuitError::Open { retry_after_ms }) = breaker.check() {
            return Err(ExecuteError::CircuitOpen { retry_after_ms });
        }

        let log_path = url_path_only(&req.url);

        // Step 2: dispatch.
        let result = match auth_type {
            AuthType::None => self.execute_direct(server_id, req, &log_path).await,
            _ => self.execute_via_sidecar(server_id, req, &log_path).await,
        };

        // Step 3: update circuit breaker.
        match &result {
            Ok(_) => breaker.on_success(),
            Err(ExecuteError::SidecarUnavailable) | Err(ExecuteError::Timeout) => {
                breaker.on_failure(None);
            }
            Err(ExecuteError::UpstreamError { status }) => {
                if *status >= 500 || *status == 429 {
                    breaker.on_failure(None);
                }
                // 4xx (non-429) do not affect the circuit breaker.
            }
            Err(ExecuteError::CircuitOpen { .. }) => {
                // Already short-circuited; no update needed.
            }
        }

        result
    }

    // ── Direct path (auth_type = none) ────────────────────────────────────────

    async fn execute_direct(
        &self,
        server_id: Uuid,
        req: UpstreamRequest,
        log_path: &str,
    ) -> Result<UpstreamResponse, ExecuteError> {
        let start = Instant::now();

        let method = reqwest::Method::from_bytes(req.method.as_bytes())
            .unwrap_or(reqwest::Method::GET);

        let mut rb = self
            .http_client
            .request(method, &req.url)
            .timeout(req.timeout);

        for (k, v) in &req.headers {
            rb = rb.header(k.as_str(), v.as_str());
        }

        if let Some(body) = &req.body {
            let bytes = serde_json::to_vec(body).unwrap_or_default();
            rb = rb.body(bytes);
        }

        let resp =
            tokio::time::timeout(Duration::from_secs(IPC_TIMEOUT_SECS), rb.send())
                .await
                .map_err(|_| {
                    tracing::warn!(
                        server_id = %server_id,
                        url_path = %log_path,
                        "direct upstream request timed out"
                    );
                    ExecuteError::Timeout
                })?
                .map_err(|e| {
                    tracing::warn!(
                        server_id = %server_id,
                        url_path = %log_path,
                        error = %e,
                        "direct upstream request failed"
                    );
                    ExecuteError::Timeout
                })?;

        let latency_ms = start.elapsed().as_millis() as u64;
        let status = resp.status().as_u16();

        let headers: HashMap<String, String> = resp
            .headers()
            .iter()
            .filter_map(|(k, v)| v.to_str().ok().map(|vs| (k.as_str().to_string(), vs.to_string())))
            .collect();

        let body = resp.bytes().await.map(|b| b.to_vec()).unwrap_or_default();

        tracing::info!(
            server_id = %server_id,
            url_path = %log_path,
            status = status,
            latency_ms = latency_ms,
            "direct upstream call complete"
        );

        if (200..300).contains(&status) {
            Ok(UpstreamResponse {
                status,
                body,
                latency_ms,
                headers,
            })
        } else {
            Err(ExecuteError::UpstreamError { status })
        }
    }

    // ── Sidecar path (auth required) ──────────────────────────────────────────

    async fn execute_via_sidecar(
        &self,
        server_id: Uuid,
        req: UpstreamRequest,
        log_path: &str,
    ) -> Result<UpstreamResponse, ExecuteError> {
        let start = Instant::now();

        // Encode body as base64 (null when absent).
        let body_b64 = req.body.as_ref().map(|b| {
            Base64::encode_string(&serde_json::to_vec(b).unwrap_or_default())
        });

        let ipc_req = IpcRequest {
            server_id,
            request: IpcHttpRequest {
                method: req.method.clone(),
                url: req.url.clone(),
                headers: req.headers.clone(),
                body: body_b64,
            },
        };

        let payload = serde_json::to_vec(&ipc_req).map_err(|e| {
            tracing::warn!(error = %e, "failed to serialize IPC request");
            ExecuteError::SidecarUnavailable
        })?;

        // Acquire a connection from the pool.
        let mut conn = self.pool.acquire().await?;

        // Send + receive with a 35-second safety-net timeout.
        let ipc_result = tokio::time::timeout(
            Duration::from_secs(IPC_TIMEOUT_SECS),
            send_recv(conn.stream_mut(), &payload),
        )
        .await;

        let resp_bytes = match ipc_result {
            Err(_elapsed) => {
                conn.discard();
                tracing::warn!(
                    server_id = %server_id,
                    url_path = %log_path,
                    "sidecar IPC request timed out"
                );
                return Err(ExecuteError::Timeout);
            }
            Ok(Err(e)) => {
                conn.discard();
                tracing::warn!(
                    server_id = %server_id,
                    url_path = %log_path,
                    error = %e,
                    "sidecar IPC I/O error"
                );
                return Err(ExecuteError::SidecarUnavailable);
            }
            Ok(Ok(bytes)) => bytes,
        };

        let _elapsed_ms = start.elapsed().as_millis() as u64;

        let ipc_resp: IpcResponse = match serde_json::from_slice(&resp_bytes) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "failed to deserialize sidecar IPC response");
                conn.discard();
                return Err(ExecuteError::SidecarUnavailable);
            }
        };

        let body = Base64::decode_vec(&ipc_resp.body).unwrap_or_default();

        tracing::info!(
            server_id = %server_id,
            url_path = %log_path,
            status = ipc_resp.status,
            latency_ms = ipc_resp.latency_ms,
            "sidecar IPC call complete"
        );

        if (200..300).contains(&ipc_resp.status) {
            Ok(UpstreamResponse {
                status: ipc_resp.status,
                body,
                latency_ms: ipc_resp.latency_ms,
                headers: ipc_resp.headers,
            })
        } else {
            Err(ExecuteError::UpstreamError {
                status: ipc_resp.status,
            })
        }
    }
}

// ── URL helpers ───────────────────────────────────────────────────────────────

/// Extract only the path component from a URL, discarding query string and fragment.
///
/// Examples:
/// - `"https://api.example.com/users/42?token=secret"` → `"/users/42"`
/// - `"https://api.example.com"` → `""`
///
/// Safe to include in structured logs.
pub fn url_path_only(url: &str) -> String {
    // Skip past `scheme://host`
    let after_scheme = url.find("://").map(|i| i + 3).unwrap_or(0);
    let rest = &url[after_scheme..];
    let path_start = rest
        .find('/')
        .map(|i| after_scheme + i)
        .unwrap_or(url.len());
    let path_end = url[path_start..]
        .find(['?', '#'])
        .map(|i| path_start + i)
        .unwrap_or(url.len());
    url[path_start..path_end].to_string()
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
    use crate::circuit_breaker::CircuitBreakerRegistry;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use std::time::Duration;

    fn make_upstream_req(url: &str) -> UpstreamRequest {
        UpstreamRequest {
            method: "GET".to_string(),
            url: url.to_string(),
            headers: HashMap::new(),
            body: None,
            timeout: Duration::from_secs(5),
        }
    }

    // ── url_path_only ─────────────────────────────────────────────────────────

    #[test]
    fn path_only_strips_query() {
        assert_eq!(
            url_path_only("https://api.example.com/users/42?token=secret&other=val"),
            "/users/42"
        );
    }

    #[test]
    fn path_only_strips_fragment() {
        assert_eq!(
            url_path_only("https://api.example.com/path#section"),
            "/path"
        );
    }

    #[test]
    fn path_only_no_path() {
        assert_eq!(url_path_only("https://api.example.com"), "");
    }

    #[test]
    fn path_only_with_port() {
        assert_eq!(
            url_path_only("http://localhost:8080/api/v1/data?key=abc"),
            "/api/v1/data"
        );
    }

    // ── IPC framing roundtrip ─────────────────────────────────────────────────

    #[tokio::test]
    async fn framing_roundtrip() {
        let tmp = format!("/tmp/mcp_framing_{}.sock", uuid::Uuid::new_v4().simple());
        let listener = tokio::net::UnixListener::bind(&tmp).unwrap();
        let payload = b"hello from gateway".to_vec();
        let payload_clone = payload.clone();

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            recv_framed(&mut stream).await.unwrap()
        });

        let mut client = UnixStream::connect(&tmp).await.unwrap();
        send_framed(&mut client, &payload_clone).await.unwrap();

        let received = server.await.unwrap();
        assert_eq!(received, payload);
        let _ = std::fs::remove_file(tmp);
    }

    // ── Sidecar unavailable → SidecarUnavailable ──────────────────────────────

    #[tokio::test]
    async fn sidecar_unavailable_returns_error() {
        let pool =
            SidecarPool::new(PathBuf::from("/tmp/nonexistent_mcp_sidecar_xyz_999.sock"));
        let registry = Arc::new(CircuitBreakerRegistry::new());
        let executor =
            UpstreamExecutor::new(pool, reqwest::Client::new(), Arc::clone(&registry));

        let server_id = uuid::Uuid::new_v4();
        let req = make_upstream_req("https://api.example.com/test");
        let result = executor.execute(server_id, req, &AuthType::Bearer).await;

        assert!(
            matches!(result, Err(ExecuteError::SidecarUnavailable)),
            "expected SidecarUnavailable, got: {result:?}"
        );
    }

    // ── auth_type=none bypasses sidecar ───────────────────────────────────────

    #[tokio::test]
    async fn auth_type_none_does_not_contact_sidecar() {
        // Spin up a fake sidecar listener and count connections.
        let tmp = format!("/tmp/mcp_none_test_{}.sock", uuid::Uuid::new_v4().simple());
        let listener = tokio::net::UnixListener::bind(&tmp).unwrap();
        let conn_count = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&conn_count);

        let server = tokio::spawn(async move {
            while let Ok(_) = listener.accept().await {
                counter.fetch_add(1, Ordering::SeqCst);
            }
        });

        let pool = SidecarPool::new(PathBuf::from(&tmp));
        let registry = Arc::new(CircuitBreakerRegistry::new());
        let executor =
            UpstreamExecutor::new(pool, reqwest::Client::new(), Arc::clone(&registry));

        let server_id = uuid::Uuid::new_v4();
        // URL points at nothing; the call will fail, but that's fine.
        let req = make_upstream_req("http://127.0.0.1:19999/test");
        let _result = executor.execute(server_id, req, &AuthType::None).await;

        assert_eq!(
            conn_count.load(Ordering::SeqCst),
            0,
            "sidecar socket must not be contacted for auth_type=none"
        );

        server.abort();
        let _ = std::fs::remove_file(tmp);
    }

    // ── Mock sidecar: correct IPC structure + response forwarding ─────────────

    #[tokio::test]
    async fn mock_sidecar_correct_structure_and_forwarding() {
        use std::sync::atomic::AtomicBool;

        let tmp = format!("/tmp/mcp_sidecar_mock_{}.sock", uuid::Uuid::new_v4().simple());
        let listener = tokio::net::UnixListener::bind(&tmp).unwrap();
        let verified = Arc::new(AtomicBool::new(false));
        let verified_clone = Arc::clone(&verified);

        // Mock sidecar: accept one connection, verify request, return 200.
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();

            let bytes = recv_framed(&mut stream).await.unwrap();
            let req_val: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

            // Structural assertions on the IPC request.
            assert!(req_val["server_id"].is_string());
            assert_eq!(req_val["request"]["method"], "GET");
            assert!(req_val["request"]["url"]
                .as_str()
                .unwrap()
                .contains("/resource"));
            assert!(req_val["request"]["body"].is_null());

            verified_clone.store(true, Ordering::SeqCst);

            // Return HTTP 200 with a JSON body.
            let body_bytes = b"{\"id\":1}";
            let resp = serde_json::json!({
                "status": 200u16,
                "headers": { "content-type": "application/json" },
                "body": Base64::encode_string(body_bytes),
                "latency_ms": 3u64,
            });
            let resp_bytes = serde_json::to_vec(&resp).unwrap();
            send_framed(&mut stream, &resp_bytes).await.unwrap();
        });

        let pool = SidecarPool::new(PathBuf::from(&tmp));
        let registry = Arc::new(CircuitBreakerRegistry::new());
        let executor =
            UpstreamExecutor::new(pool, reqwest::Client::new(), Arc::clone(&registry));

        let server_id = uuid::Uuid::new_v4();
        let req = make_upstream_req("https://api.example.com/resource");
        let result = executor.execute(server_id, req, &AuthType::Bearer).await;

        assert!(result.is_ok(), "expected Ok, got: {result:?}");
        let resp = result.unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, b"{\"id\":1}");
        assert!(
            verified.load(Ordering::SeqCst),
            "mock sidecar structural verification did not run"
        );

        let _ = std::fs::remove_file(tmp);
    }

    // ── Circuit open: no sidecar call made ───────────────────────────────────

    #[tokio::test]
    async fn circuit_open_short_circuits_before_sidecar() {
        let tmp = format!("/tmp/mcp_circuit_{}.sock", uuid::Uuid::new_v4().simple());
        let listener = tokio::net::UnixListener::bind(&tmp).unwrap();
        let conn_count = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&conn_count);

        tokio::spawn(async move {
            while let Ok(_) = listener.accept().await {
                counter.fetch_add(1, Ordering::SeqCst);
            }
        });

        let pool = SidecarPool::new(PathBuf::from(&tmp));
        let registry = Arc::new(CircuitBreakerRegistry::new());
        let executor =
            UpstreamExecutor::new(pool, reqwest::Client::new(), Arc::clone(&registry));

        let server_id = uuid::Uuid::new_v4();
        // Force circuit open by simulating 5 failures.
        let breaker = registry.get(server_id);
        for _ in 0..5 {
            breaker.on_failure(None);
        }

        let req = make_upstream_req("https://api.example.com/test");
        let result = executor.execute(server_id, req, &AuthType::Bearer).await;

        assert!(
            matches!(result, Err(ExecuteError::CircuitOpen { .. })),
            "expected CircuitOpen, got: {result:?}"
        );
        assert_eq!(
            conn_count.load(Ordering::SeqCst),
            0,
            "no sidecar connections should occur when circuit is open"
        );

        let _ = std::fs::remove_file(tmp);
    }

    // ── Sidecar 4xx: UpstreamError, circuit breaker NOT incremented ───────────

    #[tokio::test]
    async fn sidecar_4xx_does_not_open_circuit() {
        let tmp = format!("/tmp/mcp_4xx_{}.sock", uuid::Uuid::new_v4().simple());
        let listener = tokio::net::UnixListener::bind(&tmp).unwrap();

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            // Consume the request frame
            recv_framed(&mut stream).await.unwrap();
            // Respond with 404
            let resp = serde_json::json!({
                "status": 404u16,
                "headers": {},
                "body": Base64::encode_string(b"not found"),
                "latency_ms": 1u64,
            });
            let bytes = serde_json::to_vec(&resp).unwrap();
            send_framed(&mut stream, &bytes).await.unwrap();
        });

        let pool = SidecarPool::new(PathBuf::from(&tmp));
        let registry = Arc::new(CircuitBreakerRegistry::new());
        let executor =
            UpstreamExecutor::new(pool, reqwest::Client::new(), Arc::clone(&registry));

        let server_id = uuid::Uuid::new_v4();
        let req = make_upstream_req("https://api.example.com/missing");
        let result = executor.execute(server_id, req, &AuthType::Bearer).await;

        assert!(matches!(result, Err(ExecuteError::UpstreamError { status: 404 })));

        // Circuit breaker should still be closed (0 failures for 4xx).
        let breaker = registry.get(server_id);
        assert_eq!(
            breaker.consecutive_failures(),
            0,
            "4xx must not increment circuit breaker failure count"
        );

        let _ = std::fs::remove_file(tmp);
    }

    // ── Sidecar 5xx: UpstreamError, circuit breaker incremented ──────────────

    #[tokio::test]
    async fn sidecar_5xx_increments_circuit_breaker() {
        let tmp = format!("/tmp/mcp_5xx_{}.sock", uuid::Uuid::new_v4().simple());
        let listener = tokio::net::UnixListener::bind(&tmp).unwrap();

        // Handle all 5 requests on the SAME persistent connection (pool reuse).
        tokio::spawn(async move {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            for _ in 0..5_usize {
                let Ok(_req_bytes) = recv_framed(&mut stream).await else {
                    break;
                };
                let resp = serde_json::json!({
                    "status": 500u16,
                    "headers": {},
                    "body": Base64::encode_string(b"error"),
                    "latency_ms": 1u64,
                });
                let bytes = serde_json::to_vec(&resp).unwrap();
                if send_framed(&mut stream, &bytes).await.is_err() {
                    break;
                }
            }
        });

        let pool = SidecarPool::new(PathBuf::from(&tmp));
        let registry = Arc::new(CircuitBreakerRegistry::new());
        let executor =
            UpstreamExecutor::new(pool, reqwest::Client::new(), Arc::clone(&registry));

        let server_id = uuid::Uuid::new_v4();

        for i in 1..=5_u32 {
            let req = make_upstream_req("https://api.example.com/data");
            let result = executor.execute(server_id, req, &AuthType::Bearer).await;
            assert!(
                matches!(result, Err(ExecuteError::UpstreamError { status: 500 })),
                "iteration {i}: expected UpstreamError 500, got: {result:?}"
            );

            if i < 5 {
                let breaker = registry.get(server_id);
                assert_eq!(
                    breaker.consecutive_failures(),
                    i,
                    "after {i} failures, expected {i} consecutive failures"
                );
            }
        }

        // After 5 failures the circuit should be open.
        let breaker = registry.get(server_id);
        assert_eq!(
            breaker.state(),
            crate::circuit_breaker::BreakerState::Open,
            "circuit must be open after 5 consecutive 5xx responses"
        );

        let _ = std::fs::remove_file(tmp);
    }
}
