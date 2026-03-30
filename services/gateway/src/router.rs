//! Gateway request router.
//!
//! Dispatches parsed JSON-RPC 2.0 requests to the correct handler based on
//! method. Both the Streamable HTTP and SSE transports share this single
//! routing function.
//!
//! # Dispatch logic
//!
//! 1. Resolve the server slug → `Arc<ServerConfig>` from the in-memory cache.
//!    Unknown slug → JSON-RPC error `{code:-32001, message:"Server not found"}`.
//! 2. Match on method:
//!    - `initialize`  → server capabilities + `serverInfo`
//!    - `initialized` → no-op, `{jsonrpc:"2.0", id:null}`
//!    - `ping`        → `{jsonrpc:"2.0", id:…, result:{}}`
//!    - `tools/list`  → tool schemas from [`SchemaCache`]
//!    - `tools/call`  → upstream pipeline (S-026 → S-027 → S-028)
//!    - anything else → `-32601 Method not found`
//!
//! # Upstream pipeline for `tools/call`
//!
//! S-026: [`UpstreamRequestBuilder::build`] — pure, converts tool call args to
//!        `UpstreamRequest` (URL, method, headers, body).
//! S-027: [`UpstreamExecutor::execute`] — dispatches to sidecar IPC or direct
//!        `reqwest::Client`; circuit breaker checked before and after.
//! S-028: [`TransformPipeline::apply`] — applies declarative transforms and
//!        wraps the result as `[{type:"text", text:"<json>"}]`.
//!
//! # Thread safety
//!
//! `Router` is `Clone`-able via `Arc` fields; call `Arc::new(Router::new(…))`
//! to share across request tasks.

use crate::cache::{ConfigCache, ServerConfig};
use crate::logging::{sanitise_url, RequestLogger};
use crate::protocol::jsonrpc::ParsedRequest;
use crate::sidecar::{ExecuteError, UpstreamExecutor};
use crate::tool_schema::{SchemaCache, ToolsListResult};
use crate::transform::{TransformPipeline, TransformPipelineConfig};
use crate::upstream::{GatewayConfig, ToolCallParams, UpstreamRequestBuilder};
use mcp_protocol::{error_codes, JsonRpcError, JsonRpcResponse};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Instant;

// ── Gateway-specific JSON-RPC error codes ──────────────────────────────────────

/// Server slug is not present in the config cache.
pub const CODE_SERVER_NOT_FOUND: i32 = -32001;
/// Upstream unavailable (sidecar unreachable or request timed out).
pub const CODE_UPSTREAM_UNAVAILABLE: i32 = -32002;
/// Upstream returned a non-2xx HTTP status.
pub const CODE_UPSTREAM_ERROR: i32 = -32003;
/// Circuit breaker is open; fast-fail without calling upstream.
pub const CODE_CIRCUIT_OPEN: i32 = -32004;

// ── UpstreamPipeline ──────────────────────────────────────────────────────────

/// Bundles the three stages of the upstream call pipeline:
///
/// - S-026 [`UpstreamRequestBuilder`] — converts a `tools/call` and server
///   config into a ready-to-send `UpstreamRequest`.
/// - S-027 [`UpstreamExecutor`] — executes via sidecar IPC or direct reqwest
///   (for `auth_type = "none"` servers), with circuit breaker integration.
/// - S-028 `TransformPipeline` — applied per-request to the raw response body.
///
/// Wrap in `Arc` and share across request tasks.
pub struct UpstreamPipeline {
    /// Executor: dispatches to sidecar IPC or direct reqwest.
    pub executor: Arc<UpstreamExecutor>,
    /// Request builder: converts tool call + config → HTTP request.
    pub request_builder: UpstreamRequestBuilder,
}

impl UpstreamPipeline {
    /// Construct a new pipeline and wrap it in `Arc`.
    pub fn new(
        executor: Arc<UpstreamExecutor>,
        request_builder: UpstreamRequestBuilder,
    ) -> Arc<Self> {
        Arc::new(Self {
            executor,
            request_builder,
        })
    }
}

// ── Internal logging metadata ─────────────────────────────────────────────────

/// Logging metadata captured during a `tools/call` execution.
///
/// Fields are populated incrementally as execution progresses; earlier errors
/// leave downstream fields as `None`.
#[derive(Default)]
struct ToolsCallLogMeta {
    tool_name: Option<String>,
    upstream_url: Option<String>,
    upstream_status: Option<u16>,
    upstream_latency_ms: Option<u64>,
    response_size_bytes: Option<usize>,
    transform_warnings: Vec<String>,
}

/// Return value from [`Router::handle_tools_call`].
///
/// Bundles the JSON-RPC result (success or error) with logging metadata so the
/// caller can emit a structured log record.
struct ToolsCallOutcome {
    result: Result<Value, JsonRpcError>,
    meta: ToolsCallLogMeta,
}

impl ToolsCallOutcome {
    fn error(meta: ToolsCallLogMeta, err: JsonRpcError) -> Self {
        Self { result: Err(err), meta }
    }
}

// ── Router ────────────────────────────────────────────────────────────────────

/// Config-driven JSON-RPC request dispatcher.
///
/// Holds `Arc<ConfigCache>` (for O(1) slug → config lookup) and
/// `Arc<UpstreamPipeline>` (for `tools/call` execution). Both are
/// `Send + Sync` and safe to share across request-handler tasks.
pub struct Router {
    cache: Arc<ConfigCache>,
    upstream: Arc<UpstreamPipeline>,
    schema_cache: Arc<SchemaCache>,
    logger: Arc<RequestLogger>,
}

impl Router {
    /// Create a new router.
    ///
    /// A fresh [`SchemaCache`] is allocated internally; it is shared for the
    /// lifetime of this `Router`.
    pub fn new(
        cache: Arc<ConfigCache>,
        upstream: Arc<UpstreamPipeline>,
        logger: Arc<RequestLogger>,
    ) -> Self {
        Self {
            cache,
            upstream,
            schema_cache: Arc::new(SchemaCache::new()),
            logger,
        }
    }

    /// Dispatch a parsed JSON-RPC request for the given server slug.
    ///
    /// Always returns a `JsonRpcResponse`; callers are responsible for
    /// filtering out responses for notifications if needed (e.g. `initialized`
    /// returns `{jsonrpc:"2.0", id:null}` per acceptance criteria rather than
    /// being silently dropped).
    ///
    /// `token_prefix` is the first 8 characters of the bearer token used to
    /// authenticate the caller, safe to include in logs. Pass `None` when no
    /// token was presented (e.g. during tests or unauthenticated endpoints).
    pub async fn dispatch(
        &self,
        slug: &str,
        request: ParsedRequest,
        token_prefix: Option<String>,
    ) -> JsonRpcResponse {
        let start = Instant::now();
        let id = request.id.clone();

        // Resolve slug → server config (synchronous O(1) DashMap + moka reads).
        let config = match self.resolve_slug(slug) {
            Some(c) => c,
            None => {
                return make_error_response(id, CODE_SERVER_NOT_FOUND, "Server not found", None);
            }
        };

        match request.method.as_str() {
            "initialize" => {
                let resp = make_ok_response(id, self.handle_initialize(&config));
                if self.logger.is_debug_enabled() {
                    let record = self.logger.new_record(
                        config.id,
                        slug,
                        "initialize",
                        start.elapsed().as_millis() as u64,
                    );
                    self.logger.log(record);
                }
                resp
            }
            "initialized" => {
                // No-op; return {jsonrpc:'2.0', id:null} as specified.
                // Not logged (no meaningful observability value).
                JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    result: None,
                    error: None,
                    id: Some(Value::Null),
                }
            }
            "ping" => {
                let resp = make_ok_response(id, json!({}));
                if self.logger.is_debug_enabled() {
                    let record = self.logger.new_record(
                        config.id,
                        slug,
                        "ping",
                        start.elapsed().as_millis() as u64,
                    );
                    self.logger.log(record);
                }
                resp
            }
            "tools/list" => {
                let result = self.handle_tools_list(&config);
                let latency_ms = start.elapsed().as_millis() as u64;
                let mut record =
                    self.logger.new_record(config.id, slug, "tools/list", latency_ms);
                record.token_prefix = token_prefix;
                self.logger.log(record);
                make_ok_response(id, result)
            }
            "tools/call" => {
                let outcome = self.handle_tools_call(&config, request.params).await;
                let latency_ms = start.elapsed().as_millis() as u64;

                let mut record =
                    self.logger.new_record(config.id, slug, "tools/call", latency_ms);
                record.tool_name = outcome.meta.tool_name;
                record.upstream_url = outcome.meta.upstream_url;
                record.upstream_status = outcome.meta.upstream_status;
                record.upstream_latency_ms = outcome.meta.upstream_latency_ms;
                record.response_size_bytes = outcome.meta.response_size_bytes;
                record.transform_warnings = outcome.meta.transform_warnings;
                record.token_prefix = token_prefix;
                self.logger.log(record);

                match outcome.result {
                    Ok(v) => make_ok_response(id, v),
                    Err(err) => JsonRpcResponse {
                        jsonrpc: "2.0".to_string(),
                        result: None,
                        error: Some(err),
                        id,
                    },
                }
            }
            _ => make_error_response(id, error_codes::METHOD_NOT_FOUND, "Method not found", None),
        }
    }

    // ── Slug resolution ───────────────────────────────────────────────────────

    fn resolve_slug(&self, slug: &str) -> Option<Arc<ServerConfig>> {
        let id = self.cache.slug_to_id(slug)?;
        self.cache.get(id)
    }

    // ── Method handlers ───────────────────────────────────────────────────────

    fn handle_initialize(&self, config: &ServerConfig) -> Value {
        json!({
            "protocolVersion": "2025-03-26",
            "capabilities": { "tools": {} },
            "serverInfo": {
                "name": config.name,
                "version": "1.0.0"
            }
        })
    }

    fn handle_tools_list(&self, config: &Arc<ServerConfig>) -> Value {
        let tools = self.schema_cache.get_or_generate(config);
        let result = ToolsListResult {
            tools: (*tools).clone(),
        };
        // Serialisation is infallible for well-typed ToolsListResult.
        #[allow(clippy::expect_used)]
        serde_json::to_value(&result).expect("ToolsListResult serialisation must not fail")
    }

    async fn handle_tools_call(
        &self,
        config: &Arc<ServerConfig>,
        params: Option<Value>,
    ) -> ToolsCallOutcome {
        let mut meta = ToolsCallLogMeta::default();

        // Parse tool call params from the JSON-RPC `params` field.
        let tool_params: ToolCallParams = match params {
            Some(p) => match serde_json::from_value(p) {
                Ok(t) => t,
                Err(e) => return ToolsCallOutcome::error(meta, JsonRpcError {
                    code: error_codes::INVALID_PARAMS,
                    message: format!("Invalid tools/call params: {e}"),
                    data: None,
                }),
            },
            None => return ToolsCallOutcome::error(meta, JsonRpcError {
                code: error_codes::INVALID_PARAMS,
                message: "tools/call requires a params object".to_string(),
                data: None,
            }),
        };

        meta.tool_name = Some(tool_params.name.clone());

        // Deserialise config_json to check whether the requested tool exists.
        let gw_config: GatewayConfig = match serde_json::from_value(config.config_json.clone()) {
            Ok(c) => c,
            Err(e) => return ToolsCallOutcome::error(meta, JsonRpcError {
                code: error_codes::INTERNAL_ERROR,
                message: format!("Invalid server config_json: {e}"),
                data: None,
            }),
        };

        // Validate tool name exists. Return -32602 (Invalid Params) on unknown tool.
        if !gw_config.tools.iter().any(|t| t.name == tool_params.name) {
            return ToolsCallOutcome::error(meta, JsonRpcError {
                code: error_codes::INVALID_PARAMS,
                message: format!("Unknown tool: {}", tool_params.name),
                data: None,
            });
        }

        // S-026: Build the upstream HTTP request.
        let upstream_req = match self.upstream.request_builder.build(config, &tool_params) {
            Ok(r) => r,
            Err(e) => return ToolsCallOutcome::error(meta, JsonRpcError {
                code: error_codes::INVALID_PARAMS,
                message: e.to_string(),
                data: None,
            }),
        };

        // Sanitise the upstream URL for safe logging (no auth query params).
        meta.upstream_url = url::Url::parse(&upstream_req.url)
            .map(|u| sanitise_url(&u))
            .ok();

        // S-027: Execute via sidecar IPC or direct reqwest.
        let upstream_resp =
            match self.upstream.executor.execute(config.id, upstream_req, &gw_config.auth_type).await {
                Ok(r) => r,
                Err(e) => return ToolsCallOutcome::error(meta, execute_error_to_jsonrpc_error(e)),
            };

        meta.upstream_status = Some(upstream_resp.status);
        meta.upstream_latency_ms = Some(upstream_resp.latency_ms);
        meta.response_size_bytes = Some(upstream_resp.body.len());

        // S-028: Apply declarative transform pipeline.
        // No per-server transform pipeline config is stored yet; an empty
        // pipeline wraps the upstream body verbatim in MCP content format.
        let body_str = String::from_utf8_lossy(&upstream_resp.body);
        let empty_config = TransformPipelineConfig { ops: vec![] };
        let pipeline = match TransformPipeline::new(empty_config) {
            Ok(p) => p,
            Err(e) => return ToolsCallOutcome::error(meta, JsonRpcError {
                code: error_codes::INTERNAL_ERROR,
                message: format!("Failed to build transform pipeline: {e}"),
                data: None,
            }),
        };

        let (content, warnings) = match pipeline.apply(&body_str) {
            Ok(r) => r,
            Err(e) => return ToolsCallOutcome::error(meta, JsonRpcError {
                code: error_codes::INTERNAL_ERROR,
                message: format!("Transform failed: {e}"),
                data: None,
            }),
        };

        if !warnings.is_empty() {
            tracing::warn!(
                server_id = %config.id,
                tool = %tool_params.name,
                warning_count = warnings.len(),
                "transform warnings during tools/call"
            );
        }

        meta.transform_warnings = warnings
            .iter()
            .map(|w| format!("[{}:{}] {}", w.op_index, w.op_name, w.message))
            .collect();

        // tools/call result: {"content": [{"type":"text","text":"<json>"}]}
        ToolsCallOutcome { result: Ok(json!({ "content": content })), meta }
    }
}

// ── Response constructors ─────────────────────────────────────────────────────

fn make_ok_response(id: Option<Value>, result: Value) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        result: Some(result),
        error: None,
        id,
    }
}

fn make_error_response(
    id: Option<Value>,
    code: i32,
    message: &str,
    data: Option<Value>,
) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        result: None,
        error: Some(JsonRpcError {
            code,
            message: message.to_string(),
            data,
        }),
        id,
    }
}

// ── ExecuteError → JsonRpcError mapping ───────────────────────────────────────

fn execute_error_to_jsonrpc_error(err: ExecuteError) -> JsonRpcError {
    match err {
        ExecuteError::CircuitOpen { retry_after_ms } => JsonRpcError {
            code: CODE_CIRCUIT_OPEN,
            message: "Upstream temporarily unavailable".to_string(),
            data: Some(json!({ "retry_after_ms": retry_after_ms })),
        },
        ExecuteError::SidecarUnavailable => JsonRpcError {
            code: CODE_UPSTREAM_UNAVAILABLE,
            message: "Upstream unavailable".to_string(),
            data: Some(json!({ "reason": "credential_service_unreachable" })),
        },
        ExecuteError::Timeout => JsonRpcError {
            code: CODE_UPSTREAM_UNAVAILABLE,
            message: "Upstream timeout".to_string(),
            data: None,
        },
        ExecuteError::UpstreamError { status } => JsonRpcError {
            code: CODE_UPSTREAM_ERROR,
            message: "Upstream error".to_string(),
            data: Some(json!({ "status": status })),
        },
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::cache::ConfigCache;
    use crate::circuit_breaker::CircuitBreakerRegistry;
    use crate::logging::{LogLevel, RequestLogger};
    use crate::protocol::jsonrpc::ParsedRequest;
    use crate::sidecar::{SidecarPool, UpstreamExecutor};
    use crate::upstream::UpstreamRequestBuilder;
    use chrono::Utc;
    use mcp_common::testing::MockUpstream;
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::Arc;
    use uuid::Uuid;

    // ── Helpers ────────────────────────────────────────────────────────────────

    /// Build a minimal ServerConfig for testing.
    fn make_config(name: &str, slug: &str, config_json: serde_json::Value) -> Arc<ServerConfig> {
        Arc::new(ServerConfig {
            id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            name: name.to_string(),
            slug: slug.to_string(),
            description: None,
            config_json,
            status: "active".to_string(),
            config_version: 1,
            token_hash: None,
            token_prefix: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        })
    }

    /// Minimal config_json with one GET tool and no authentication.
    fn make_simple_config_json(base_url: &str) -> serde_json::Value {
        json!({
            "base_url": base_url,
            "auth_type": "none",
            "tools": [
                {
                    "name": "get_weather",
                    "description": "Fetch current weather",
                    "http_method": "GET",
                    "path_template": "/weather",
                    "query_params": [],
                    "parameters": []
                }
            ]
        })
    }

    /// Build a Router with a pre-populated cache and an executor backed by a
    /// non-existent sidecar socket (acceptable for tests that don't call the
    /// upstream).
    #[allow(clippy::disallowed_methods)]
    fn make_router_with_config(config: Arc<ServerConfig>) -> Router {
        let pool = sqlx::PgPool::connect_lazy("postgres://localhost/test_router").unwrap();
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

        let logger = RequestLogger::new(uuid::Uuid::new_v4().to_string(), LogLevel::Info);
        Router::new(cache, upstream, logger)
    }

    fn make_request(method: &str, id: Option<serde_json::Value>, params: Option<serde_json::Value>) -> ParsedRequest {
        ParsedRequest {
            jsonrpc: "2.0".to_string(),
            id,
            method: method.to_string(),
            params,
        }
    }

    // ── Tests ──────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn dispatch_unknown_slug_returns_server_not_found() {
        let config = make_config("Test", "my-server", make_simple_config_json("http://localhost"));
        let router = make_router_with_config(config);

        let req = make_request("tools/list", Some(json!(1)), None);
        let resp = router.dispatch("nonexistent-slug", req, None).await;

        let err = resp.error.expect("expected error");
        assert_eq!(err.code, CODE_SERVER_NOT_FOUND);
        assert_eq!(err.message, "Server not found");
    }

    #[tokio::test]
    async fn dispatch_initialize_returns_correct_server_name() {
        let config = make_config("My Weather API", "weather", make_simple_config_json("http://localhost"));
        let router = make_router_with_config(Arc::clone(&config));

        let req = make_request("initialize", Some(json!(1)), None);
        let resp = router.dispatch("weather", req, None).await;

        assert!(resp.error.is_none(), "expected success, got {:?}", resp.error);
        let result = resp.result.expect("expected result");
        assert_eq!(result["protocolVersion"], "2025-03-26");
        assert_eq!(result["serverInfo"]["name"], "My Weather API");
        assert_eq!(result["serverInfo"]["version"], "1.0.0");
        assert!(result["capabilities"]["tools"].is_object());
    }

    #[tokio::test]
    async fn dispatch_initialized_returns_null_id_response() {
        let config = make_config("Test", "test-server", make_simple_config_json("http://localhost"));
        let router = make_router_with_config(config);

        let req = make_request("initialized", None, None);
        let resp = router.dispatch("test-server", req, None).await;

        assert_eq!(resp.jsonrpc, "2.0");
        assert!(resp.error.is_none());
        assert!(resp.result.is_none());
        assert_eq!(resp.id, Some(Value::Null));
    }

    #[tokio::test]
    async fn dispatch_ping_returns_empty_result() {
        let config = make_config("Test", "ping-server", make_simple_config_json("http://localhost"));
        let router = make_router_with_config(config);

        let req = make_request("ping", Some(json!(42)), None);
        let resp = router.dispatch("ping-server", req, None).await;

        assert!(resp.error.is_none(), "expected success, got {:?}", resp.error);
        let result = resp.result.expect("expected result");
        assert_eq!(result, json!({}));
        assert_eq!(resp.id, Some(json!(42)));
    }

    #[tokio::test]
    async fn dispatch_tools_list_returns_tools_array() {
        let config = make_config("Test", "tools-server", make_simple_config_json("http://localhost"));
        let router = make_router_with_config(Arc::clone(&config));

        let req = make_request("tools/list", Some(json!(1)), None);
        let resp = router.dispatch("tools-server", req, None).await;

        assert!(resp.error.is_none(), "expected success, got {:?}", resp.error);
        let result = resp.result.expect("expected result");
        let tools = result["tools"].as_array().expect("tools must be array");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "get_weather");
    }

    #[tokio::test]
    async fn dispatch_tools_list_result_is_cached_per_config_version() {
        let config = make_config("Test", "cache-server", make_simple_config_json("http://localhost"));
        let router = make_router_with_config(Arc::clone(&config));

        // First call populates the schema cache.
        let req1 = make_request("tools/list", Some(json!(1)), None);
        let resp1 = router.dispatch("cache-server", req1, None).await;

        // Second call must hit the cache (same config_version).
        let req2 = make_request("tools/list", Some(json!(2)), None);
        let resp2 = router.dispatch("cache-server", req2, None).await;

        let tools1 = resp1.result.unwrap()["tools"].clone();
        let tools2 = resp2.result.unwrap()["tools"].clone();
        assert_eq!(tools1, tools2, "schema cache must return same tools on repeated calls");
    }

    #[tokio::test]
    async fn dispatch_tools_call_unknown_tool_returns_invalid_params() {
        let config = make_config("Test", "tool-server", make_simple_config_json("http://localhost"));
        let router = make_router_with_config(config);

        let req = make_request(
            "tools/call",
            Some(json!(1)),
            Some(json!({ "name": "nonexistent_tool", "arguments": {} })),
        );
        let resp = router.dispatch("tool-server", req, None).await;

        let err = resp.error.expect("expected error");
        assert_eq!(err.code, error_codes::INVALID_PARAMS);
        assert!(
            err.message.contains("Unknown tool: nonexistent_tool"),
            "message was: {}",
            err.message
        );
    }

    #[tokio::test]
    async fn dispatch_tools_call_missing_params_returns_invalid_params() {
        let config = make_config("Test", "param-server", make_simple_config_json("http://localhost"));
        let router = make_router_with_config(config);

        let req = make_request("tools/call", Some(json!(1)), None);
        let resp = router.dispatch("param-server", req, None).await;

        let err = resp.error.expect("expected error");
        assert_eq!(err.code, error_codes::INVALID_PARAMS);
    }

    #[tokio::test]
    async fn dispatch_unknown_method_returns_method_not_found() {
        let config = make_config("Test", "method-server", make_simple_config_json("http://localhost"));
        let router = make_router_with_config(config);

        let req = make_request("resources/list", Some(json!(1)), None);
        let resp = router.dispatch("method-server", req, None).await;

        let err = resp.error.expect("expected error");
        assert_eq!(err.code, error_codes::METHOD_NOT_FOUND);
        assert_eq!(err.message, "Method not found");
    }

    #[tokio::test]
    async fn dispatch_tools_call_auth_none_calls_upstream_directly() {
        // Start a mock HTTP server to act as the upstream.
        let mock = MockUpstream::start().await;
        mock.set_response_body(json!({"temperature": 22, "unit": "celsius"}));

        let base_url = format!("http://{}", mock.addr);
        let config_json = json!({
            "base_url": base_url,
            "auth_type": "none",
            "tools": [
                {
                    "name": "get_weather",
                    "description": "Fetch current weather",
                    "http_method": "GET",
                    "path_template": "/weather",
                    "query_params": [],
                    "parameters": []
                }
            ]
        });
        let config = make_config("Weather API", "weather-api", config_json);
        let router = make_router_with_config(Arc::clone(&config));

        let req = make_request(
            "tools/call",
            Some(json!(1)),
            Some(json!({ "name": "get_weather", "arguments": {} })),
        );
        let resp = router.dispatch("weather-api", req, None).await;

        assert!(resp.error.is_none(), "expected success, got {:?}", resp.error);
        let result = resp.result.expect("expected result");
        let content = result["content"].as_array().expect("content must be array");
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
        // The text field must contain the upstream JSON response.
        let text = content[0]["text"].as_str().expect("text must be string");
        let parsed: serde_json::Value = serde_json::from_str(text).expect("text must be valid JSON");
        assert_eq!(parsed["temperature"], 22);

        // Verify the mock received exactly one request.
        let reqs: Vec<_> = mock.received_requests();
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].path, "/weather");
    }
}
