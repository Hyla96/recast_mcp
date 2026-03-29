//! MCP (Model Context Protocol) types and serialization.

use serde::{Deserialize, Serialize};

/// A JSON-RPC 2.0 request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    /// The JSON-RPC version (always "2.0").
    pub jsonrpc: String,
    /// The method name.
    pub method: String,
    /// Request parameters (optional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
    /// Request ID (required for non-notification requests).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<serde_json::Value>,
}

/// A JSON-RPC 2.0 response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    /// The JSON-RPC version (always "2.0").
    pub jsonrpc: String,
    /// The result (for successful responses).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    /// The error (for error responses).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
    /// The request ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<serde_json::Value>,
}

/// A JSON-RPC error object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    /// The error code.
    pub code: i32,
    /// The error message.
    pub message: String,
    /// Additional error data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialize_json_rpc_request() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/list".to_string(),
            params: None,
            id: Some(serde_json::json!(1)),
        };

        let json = serde_json::to_string(&req).expect("failed to serialize");
        assert!(json.contains("\"method\":\"tools/list\""));
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
    }

    #[test]
    fn test_deserialize_json_rpc_request() {
        let json = r#"{"jsonrpc":"2.0","method":"tools/list","id":1}"#;
        let req: JsonRpcRequest = serde_json::from_str(json).expect("failed to deserialize");

        assert_eq!(req.jsonrpc, "2.0");
        assert_eq!(req.method, "tools/list");
        assert_eq!(req.id, Some(serde_json::json!(1)));
        assert_eq!(req.params, None);
    }

    #[test]
    fn test_deserialize_tools_call_params() {
        let json = r#"{"jsonrpc":"2.0","method":"tools/call","params":{"tool":"fetch","input":{"url":"https://example.com"}},"id":2}"#;
        let req: JsonRpcRequest = serde_json::from_str(json).expect("failed to deserialize");

        assert_eq!(req.method, "tools/call");
        assert!(req.params.is_some());
    }
}
