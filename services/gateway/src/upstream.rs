//! Upstream HTTP request builder.
//!
//! Converts a [`ToolCallParams`] and a [`crate::cache::ServerConfig`] into
//! a ready-to-send [`UpstreamRequest`] struct.  This module is **pure**: no
//! async, no I/O, no authentication.  Auth injection is the credential-injector
//! sidecar's responsibility (see S-027).
//!
//! # Usage
//!
//! ```ignore
//! let builder = UpstreamRequestBuilder::new(); // reads GATEWAY_ALLOW_HTTP env
//! let req = builder.build(&server_config, &tool_call)?;
//! // hand req to the sidecar IPC call or a direct reqwest::Client
//! ```
//!
//! # URL construction
//!
//! 1. `base_url` from `config_json` (e.g. `https://api.example.com`)
//! 2. `path_template` with `{param_name}` placeholders percent-encoded from
//!    tool-call arguments (e.g. `/users/{user_id}` → `/users/alice%40example`)
//! 3. Query parameters appended in declared order; argument value takes
//!    precedence over static default
//!
//! # Security
//!
//! - Non-HTTPS `base_url` is rejected unless `GATEWAY_ALLOW_HTTP=true` is set.
//! - URLs exceeding 8,192 characters are rejected.
//! - Auth headers are **never** present in `UpstreamRequest`.

use crate::cache::ServerConfig;
use crate::util::template::extract_placeholders;
use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::time::Duration;
use thiserror::Error;

// ── Percent-encoding sets ─────────────────────────────────────────────────────

/// Characters that must be percent-encoded in a URL path segment.
///
/// Encodes everything except RFC 3986 unreserved characters
/// (`ALPHA / DIGIT / "-" / "." / "_" / "~"`).
const PATH_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}')
    .add(b'/')
    .add(b':')
    .add(b';')
    .add(b'@')
    .add(b'=')
    .add(b'&')
    .add(b'+')
    .add(b'$')
    .add(b',')
    .add(b'!')
    .add(b'\'')
    .add(b'(')
    .add(b')')
    .add(b'*');

/// Characters that must be percent-encoded in a query parameter value.
const QUERY_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'&')
    .add(b'+')
    .add(b'=')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}');

// ── Constants ─────────────────────────────────────────────────────────────────

/// Maximum allowed URL length in characters.
pub const MAX_URL_LEN: usize = 8_192;

/// Default upstream request timeout in milliseconds.
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// User-Agent header value sent on every upstream request.
pub const USER_AGENT: &str = "mcp-gateway/1.0";

// ── Config types (parsed from ServerConfig.config_json) ───────────────────────

/// Gateway view of a server's `config_json` JSONB column.
///
/// This struct is deserialized from `ServerConfig.config_json` at request
/// time. Fields use `#[serde(default)]` so that JSONB rows written by older
/// platform API versions deserialize without error.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GatewayConfig {
    /// Base URL of the upstream REST API (e.g. `https://api.stripe.com`).
    #[serde(default)]
    pub base_url: String,
    /// Per-request timeout in milliseconds. Defaults to 30,000.
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    /// Authentication type. Used downstream by the credential injector.
    #[serde(default)]
    pub auth_type: AuthType,
    /// Tool definitions registered for this server.
    #[serde(default)]
    pub tools: Vec<ToolDefinition>,
}

fn default_timeout_ms() -> u64 {
    DEFAULT_TIMEOUT_MS
}

/// Authentication type for the upstream API.
#[derive(Debug, Clone, Deserialize, Serialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AuthType {
    /// No authentication required.
    #[default]
    None,
    /// HTTP `Authorization: Bearer <token>` header.
    Bearer,
    /// API key in a custom request header.
    ApiKeyHeader,
    /// API key as a query parameter.
    ApiKeyQuery,
    /// HTTP Basic Authentication.
    Basic,
}

/// Definition of a single tool exposed by a server.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolDefinition {
    /// Tool name (must satisfy the MCP tool name regex).
    pub name: String,
    /// Human-readable description shown in `tools/list`.
    #[serde(default)]
    pub description: String,
    /// HTTP method (GET, POST, PUT, PATCH, DELETE). Defaults to `"GET"`.
    #[serde(default = "default_http_method")]
    pub http_method: String,
    /// URL path template with `{param_name}` placeholders.
    ///
    /// Example: `/users/{user_id}/posts/{post_id}`
    #[serde(default)]
    pub path_template: String,
    /// Query parameters appended to the URL.
    #[serde(default)]
    pub query_params: Vec<QueryParamDef>,
    /// JSON body template with `{param_name}` string leaves for POST/PUT/PATCH.
    ///
    /// `null` means the method uses no request body.
    #[serde(default)]
    pub body_template: Option<Value>,
    /// Parameter declarations used for MCP JSON Schema generation.
    #[serde(default)]
    pub parameters: Vec<ParameterDef>,
}

fn default_http_method() -> String {
    "GET".to_string()
}

/// A single query-parameter definition.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct QueryParamDef {
    /// Query parameter key.
    ///
    /// If a `tools/call` argument with this name is present, its value is used.
    /// Otherwise `default` is used (if provided). If neither is present the
    /// parameter is omitted from the URL.
    pub key: String,
    /// Static default value used when no matching argument is supplied.
    #[serde(default)]
    pub default: Option<String>,
}

/// A single parameter declaration used for JSON Schema generation.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ParameterDef {
    /// Parameter name.
    pub name: String,
    /// JSON Schema type string (e.g. `"string"`, `"integer"`, `"boolean"`).
    #[serde(rename = "type", default = "default_param_type")]
    pub param_type: String,
    /// Whether this parameter is required.
    #[serde(default)]
    pub required: bool,
    /// Where the parameter appears: `"path"`, `"query"`, `"body"`.
    #[serde(default = "default_param_location")]
    pub location: String,
}

fn default_param_type() -> String {
    "string".to_string()
}

fn default_param_location() -> String {
    "query".to_string()
}

// ── Input types ───────────────────────────────────────────────────────────────

/// Parameters from a JSON-RPC `tools/call` request.
///
/// Deserialised from the `params` field of the RPC request body.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolCallParams {
    /// The tool to invoke.
    pub name: String,
    /// Caller-supplied key/value arguments.
    #[serde(default)]
    pub arguments: Map<String, Value>,
}

// ── Output types ──────────────────────────────────────────────────────────────

/// A fully-constructed upstream HTTP request, ready for the credential injector
/// or a direct `reqwest::Client` call.
///
/// **No auth headers are present.** Auth injection is the sidecar's domain.
#[derive(Debug, Clone)]
pub struct UpstreamRequest {
    /// HTTP method (uppercase), e.g. `"GET"`, `"POST"`.
    pub method: String,
    /// Fully-constructed URL including path and query string.
    pub url: String,
    /// Request headers (Content-Type, User-Agent). No Authorization header.
    pub headers: HashMap<String, String>,
    /// Serialized JSON body for POST/PUT/PATCH requests; `None` for others.
    pub body: Option<Value>,
    /// Per-request timeout derived from `config_json.timeout_ms`.
    pub timeout: Duration,
}

// ── Error types ───────────────────────────────────────────────────────────────

/// Errors that can occur while building an upstream request.
#[derive(Debug, Error)]
pub enum BuildError {
    /// A `{param_name}` placeholder in the path template has no matching
    /// argument in the tool call.
    #[error("missing required path parameter: {0}")]
    MissingPathParam(String),

    /// The `base_url` uses HTTP instead of HTTPS and `GATEWAY_ALLOW_HTTP` is
    /// not set.
    #[error("base_url must use HTTPS; set GATEWAY_ALLOW_HTTP=true to override")]
    InsecureUrl,

    /// The constructed URL exceeds [`MAX_URL_LEN`] characters.
    #[error("constructed URL is {0} characters, exceeding the 8,192 character limit")]
    UrlTooLong(usize),

    /// A body template placeholder referenced a parameter that is absent from
    /// the tool call arguments, or the template could not be processed.
    #[error("invalid body template: {0}")]
    InvalidBodyTemplate(String),

    /// The `config_json` field could not be deserialized into [`GatewayConfig`].
    #[error("invalid server config_json: {0}")]
    InvalidConfig(String),

    /// No tool with the requested name exists in the server config.
    #[error("tool not found in server config: {0}")]
    ToolNotFound(String),
}

// ── Builder ───────────────────────────────────────────────────────────────────

/// Builds [`UpstreamRequest`] from a server config and a tool call.
///
/// Construct once and reuse across requests; it holds no per-request state.
///
/// # HTTPS enforcement
///
/// By default, `base_url` must start with `https://`.  Set the environment
/// variable `GATEWAY_ALLOW_HTTP=true` to permit plain HTTP connections
/// (intended for local development and tests only).
#[derive(Debug, Clone)]
pub struct UpstreamRequestBuilder {
    /// Whether to allow non-HTTPS base URLs.
    allow_http: bool,
}

impl UpstreamRequestBuilder {
    /// Create a new builder, reading `GATEWAY_ALLOW_HTTP` from the environment.
    #[must_use]
    pub fn new() -> Self {
        let allow_http = std::env::var("GATEWAY_ALLOW_HTTP")
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        Self { allow_http }
    }

    /// Create a builder with an explicit `allow_http` flag (useful in tests).
    #[must_use]
    pub fn with_allow_http(allow_http: bool) -> Self {
        Self { allow_http }
    }

    /// Build an [`UpstreamRequest`] from a server config and tool call params.
    ///
    /// # Errors
    ///
    /// Returns [`BuildError`] for any of: missing path param, insecure URL,
    /// URL too long, invalid body template, malformed config, or unknown tool.
    pub fn build(
        &self,
        config: &ServerConfig,
        tool_call: &ToolCallParams,
    ) -> Result<UpstreamRequest, BuildError> {
        // Deserialize config_json into our typed config structure.
        let gw_config: GatewayConfig =
            serde_json::from_value(config.config_json.clone())
                .map_err(|e| BuildError::InvalidConfig(e.to_string()))?;

        // Find the matching tool definition.
        let tool_def = gw_config
            .tools
            .iter()
            .find(|t| t.name == tool_call.name)
            .ok_or_else(|| BuildError::ToolNotFound(tool_call.name.clone()))?;

        // Validate base_url scheme.
        if !self.allow_http && !gw_config.base_url.starts_with("https://") {
            return Err(BuildError::InsecureUrl);
        }

        // Interpolate path template placeholders.
        let interpolated_path =
            interpolate_path(&tool_def.path_template, &tool_call.arguments)?;

        // Join base URL and path.
        let base = gw_config.base_url.trim_end_matches('/');
        let path = interpolated_path.trim_start_matches('/');
        let mut url = if path.is_empty() {
            base.to_string()
        } else {
            format!("{base}/{path}")
        };

        // Append query parameters.
        let query_string = build_query_string(&tool_def.query_params, &tool_call.arguments);
        if !query_string.is_empty() {
            url.push('?');
            url.push_str(&query_string);
        }

        // Enforce URL length limit.
        if url.len() > MAX_URL_LEN {
            return Err(BuildError::UrlTooLong(url.len()));
        }

        // Build standard headers.
        let mut headers = HashMap::new();
        headers.insert("User-Agent".to_string(), USER_AGENT.to_string());

        // Build optional request body.
        let method = tool_def.http_method.to_uppercase();
        let body = if matches!(method.as_str(), "POST" | "PUT" | "PATCH") {
            headers.insert("Content-Type".to_string(), "application/json".to_string());
            if let Some(template) = &tool_def.body_template {
                let body_val = interpolate_body(template, &tool_call.arguments)
                    .map_err(BuildError::InvalidBodyTemplate)?;
                Some(body_val)
            } else {
                // POST/PUT/PATCH with no template → empty JSON object body.
                Some(Value::Object(Map::new()))
            }
        } else {
            None
        };

        let timeout = Duration::from_millis(gw_config.timeout_ms);

        Ok(UpstreamRequest {
            method,
            url,
            headers,
            body,
            timeout,
        })
    }
}

impl Default for UpstreamRequestBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// ── Private helpers ───────────────────────────────────────────────────────────

/// Replace `{param_name}` placeholders in a URL path template.
///
/// Each placeholder value is percent-encoded for use in a URL path segment
/// (all characters except RFC 3986 unreserved chars are encoded).
/// Returns [`BuildError::MissingPathParam`] for any placeholder with no
/// matching key in `args`.
fn interpolate_path(template: &str, args: &Map<String, Value>) -> Result<String, BuildError> {
    // Fast path: no placeholders.
    if !template.contains('{') {
        return Ok(template.to_string());
    }

    let placeholders = extract_placeholders(template);
    let mut result = template.to_string();

    for name in &placeholders {
        let val = args
            .get(name)
            .ok_or_else(|| BuildError::MissingPathParam(name.clone()))?;

        let raw_value = value_to_string(val);
        let encoded =
            utf8_percent_encode(&raw_value, PATH_ENCODE_SET).to_string();
        let placeholder = format!("{{{name}}}");
        result = result.replace(&placeholder, &encoded);
    }

    Ok(result)
}

/// Build a percent-encoded query string from parameter definitions and arguments.
///
/// For each `QueryParamDef`:
/// - If the tool call `args` contains a key matching `def.key`, use that value.
/// - Otherwise, if `def.default` is set, use the default.
/// - If neither is present, skip the parameter.
///
/// Returns an empty string if no parameters are applicable.
fn build_query_string(params: &[QueryParamDef], args: &Map<String, Value>) -> String {
    let mut pairs: Vec<String> = Vec::with_capacity(params.len());

    for def in params {
        let value: Option<String> = args
            .get(&def.key)
            .map(value_to_string)
            .or_else(|| def.default.clone());

        if let Some(v) = value {
            let k = utf8_percent_encode(&def.key, QUERY_ENCODE_SET).to_string();
            let v = utf8_percent_encode(&v, QUERY_ENCODE_SET).to_string();
            pairs.push(format!("{k}={v}"));
        }
    }

    pairs.join("&")
}

/// Recursively walk a body template JSON value, replacing `{param_name}`
/// placeholder strings with argument values.
///
/// Rules:
/// - Object → recurse into each field value.
/// - Array → recurse into each element.
/// - String that equals exactly `{param_name}` → replace with the argument
///   value (preserving its JSON type — number, bool, string, etc.).
/// - String containing `{param_name}` as a substring → string interpolation
///   (argument value serialized to its string representation).
/// - Other scalar → pass through unchanged.
///
/// Returns `Err(message)` if a required placeholder is absent from `args`.
fn interpolate_body(
    template: &Value,
    args: &Map<String, Value>,
) -> Result<Value, String> {
    match template {
        Value::Object(obj) => {
            let mut result = Map::with_capacity(obj.len());
            for (k, v) in obj {
                result.insert(k.clone(), interpolate_body(v, args)?);
            }
            Ok(Value::Object(result))
        }
        Value::Array(arr) => {
            let mut result = Vec::with_capacity(arr.len());
            for item in arr {
                result.push(interpolate_body(item, args)?);
            }
            Ok(Value::Array(result))
        }
        Value::String(s) => {
            // Check if the entire string is a single placeholder.
            if let Some(name) = extract_single_placeholder(s) {
                let val = args
                    .get(name)
                    .ok_or_else(|| format!("missing argument for placeholder '{name}'"))?;
                return Ok(val.clone());
            }

            // Partial-replacement: substitute {name} substrings.
            if s.contains('{') {
                let placeholders = extract_placeholders(s);
                let mut result = s.clone();
                for name in &placeholders {
                    let val = args.get(name).ok_or_else(|| {
                        format!("missing argument for placeholder '{name}'")
                    })?;
                    let replacement = value_to_string(val);
                    result = result.replace(&format!("{{{name}}}"), &replacement);
                }
                return Ok(Value::String(result));
            }

            Ok(template.clone())
        }
        // Null, bool, number → unchanged.
        other => Ok(other.clone()),
    }
}

/// If `s` is exactly `{param_name}` (a single whole-string placeholder), return
/// the inner name.  Returns `None` for empty strings, strings without braces,
/// or strings with content outside the braces.
fn extract_single_placeholder(s: &str) -> Option<&str> {
    let s = s.trim();
    if s.starts_with('{') && s.ends_with('}') && s.len() > 2 {
        let inner = &s[1..s.len() - 1];
        // Ensure there are no nested braces.
        if !inner.contains('{') && !inner.contains('}') {
            return Some(inner);
        }
    }
    None
}

/// Convert a JSON [`Value`] to its string representation for use in URL
/// templates and query strings.
///
/// - String → inner string value (no quotes).
/// - Number / Bool / Null → `serde_json::to_string` (lossless).
/// - Object / Array → JSON-serialized string.
fn value_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
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
    use chrono::Utc;
    use serde_json::json;
    use uuid::Uuid;

    /// Helper to build a minimal [`ServerConfig`] from a `config_json` blob.
    fn make_server_config(config_json: Value) -> ServerConfig {
        ServerConfig {
            id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            name: "test server".to_string(),
            slug: "test-server".to_string(),
            description: None,
            config_json,
            status: "active".to_string(),
            config_version: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    /// Create a builder that accepts HTTP (for tests with localhost URLs).
    fn http_builder() -> UpstreamRequestBuilder {
        UpstreamRequestBuilder::with_allow_http(true)
    }

    /// Create a builder that requires HTTPS.
    fn https_builder() -> UpstreamRequestBuilder {
        UpstreamRequestBuilder::with_allow_http(false)
    }

    // ── GET with single path param ────────────────────────────────────────────

    #[test]
    fn get_with_path_param_substitution() {
        let config = make_server_config(json!({
            "base_url": "https://api.example.com",
            "tools": [{
                "name": "get_user",
                "description": "Get user",
                "http_method": "GET",
                "path_template": "/users/{user_id}",
                "query_params": [],
                "parameters": [{"name": "user_id", "type": "string", "required": true}]
            }]
        }));

        let call = ToolCallParams {
            name: "get_user".to_string(),
            arguments: {
                let mut m = Map::new();
                m.insert("user_id".to_string(), json!("alice"));
                m
            },
        };

        let req = https_builder().build(&config, &call).unwrap();
        assert_eq!(req.method, "GET");
        assert_eq!(req.url, "https://api.example.com/users/alice");
        assert_eq!(req.headers.get("User-Agent").unwrap(), USER_AGENT);
        assert!(req.body.is_none());
    }

    // ── POST with body template ───────────────────────────────────────────────

    #[test]
    fn post_with_body_template() {
        let config = make_server_config(json!({
            "base_url": "https://api.example.com",
            "tools": [{
                "name": "create_post",
                "description": "Create a post",
                "http_method": "POST",
                "path_template": "/posts",
                "body_template": {"title": "{title}", "body": "{content}"},
                "parameters": [
                    {"name": "title", "type": "string", "required": true},
                    {"name": "content", "type": "string", "required": true}
                ]
            }]
        }));

        let call = ToolCallParams {
            name: "create_post".to_string(),
            arguments: {
                let mut m = Map::new();
                m.insert("title".to_string(), json!("Hello World"));
                m.insert("content".to_string(), json!("Some text here"));
                m
            },
        };

        let req = https_builder().build(&config, &call).unwrap();
        assert_eq!(req.method, "POST");
        assert_eq!(req.url, "https://api.example.com/posts");
        assert_eq!(
            req.headers.get("Content-Type").unwrap(),
            "application/json"
        );
        let body = req.body.unwrap();
        assert_eq!(body["title"], json!("Hello World"));
        // "body" is the JSON key in the template; its value comes from the "content" argument.
        assert_eq!(body["body"], json!("Some text here"));
    }

    // ── Missing required path param ───────────────────────────────────────────

    #[test]
    fn missing_path_param_returns_error() {
        let config = make_server_config(json!({
            "base_url": "https://api.example.com",
            "tools": [{
                "name": "get_item",
                "http_method": "GET",
                "path_template": "/items/{item_id}"
            }]
        }));

        let call = ToolCallParams {
            name: "get_item".to_string(),
            arguments: Map::new(),
        };

        let err = https_builder().build(&config, &call).unwrap_err();
        assert!(matches!(err, BuildError::MissingPathParam(n) if n == "item_id"));
    }

    // ── Query param with argument override ────────────────────────────────────

    #[test]
    fn query_param_argument_overrides_default() {
        let config = make_server_config(json!({
            "base_url": "https://api.example.com",
            "tools": [{
                "name": "search",
                "http_method": "GET",
                "path_template": "/search",
                "query_params": [
                    {"key": "q", "default": null},
                    {"key": "limit", "default": "10"}
                ]
            }]
        }));

        let call = ToolCallParams {
            name: "search".to_string(),
            arguments: {
                let mut m = Map::new();
                m.insert("q".to_string(), json!("rust lang"));
                m.insert("limit".to_string(), json!("50"));
                m
            },
        };

        let req = https_builder().build(&config, &call).unwrap();
        // q=rust%20lang&limit=50 (spaces encoded)
        assert!(req.url.contains("q=rust%20lang"), "got: {}", req.url);
        assert!(req.url.contains("limit=50"), "got: {}", req.url);
    }

    // ── Query param with static default (no arg) ──────────────────────────────

    #[test]
    fn query_param_uses_static_default_when_no_arg() {
        let config = make_server_config(json!({
            "base_url": "https://api.example.com",
            "tools": [{
                "name": "list_items",
                "http_method": "GET",
                "path_template": "/items",
                "query_params": [
                    {"key": "page_size", "default": "20"}
                ]
            }]
        }));

        let call = ToolCallParams {
            name: "list_items".to_string(),
            arguments: Map::new(),
        };

        let req = https_builder().build(&config, &call).unwrap();
        assert!(
            req.url.contains("page_size=20"),
            "default must be used; got: {}",
            req.url
        );
    }

    // ── Multiple path params ──────────────────────────────────────────────────

    #[test]
    fn multiple_path_params_all_substituted() {
        let config = make_server_config(json!({
            "base_url": "https://api.example.com",
            "tools": [{
                "name": "get_comment",
                "http_method": "GET",
                "path_template": "/users/{user_id}/posts/{post_id}/comments/{comment_id}"
            }]
        }));

        let call = ToolCallParams {
            name: "get_comment".to_string(),
            arguments: {
                let mut m = Map::new();
                m.insert("user_id".to_string(), json!("u1"));
                m.insert("post_id".to_string(), json!("p2"));
                m.insert("comment_id".to_string(), json!("c3"));
                m
            },
        };

        let req = https_builder().build(&config, &call).unwrap();
        assert_eq!(
            req.url,
            "https://api.example.com/users/u1/posts/p2/comments/c3"
        );
    }

    // ── Insecure URL rejected ─────────────────────────────────────────────────

    #[test]
    fn http_url_rejected_without_allow_flag() {
        let config = make_server_config(json!({
            "base_url": "http://api.example.com",
            "tools": [{"name": "t", "http_method": "GET", "path_template": "/"}]
        }));
        let call = ToolCallParams {
            name: "t".to_string(),
            arguments: Map::new(),
        };

        let err = https_builder().build(&config, &call).unwrap_err();
        assert!(matches!(err, BuildError::InsecureUrl));
    }

    #[test]
    fn http_url_allowed_with_flag() {
        let config = make_server_config(json!({
            "base_url": "http://localhost:8080",
            "tools": [{"name": "t", "http_method": "GET", "path_template": "/ping"}]
        }));
        let call = ToolCallParams {
            name: "t".to_string(),
            arguments: Map::new(),
        };

        let req = http_builder().build(&config, &call).unwrap();
        assert_eq!(req.url, "http://localhost:8080/ping");
    }

    // ── URL too long ──────────────────────────────────────────────────────────

    #[test]
    fn url_too_long_returns_error() {
        let long_base = format!("https://api.example.com/{}", "x".repeat(MAX_URL_LEN));
        let config = make_server_config(json!({
            "base_url": long_base,
            "tools": [{"name": "t", "http_method": "GET", "path_template": "/data"}]
        }));
        let call = ToolCallParams {
            name: "t".to_string(),
            arguments: Map::new(),
        };

        let err = http_builder().build(&config, &call).unwrap_err();
        assert!(matches!(err, BuildError::UrlTooLong(_)));
    }

    // ── Path param special chars are percent-encoded ──────────────────────────

    #[test]
    fn path_param_special_chars_encoded() {
        let config = make_server_config(json!({
            "base_url": "https://api.example.com",
            "tools": [{
                "name": "get",
                "http_method": "GET",
                "path_template": "/data/{key}"
            }]
        }));
        let call = ToolCallParams {
            name: "get".to_string(),
            arguments: {
                let mut m = Map::new();
                // Slash and space should be percent-encoded in path segment.
                m.insert("key".to_string(), json!("hello world/test"));
                m
            },
        };
        let req = https_builder().build(&config, &call).unwrap();
        // space → %20, / → %2F
        assert!(req.url.contains("hello%20world%2Ftest"), "got: {}", req.url);
    }

    // ── Body template with type-preserving whole-placeholder substitution ─────

    #[test]
    fn body_template_preserves_number_type() {
        let config = make_server_config(json!({
            "base_url": "https://api.example.com",
            "tools": [{
                "name": "update_count",
                "http_method": "POST",
                "path_template": "/counts",
                "body_template": {"count": "{count}"}
            }]
        }));
        let call = ToolCallParams {
            name: "update_count".to_string(),
            arguments: {
                let mut m = Map::new();
                m.insert("count".to_string(), json!(42));
                m
            },
        };
        let req = https_builder().build(&config, &call).unwrap();
        let body = req.body.unwrap();
        // Whole-string placeholder → should preserve numeric type.
        assert_eq!(body["count"], json!(42));
    }

    // ── Unknown tool returns error ────────────────────────────────────────────

    #[test]
    fn unknown_tool_returns_tool_not_found() {
        let config = make_server_config(json!({
            "base_url": "https://api.example.com",
            "tools": [{"name": "existing", "http_method": "GET", "path_template": "/"}]
        }));
        let call = ToolCallParams {
            name: "nonexistent".to_string(),
            arguments: Map::new(),
        };

        let err = https_builder().build(&config, &call).unwrap_err();
        assert!(matches!(err, BuildError::ToolNotFound(n) if n == "nonexistent"));
    }

    // ── Timeout derived from config ───────────────────────────────────────────

    #[test]
    fn custom_timeout_applied() {
        let config = make_server_config(json!({
            "base_url": "https://api.example.com",
            "timeout_ms": 5000,
            "tools": [{"name": "t", "http_method": "GET", "path_template": "/"}]
        }));
        let call = ToolCallParams {
            name: "t".to_string(),
            arguments: Map::new(),
        };
        let req = https_builder().build(&config, &call).unwrap();
        assert_eq!(req.timeout, Duration::from_millis(5000));
    }

    // ── Default timeout when not specified ────────────────────────────────────

    #[test]
    fn default_timeout_when_not_specified() {
        let config = make_server_config(json!({
            "base_url": "https://api.example.com",
            "tools": [{"name": "t", "http_method": "GET", "path_template": "/"}]
        }));
        let call = ToolCallParams {
            name: "t".to_string(),
            arguments: Map::new(),
        };
        let req = https_builder().build(&config, &call).unwrap();
        assert_eq!(req.timeout, Duration::from_millis(DEFAULT_TIMEOUT_MS));
    }
}
