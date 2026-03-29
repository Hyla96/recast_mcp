//! Minimal MCP JSON-RPC client for driving integration tests.
// Testing utilities intentionally use unwrap on header construction — invalid
// header values in tests are always a programmer error.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//!
//! [`TestMcpClient`] wraps `reqwest` and exposes typed helpers for the three
//! MCP methods used during testing: `initialize`, `tools/list`, `tools/call`.
//! All methods return the raw [`mcp_protocol::JsonRpcResponse`] so tests can
//! inspect the `result` or `error` fields directly.

use mcp_protocol::{JsonRpcRequest, JsonRpcResponse};
use std::sync::atomic::{AtomicU64, Ordering};

/// Error returned by [`TestMcpClient`] operations.
#[derive(Debug, thiserror::Error)]
pub enum McpClientError {
    /// HTTP transport error (connection refused, timeout, etc.).
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
}

/// A minimal MCP JSON-RPC client for use in integration tests.
///
/// Sends requests to the provided `base_url` using `POST` with
/// `Content-Type: application/json`. Auto-increments request IDs.
///
/// # Example
///
/// ```rust,no_run
/// # use mcp_common::testing::TestMcpClient;
/// # #[tokio::main]
/// # async fn main() {
/// let client = TestMcpClient::new("http://localhost:3000/rpc/my-server");
/// let resp = client.initialize().await.unwrap();
/// assert!(resp.error.is_none());
/// # }
/// ```
pub struct TestMcpClient {
    base_url: String,
    client: reqwest::Client,
    next_id: AtomicU64,
}

impl TestMcpClient {
    /// Creates a new client pointing at `base_url`.
    ///
    /// `base_url` should be the full endpoint URL including path (e.g.
    /// `http://localhost:3000/rpc/my-slug`).
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            client: reqwest::Client::new(),
            next_id: AtomicU64::new(1),
        }
    }

    /// Creates a new client that sends an `Authorization: Bearer <token>` header
    /// on every request.
    pub fn with_bearer_token(base_url: impl Into<String>, token: &str) -> Self {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Ok(val) = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}")) {
            headers.insert(reqwest::header::AUTHORIZATION, val);
        }
        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .unwrap_or_default();
        Self {
            base_url: base_url.into(),
            client,
            next_id: AtomicU64::new(1),
        }
    }

    /// Sends an `initialize` request.
    ///
    /// # Errors
    ///
    /// Returns [`McpClientError`] on transport or deserialization failure.
    pub async fn initialize(&self) -> Result<JsonRpcResponse, McpClientError> {
        self.send_request(
            "initialize",
            Some(serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "test-client", "version": "0.0.0"}
            })),
        )
        .await
    }

    /// Sends a `tools/list` request.
    ///
    /// # Errors
    ///
    /// Returns [`McpClientError`] on transport or deserialization failure.
    pub async fn tools_list(&self) -> Result<JsonRpcResponse, McpClientError> {
        self.send_request("tools/list", None).await
    }

    /// Sends a `tools/call` request.
    ///
    /// # Errors
    ///
    /// Returns [`McpClientError`] on transport or deserialization failure.
    pub async fn tools_call(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<JsonRpcResponse, McpClientError> {
        self.send_request(
            "tools/call",
            Some(serde_json::json!({
                "name": name,
                "arguments": arguments
            })),
        )
        .await
    }

    /// Sends an arbitrary JSON-RPC request and returns the parsed response.
    ///
    /// # Errors
    ///
    /// Returns [`McpClientError`] on transport or deserialization failure.
    pub async fn send_request(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<JsonRpcResponse, McpClientError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params,
            id: Some(serde_json::json!(id)),
        };
        let resp = self
            .client
            .post(&self.base_url)
            .json(&req)
            .send()
            .await?
            .json::<JsonRpcResponse>()
            .await?;
        Ok(resp)
    }
}
