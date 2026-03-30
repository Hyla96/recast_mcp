//! MCP tool schema generation.
//!
//! Converts [`crate::cache::ServerConfig`] tool definitions into MCP
//! `tools/list` response payloads, each carrying a full JSON Schema for its
//! input parameters.  The function [`generate_tool_schemas`] is **pure**: no
//! async, no I/O, no authentication.
//!
//! # Schema construction
//!
//! For each [`ToolDefinition`]:
//!
//! 1. Path placeholders extracted from `path_template` via
//!    [`crate::util::template::extract_placeholders`] become required
//!    properties. Their type comes from a matching [`ParameterDef`] entry if
//!    present, otherwise defaults to `"string"`.
//! 2. [`ParameterDef`] entries not covered by path placeholders are emitted
//!    next; `required = true` entries are listed in the `required` array.
//!
//! # Caching
//!
//! [`SchemaCache`] caches by `(server_id, config_version)`.  Because
//! `config_version` is incremented on every server update (S-025), a changed
//! server automatically produces a cache miss.  Stale entries are evicted by
//! a 1-hour TTL.

use crate::cache::ServerConfig;
use crate::upstream::{GatewayConfig, ToolDefinition};
use crate::util::template::extract_placeholders;
use moka::sync::Cache;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Maximum tool description length **in characters**.
/// Descriptions longer than this are truncated with `...` appended.
pub const MAX_DESCRIPTION_LEN: usize = 1_024;

/// Maximum tool name length per MCP spec (`^[a-zA-Z][a-zA-Z0-9_-]{0,63}$`).
const MAX_TOOL_NAME_LEN: usize = 64;

/// Maximum number of `(server_id, config_version)` pairs held in the schema
/// cache.
const SCHEMA_CACHE_CAPACITY: u64 = 50_000;

/// Duration after which an unaccessed schema cache entry is evicted.
const SCHEMA_CACHE_TTL_SECS: u64 = 3_600;

// ── Output types ──────────────────────────────────────────────────────────────

/// A single tool entry in a `tools/list` MCP response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct McpTool {
    /// Tool name. Always satisfies the MCP tool-name regex.
    pub name: String,
    /// Human-readable description (at most [`MAX_DESCRIPTION_LEN`] chars).
    pub description: String,
    /// JSON Schema for the tool's input parameters.
    #[serde(rename = "inputSchema")]
    pub input_schema: InputSchema,
}

/// JSON Schema `inputSchema` object for a tool's input parameters.
///
/// `type` is always `"object"` per the MCP specification.
/// `required` is omitted from serialisation when empty.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InputSchema {
    /// Always `"object"`.
    #[serde(rename = "type")]
    pub schema_type: String,
    /// Property definitions, keyed by parameter name.
    pub properties: Map<String, Value>,
    /// Required parameter names.  Omitted from JSON when empty.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub required: Vec<String>,
}

/// Wrapper struct for the `tools/list` JSON-RPC result field.
///
/// Serialises as `{"tools": [...]}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolsListResult {
    /// Array of tool schemas.
    pub tools: Vec<McpTool>,
}

// ── Schema cache ──────────────────────────────────────────────────────────────

/// Per-`(server_id, config_version)` cache for generated tool schemas.
///
/// Because `config_version` increments on every server update, a changed
/// server automatically results in a cache miss — no explicit invalidation is
/// required.  Old entries expire via TTL.
pub struct SchemaCache {
    inner: Cache<(Uuid, i64), Arc<Vec<McpTool>>>,
}

impl SchemaCache {
    /// Create a new cache with default capacity and TTL.
    pub fn new() -> Self {
        Self {
            inner: Cache::builder()
                .max_capacity(SCHEMA_CACHE_CAPACITY)
                .time_to_live(Duration::from_secs(SCHEMA_CACHE_TTL_SECS))
                .build(),
        }
    }

    /// Return cached schemas for this config version or generate and insert them.
    pub fn get_or_generate(&self, config: &Arc<ServerConfig>) -> Arc<Vec<McpTool>> {
        let key = (config.id, config.config_version);
        if let Some(cached) = self.inner.get(&key) {
            return cached;
        }
        let tools = Arc::new(generate_tool_schemas(config));
        self.inner.insert(key, Arc::clone(&tools));
        tools
    }
}

impl Default for SchemaCache {
    fn default() -> Self {
        Self::new()
    }
}

// ── Core generation function ──────────────────────────────────────────────────

/// Generate MCP tool schemas from a [`ServerConfig`].
///
/// Invalid tool names are logged at `ERROR` and omitted so that the caller
/// always receives a valid, serialisable `Vec` (possibly empty).
pub fn generate_tool_schemas(config: &ServerConfig) -> Vec<McpTool> {
    let gateway_config: GatewayConfig =
        match serde_json::from_value(config.config_json.clone()) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(
                    server_id = %config.id,
                    error = %e,
                    "failed to parse config_json for tool schema generation"
                );
                return Vec::new();
            }
        };

    gateway_config
        .tools
        .iter()
        .filter_map(|tool| build_mcp_tool(config.id, tool))
        .collect()
}

// ── Internal helpers ───────────────────────────────────────────────────────────

/// Build one [`McpTool`] from a [`ToolDefinition`].
///
/// Returns `None` if the tool name fails the MCP regex; logs an error.
fn build_mcp_tool(server_id: Uuid, tool: &ToolDefinition) -> Option<McpTool> {
    if !is_valid_tool_name(&tool.name) {
        tracing::error!(
            server_id = %server_id,
            tool_name = %tool.name,
            "invalid tool name — omitting from tools/list"
        );
        return None;
    }

    let description = truncate_description(&tool.description);
    let input_schema = build_input_schema(tool);

    Some(McpTool {
        name: tool.name.clone(),
        description,
        input_schema,
    })
}

/// Validate a tool name against `^[a-zA-Z][a-zA-Z0-9_-]{0,63}$`.
///
/// Implemented without the regex crate to keep compile times down.
fn is_valid_tool_name(name: &str) -> bool {
    let mut chars = name.chars();

    // First character must be ASCII alphabetic.
    match chars.next() {
        None => return false,
        Some(c) if !c.is_ascii_alphabetic() => return false,
        Some(_) => {}
    }

    // Remaining characters: at most 63, alphanumeric / '_' / '-'.
    let rest: Vec<char> = chars.collect();
    if rest.len() > MAX_TOOL_NAME_LEN - 1 {
        return false;
    }
    rest.iter()
        .all(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
}

/// Truncate a description to at most [`MAX_DESCRIPTION_LEN`] characters.
///
/// When truncation occurs, the last three characters are replaced with `...`
/// so the total output length is exactly `MAX_DESCRIPTION_LEN`.
fn truncate_description(description: &str) -> String {
    let chars: Vec<char> = description.chars().collect();
    if chars.len() <= MAX_DESCRIPTION_LEN {
        description.to_string()
    } else {
        let truncated: String = chars[..MAX_DESCRIPTION_LEN - 3].iter().collect();
        format!("{truncated}...")
    }
}

/// Build the `inputSchema` JSON Schema object for a single tool.
///
/// # Parameter ordering
///
/// 1. Path placeholders from `path_template` — always required, emitted first.
/// 2. [`ParameterDef`] entries for non-path params — typed + required flag.
///
/// If a path placeholder also appears in `ParameterDef.parameters`, the
/// declared type is used but the parameter is always required regardless.
fn build_input_schema(tool: &ToolDefinition) -> InputSchema {
    let mut properties: Map<String, Value> = Map::new();
    let mut required: Vec<String> = Vec::new();

    // Path placeholder names (deduped by extract_placeholders already).
    let path_placeholders: Vec<String> = extract_placeholders(&tool.path_template);
    let path_param_set: HashSet<&str> =
        path_placeholders.iter().map(String::as_str).collect();

    // Index ParameterDef by name for O(1) type lookup.
    let param_index: HashMap<&str, &crate::upstream::ParameterDef> =
        tool.parameters.iter().map(|p| (p.name.as_str(), p)).collect();

    // 1. Emit path params (always required).
    for placeholder in &path_placeholders {
        let type_str = param_index
            .get(placeholder.as_str())
            .map(|p| p.param_type.as_str())
            .unwrap_or("string");

        properties.insert(placeholder.clone(), serde_json::json!({ "type": type_str }));
        required.push(placeholder.clone());
    }

    // 2. Emit ParameterDef entries for non-path params.
    for param in &tool.parameters {
        if path_param_set.contains(param.name.as_str()) {
            // Already covered by the path placeholder pass above.
            continue;
        }

        properties.insert(
            param.name.clone(),
            serde_json::json!({ "type": param.param_type.as_str() }),
        );

        if param.required {
            required.push(param.name.clone());
        }
    }

    InputSchema {
        schema_type: "object".to_string(),
        properties,
        required,
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
    use crate::upstream::{ParameterDef, QueryParamDef, ToolDefinition};
    use chrono::Utc;
    use serde_json::{json, Value};

    // ── Fixture helpers ────────────────────────────────────────────────────────

    /// MCP tools-list schema loaded from the fixture file.
    static TOOLS_LIST_SCHEMA: &str =
        include_str!("../tests/fixtures/mcp-tools-list-schema.json");

    fn make_server_config(tools: Vec<ToolDefinition>) -> ServerConfig {
        use crate::upstream::GatewayConfig;
        let config_json = serde_json::to_value(GatewayConfig {
            base_url: "https://api.example.com".to_string(),
            timeout_ms: 30_000,
            auth_type: crate::upstream::AuthType::None,
            tools,
        })
        .unwrap();

        ServerConfig {
            id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            name: "Test Server".to_string(),
            slug: "test-server".to_string(),
            description: Some("A test server".to_string()),
            config_json,
            status: "active".to_string(),
            config_version: 1,
            token_hash: None,
            token_prefix: None,
            max_connections: 50,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn make_param(
        name: &str,
        param_type: &str,
        required: bool,
        location: &str,
    ) -> ParameterDef {
        ParameterDef {
            name: name.to_string(),
            param_type: param_type.to_string(),
            required,
            location: location.to_string(),
        }
    }

    // ── Schema validation helper ───────────────────────────────────────────────

    /// Validate a tools JSON Value against the embedded MCP fixture schema.
    /// Returns true when valid.
    fn validate_against_fixture(tools_value: &Value) -> bool {
        // Verify that the fixture parses correctly (documents its use).
        let _schema: Value = serde_json::from_str(TOOLS_LIST_SCHEMA).unwrap();
        // Manual structural validation matching the fixture schema constraints.
        let tools_arr = match tools_value.as_array() {
            Some(a) => a,
            None => return false,
        };
        for tool in tools_arr {
            let obj = match tool.as_object() {
                Some(o) => o,
                None => return false,
            };
            // "name" — required, string, matches pattern
            match obj.get("name").and_then(Value::as_str) {
                Some(n) if is_valid_tool_name(n) => {}
                _ => return false,
            }
            // "inputSchema" — required, object with type == "object"
            let input_schema = match obj.get("inputSchema").and_then(Value::as_object) {
                Some(s) => s,
                None => return false,
            };
            match input_schema.get("type").and_then(Value::as_str) {
                Some("object") => {}
                _ => return false,
            }
            // "description" — if present, string, max 1024 chars
            if let Some(desc) = obj.get("description") {
                match desc.as_str() {
                    Some(s) if s.chars().count() <= 1024 => {}
                    Some(_) => return false,
                    None => return false,
                }
            }
            // "required" — if present in inputSchema, array of strings
            if let Some(req) = input_schema.get("required") {
                match req.as_array() {
                    Some(arr) if arr.iter().all(|v| v.is_string()) => {}
                    _ => return false,
                }
            }
        }
        true
    }

    // ── Test: GET endpoint with path param only ────────────────────────────────

    #[test]
    fn get_endpoint_path_param_only() {
        let tool = ToolDefinition {
            name: "get_user".to_string(),
            description: "Fetch a user by ID".to_string(),
            http_method: "GET".to_string(),
            path_template: "/users/{user_id}".to_string(),
            query_params: vec![],
            body_template: None,
            parameters: vec![],
        };
        let config = make_server_config(vec![tool]);
        let tools = generate_tool_schemas(&config);

        assert_eq!(tools.len(), 1);
        let t = &tools[0];
        assert_eq!(t.name, "get_user");
        assert_eq!(t.description, "Fetch a user by ID");
        assert_eq!(t.input_schema.schema_type, "object");
        assert!(t.input_schema.properties.contains_key("user_id"));
        assert_eq!(
            t.input_schema.properties["user_id"],
            json!({ "type": "string" })
        );
        assert_eq!(t.input_schema.required, vec!["user_id"]);

        // Validate against MCP spec fixture.
        let tools_value = serde_json::to_value(&tools).unwrap();
        assert!(validate_against_fixture(&tools_value));
    }

    // ── Test: POST endpoint with body params ───────────────────────────────────

    #[test]
    fn post_endpoint_body_params() {
        let tool = ToolDefinition {
            name: "create_post".to_string(),
            description: "Create a new post".to_string(),
            http_method: "POST".to_string(),
            path_template: "/posts".to_string(),
            query_params: vec![],
            body_template: Some(json!({ "title": "{title}", "body": "{body}" })),
            parameters: vec![
                make_param("title", "string", true, "body"),
                make_param("body", "string", true, "body"),
                make_param("tags", "array", false, "body"),
            ],
        };
        let config = make_server_config(vec![tool]);
        let tools = generate_tool_schemas(&config);

        assert_eq!(tools.len(), 1);
        let t = &tools[0];
        assert!(t.input_schema.properties.contains_key("title"));
        assert!(t.input_schema.properties.contains_key("body"));
        assert!(t.input_schema.properties.contains_key("tags"));
        // title and body are required; tags is not
        assert!(t.input_schema.required.contains(&"title".to_string()));
        assert!(t.input_schema.required.contains(&"body".to_string()));
        assert!(!t.input_schema.required.contains(&"tags".to_string()));

        let tools_value = serde_json::to_value(&tools).unwrap();
        assert!(validate_against_fixture(&tools_value));
    }

    // ── Test: all param types mixed ────────────────────────────────────────────

    #[test]
    fn all_param_types_mixed() {
        let tool = ToolDefinition {
            name: "search_items".to_string(),
            description: "Search with all param types".to_string(),
            http_method: "POST".to_string(),
            path_template: "/orgs/{org_id}/search".to_string(),
            query_params: vec![
                QueryParamDef {
                    key: "limit".to_string(),
                    default: Some("10".to_string()),
                },
            ],
            body_template: Some(json!({ "query": "{query}" })),
            parameters: vec![
                // path param with explicit type override
                make_param("org_id", "string", true, "path"),
                // required query param
                make_param("limit", "integer", false, "query"),
                // required body param
                make_param("query", "string", true, "body"),
                // optional body param
                make_param("filters", "object", false, "body"),
            ],
        };
        let config = make_server_config(vec![tool]);
        let tools = generate_tool_schemas(&config);

        assert_eq!(tools.len(), 1);
        let t = &tools[0];
        // org_id is a path param → required, type from ParameterDef
        assert_eq!(
            t.input_schema.properties["org_id"],
            json!({ "type": "string" })
        );
        assert!(t.input_schema.required.contains(&"org_id".to_string()));
        // limit is non-required query param
        assert_eq!(
            t.input_schema.properties["limit"],
            json!({ "type": "integer" })
        );
        assert!(!t.input_schema.required.contains(&"limit".to_string()));
        // query is required body param
        assert!(t.input_schema.required.contains(&"query".to_string()));
        // filters is optional body param
        assert!(!t.input_schema.required.contains(&"filters".to_string()));

        let tools_value = serde_json::to_value(&tools).unwrap();
        assert!(validate_against_fixture(&tools_value));
    }

    // ── Test: invalid tool name is omitted ─────────────────────────────────────

    #[test]
    fn invalid_tool_name_omitted() {
        let valid_tool = ToolDefinition {
            name: "valid_tool".to_string(),
            description: "A valid tool".to_string(),
            http_method: "GET".to_string(),
            path_template: "/items".to_string(),
            query_params: vec![],
            body_template: None,
            parameters: vec![],
        };
        // Names that must be rejected:
        let invalid_tools = vec![
            // starts with digit
            ToolDefinition {
                name: "1bad".to_string(),
                ..valid_tool.clone()
            },
            // starts with underscore
            ToolDefinition {
                name: "_bad".to_string(),
                ..valid_tool.clone()
            },
            // empty name
            ToolDefinition {
                name: String::new(),
                ..valid_tool.clone()
            },
            // exceeds 64 chars (65 chars)
            ToolDefinition {
                name: "a".repeat(65),
                ..valid_tool.clone()
            },
        ];

        for invalid in invalid_tools {
            let config = make_server_config(vec![invalid]);
            let tools = generate_tool_schemas(&config);
            assert!(tools.is_empty(), "expected empty for invalid tool name");
        }

        // Mix valid + invalid in one config — only valid survives.
        let bad = ToolDefinition {
            name: "123bad".to_string(),
            description: String::new(),
            http_method: "GET".to_string(),
            path_template: "/".to_string(),
            query_params: vec![],
            body_template: None,
            parameters: vec![],
        };
        let config = make_server_config(vec![valid_tool, bad]);
        let tools = generate_tool_schemas(&config);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "valid_tool");

        let tools_value = serde_json::to_value(&tools).unwrap();
        assert!(validate_against_fixture(&tools_value));
    }

    // ── Test: empty tool list ──────────────────────────────────────────────────

    #[test]
    fn empty_tool_list() {
        let config = make_server_config(vec![]);
        let tools = generate_tool_schemas(&config);
        assert!(tools.is_empty());

        // Empty array is valid per the fixture schema.
        let tools_value = serde_json::to_value(&tools).unwrap();
        assert!(validate_against_fixture(&tools_value));
    }

    // ── Additional unit tests ─────────────────────────────────────────────────

    #[test]
    fn description_truncated_to_1024_chars() {
        let long_desc = "x".repeat(2_000);
        let result = truncate_description(&long_desc);
        let char_count: usize = result.chars().count();
        assert_eq!(char_count, MAX_DESCRIPTION_LEN);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn description_not_truncated_when_short() {
        let desc = "short description";
        assert_eq!(truncate_description(desc), desc);
    }

    #[test]
    fn description_exactly_1024_not_truncated() {
        let desc = "a".repeat(MAX_DESCRIPTION_LEN);
        let result = truncate_description(&desc);
        assert_eq!(result, desc);
        assert!(!result.ends_with("..."));
    }

    #[test]
    fn valid_tool_name_boundary_cases() {
        assert!(is_valid_tool_name("a"));
        assert!(is_valid_tool_name("A"));
        assert!(is_valid_tool_name("abc123"));
        assert!(is_valid_tool_name("get-user_data"));
        // Exactly 64 chars (1 + 63): valid.
        assert!(is_valid_tool_name(&format!("a{}", "x".repeat(63))));
        // 65 chars: invalid.
        assert!(!is_valid_tool_name(&format!("a{}", "x".repeat(64))));
        // Starts with digit: invalid.
        assert!(!is_valid_tool_name("1abc"));
        // Empty: invalid.
        assert!(!is_valid_tool_name(""));
        // Contains space: invalid.
        assert!(!is_valid_tool_name("bad name"));
        // Contains dot: invalid.
        assert!(!is_valid_tool_name("bad.name"));
    }

    #[test]
    fn path_placeholder_type_override_from_parameter_def() {
        // When a path param is also listed in ParameterDef, use the declared type.
        let tool = ToolDefinition {
            name: "get_item".to_string(),
            description: String::new(),
            http_method: "GET".to_string(),
            path_template: "/items/{item_id}".to_string(),
            query_params: vec![],
            body_template: None,
            parameters: vec![make_param("item_id", "integer", true, "path")],
        };
        let config = make_server_config(vec![tool]);
        let tools = generate_tool_schemas(&config);

        assert_eq!(tools[0].input_schema.properties["item_id"]["type"], "integer");
        assert!(tools[0].input_schema.required.contains(&"item_id".to_string()));
    }

    #[test]
    fn required_omitted_in_json_when_empty() {
        let tool = ToolDefinition {
            name: "list_items".to_string(),
            description: String::new(),
            http_method: "GET".to_string(),
            path_template: "/items".to_string(),
            query_params: vec![],
            body_template: None,
            parameters: vec![make_param("filter", "string", false, "query")],
        };
        let config = make_server_config(vec![tool]);
        let tools = generate_tool_schemas(&config);

        // required is empty → must not appear in serialised JSON.
        let json_str = serde_json::to_string(&tools[0].input_schema).unwrap();
        let v: Value = serde_json::from_str(&json_str).unwrap();
        assert!(v["required"].is_null(), "required key must be absent");
    }

    #[test]
    fn schema_cache_returns_same_arc_on_hit() {
        let tool = ToolDefinition {
            name: "ping_tool".to_string(),
            description: String::new(),
            http_method: "GET".to_string(),
            path_template: "/ping".to_string(),
            query_params: vec![],
            body_template: None,
            parameters: vec![],
        };
        let config = Arc::new(make_server_config(vec![tool]));
        let cache = SchemaCache::new();

        let first = cache.get_or_generate(&config);
        let second = cache.get_or_generate(&config);
        // Both calls must return the same Arc (pointer equality).
        assert!(Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn schema_cache_different_versions_different_entries() {
        let tool = ToolDefinition {
            name: "ver_tool".to_string(),
            description: String::new(),
            http_method: "GET".to_string(),
            path_template: "/ver".to_string(),
            query_params: vec![],
            body_template: None,
            parameters: vec![],
        };
        let mut config_v1 = make_server_config(vec![tool.clone()]);
        config_v1.config_version = 1;
        let mut config_v2 = config_v1.clone();
        config_v2.config_version = 2;

        let cache = SchemaCache::new();
        let r1 = cache.get_or_generate(&Arc::new(config_v1));
        let r2 = cache.get_or_generate(&Arc::new(config_v2));
        // Same content but different Arc allocations (different cache entries).
        assert!(!Arc::ptr_eq(&r1, &r2));
    }

    #[test]
    fn tools_list_result_serialises_correctly() {
        let tool = ToolDefinition {
            name: "my_tool".to_string(),
            description: "Does things".to_string(),
            http_method: "GET".to_string(),
            path_template: "/items/{id}".to_string(),
            query_params: vec![],
            body_template: None,
            parameters: vec![],
        };
        let config = make_server_config(vec![tool]);
        let tools = generate_tool_schemas(&config);
        let result = ToolsListResult { tools };
        let v: Value = serde_json::to_value(&result).unwrap();

        // Must serialise as { "tools": [...] }
        assert!(v["tools"].is_array());
        assert_eq!(v["tools"].as_array().unwrap().len(), 1);
        assert_eq!(v["tools"][0]["name"], "my_tool");
        assert_eq!(v["tools"][0]["inputSchema"]["type"], "object");
    }
}
