//! JSON-RPC 2.0 message parser.
//!
//! Transport-agnostic: no `async`, no I/O.  Both the Streamable HTTP and SSE
//! transports share this single parsing + validation layer.
//!
//! # Entry point
//!
//! ```ignore
//! use gateway::protocol::jsonrpc::{parse, Message, ParseResult};
//!
//! match parse(raw_bytes) {
//!     Message::Single(ParseResult::Request(req))      => /* dispatch and respond */,
//!     Message::Single(ParseResult::Notification(req)) => /* handle, no response */,
//!     Message::Single(ParseResult::Error(resp))       => /* send error response */,
//!     Message::Batch(items)                           => /* handle each item */,
//! }
//! ```

use mcp_protocol::{error_codes, JsonRpcError, JsonRpcResponse};
use serde_json::Value;

// ── Constants ────────────────────────────────────────────────────────────────

/// Maximum allowed raw input size (512 KB).
pub const MAX_BODY_SIZE: usize = 512 * 1024;

/// Methods the gateway knows how to handle.
pub const RECOGNIZED_METHODS: &[&str] = &[
    "initialize",
    "initialized",
    "tools/list",
    "tools/call",
    "ping",
];

// ── Public types ─────────────────────────────────────────────────────────────

/// A fully validated JSON-RPC 2.0 message, ready for dispatch.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedRequest {
    /// Always `"2.0"`.
    pub jsonrpc: String,
    /// Present and non-null for requests; absent or null for notifications.
    pub id: Option<Value>,
    /// Validated method name (guaranteed to be in [`RECOGNIZED_METHODS`]).
    pub method: String,
    /// Raw params value; passed to the handler unchanged.
    pub params: Option<Value>,
}

impl ParsedRequest {
    /// `true` when `id` is absent or explicitly `null` (notification semantics).
    #[must_use]
    pub fn is_notification(&self) -> bool {
        matches!(&self.id, None | Some(Value::Null))
    }
}

/// Result of parsing one message (single or batch item).
#[derive(Debug)]
pub enum ParseResult {
    /// Expects a response; `id` is a non-null JSON value.
    Request(ParsedRequest),
    /// No response should be sent (`id` absent or null).
    Notification(ParsedRequest),
    /// Parsing or validation failed; send this response to the client.
    Error(JsonRpcResponse),
}

/// Top-level result returned by [`parse`].
#[derive(Debug)]
pub enum Message {
    /// A single JSON-RPC object.
    Single(ParseResult),
    /// A JSON array batch; ordering is preserved.
    Batch(Vec<ParseResult>),
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Parse raw bytes as a JSON-RPC 2.0 message.
///
/// # Errors (returned as [`ParseResult::Error`] inside the returned [`Message`])
///
/// | Condition                          | Code    |
/// |------------------------------------|---------|
/// | Body exceeds 512 KB               | -32600  |
/// | Input is not valid JSON            | -32700  |
/// | Valid JSON but not a valid request | -32600  |
/// | Unknown method                     | -32601  |
///
/// For the size-exceeded case the error `data` field contains
/// `{"reason": "payload_too_large"}`.
#[must_use]
pub fn parse(body: &[u8]) -> Message {
    // 1. Enforce maximum body size before allocating.
    if body.len() > MAX_BODY_SIZE {
        return Message::Single(ParseResult::Error(make_error(
            None,
            error_codes::INVALID_REQUEST,
            "Payload too large",
            Some(serde_json::json!({"reason": "payload_too_large"})),
        )));
    }

    // 2. Attempt JSON deserialisation.
    let value: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => {
            return Message::Single(ParseResult::Error(make_error(
                None,
                error_codes::PARSE_ERROR,
                "Parse error",
                None,
            )));
        }
    };

    // 3. Dispatch on JSON type.
    match value {
        Value::Array(items) => {
            // JSON-RPC 2.0 §6: an empty batch array is an invalid request.
            if items.is_empty() {
                return Message::Single(ParseResult::Error(make_error(
                    None,
                    error_codes::INVALID_REQUEST,
                    "Invalid Request: empty batch",
                    None,
                )));
            }
            let results: Vec<ParseResult> = items.into_iter().map(parse_single).collect();
            Message::Batch(results)
        }
        other => Message::Single(parse_single(other)),
    }
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Validate and classify a single JSON value as a JSON-RPC message.
fn parse_single(value: Value) -> ParseResult {
    // Must be a JSON object.
    let obj = match value {
        Value::Object(m) => m,
        _ => {
            return ParseResult::Error(make_error(
                None,
                error_codes::INVALID_REQUEST,
                "Invalid Request",
                None,
            ));
        }
    };

    // Extract `id` early so error responses can echo it back.
    let id = obj.get("id").cloned();

    // `jsonrpc` must be the string `"2.0"`.
    match obj.get("jsonrpc") {
        Some(Value::String(s)) if s == "2.0" => {}
        _ => {
            return ParseResult::Error(make_error(
                id,
                error_codes::INVALID_REQUEST,
                "Invalid Request: missing or invalid jsonrpc field",
                None,
            ));
        }
    }

    // `method` must be a string.
    let method = match obj.get("method") {
        Some(Value::String(s)) => s.clone(),
        _ => {
            return ParseResult::Error(make_error(
                id,
                error_codes::INVALID_REQUEST,
                "Invalid Request: missing or invalid method field",
                None,
            ));
        }
    };

    // `method` must be in the recognised set.
    if !RECOGNIZED_METHODS.contains(&method.as_str()) {
        return ParseResult::Error(make_error(
            id,
            error_codes::METHOD_NOT_FOUND,
            &format!("Method not found: {method}"),
            None,
        ));
    }

    let params = obj.get("params").cloned();

    let req = ParsedRequest {
        jsonrpc: "2.0".to_string(),
        id: id.clone(),
        method,
        params,
    };

    // Notifications have id absent or explicitly null.
    if matches!(&id, None | Some(Value::Null)) {
        ParseResult::Notification(req)
    } else {
        ParseResult::Request(req)
    }
}

/// Build a [`JsonRpcResponse`] carrying an error.
fn make_error(
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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn as_request(r: ParseResult) -> ParsedRequest {
        match r {
            ParseResult::Request(req) => req,
            other => panic!("expected Request, got {other:?}"),
        }
    }

    fn as_notification(r: ParseResult) -> ParsedRequest {
        match r {
            ParseResult::Notification(req) => req,
            other => panic!("expected Notification, got {other:?}"),
        }
    }

    fn as_error(r: ParseResult) -> JsonRpcResponse {
        match r {
            ParseResult::Error(resp) => resp,
            other => panic!("expected Error, got {other:?}"),
        }
    }

    fn parse_str(s: &str) -> ParseResult {
        match parse(s.as_bytes()) {
            Message::Single(r) => r,
            Message::Batch(_) => panic!("expected Single, got Batch"),
        }
    }

    fn parse_batch_str(s: &str) -> Vec<ParseResult> {
        match parse(s.as_bytes()) {
            Message::Batch(items) => items,
            Message::Single(_) => panic!("expected Batch, got Single"),
        }
    }

    // ── Table-driven tests ────────────────────────────────────────────────────

    #[test]
    fn valid_request_with_params() {
        let req = as_request(parse_str(
            r#"{"jsonrpc":"2.0","method":"tools/call","id":1,"params":{"name":"greet"}}"#,
        ));
        assert_eq!(req.method, "tools/call");
        assert_eq!(req.id, Some(json!(1)));
        assert_eq!(req.params, Some(json!({"name": "greet"})));
        assert!(!req.is_notification());
    }

    #[test]
    fn valid_request_string_id() {
        let req = as_request(parse_str(
            r#"{"jsonrpc":"2.0","method":"tools/list","id":"abc"}"#,
        ));
        assert_eq!(req.id, Some(json!("abc")));
    }

    #[test]
    fn valid_request_no_params() {
        let req = as_request(parse_str(
            r#"{"jsonrpc":"2.0","method":"ping","id":42}"#,
        ));
        assert_eq!(req.method, "ping");
        assert_eq!(req.params, None);
    }

    #[test]
    fn valid_notification_id_absent() {
        let note = as_notification(parse_str(
            r#"{"jsonrpc":"2.0","method":"initialized"}"#,
        ));
        assert_eq!(note.method, "initialized");
        assert!(note.is_notification());
    }

    #[test]
    fn valid_notification_id_null() {
        let note = as_notification(parse_str(
            r#"{"jsonrpc":"2.0","method":"initialized","id":null}"#,
        ));
        assert!(note.is_notification());
        assert_eq!(note.id, Some(Value::Null));
    }

    #[test]
    fn valid_batch_all_requests() {
        let items = parse_batch_str(
            r#"[
                {"jsonrpc":"2.0","method":"ping","id":1},
                {"jsonrpc":"2.0","method":"tools/list","id":2}
            ]"#,
        );
        assert_eq!(items.len(), 2);
        // Ordering preserved.
        let r0 = as_request(items.into_iter().next().expect("first item"));
        assert_eq!(r0.id, Some(json!(1)));
    }

    #[test]
    fn valid_batch_mixed_notification() {
        let items = parse_batch_str(
            r#"[
                {"jsonrpc":"2.0","method":"ping","id":1},
                {"jsonrpc":"2.0","method":"initialized"}
            ]"#,
        );
        assert_eq!(items.len(), 2);
        let mut it = items.into_iter();
        assert!(matches!(it.next(), Some(ParseResult::Request(_))));
        assert!(matches!(it.next(), Some(ParseResult::Notification(_))));
    }

    #[test]
    fn batch_with_one_invalid_item() {
        let items = parse_batch_str(
            r#"[
                {"jsonrpc":"2.0","method":"ping","id":1},
                {"jsonrpc":"1.0","method":"ping","id":2}
            ]"#,
        );
        assert_eq!(items.len(), 2);
        let mut it = items.into_iter();
        assert!(matches!(it.next(), Some(ParseResult::Request(_))));
        let err = as_error(it.next().expect("second item"));
        assert_eq!(err.error.as_ref().map(|e| e.code), Some(error_codes::INVALID_REQUEST));
    }

    #[test]
    fn parse_error_invalid_json() {
        let err = as_error(parse_str("not json at all {{{"));
        let e = err.error.as_ref().expect("error field");
        assert_eq!(e.code, error_codes::PARSE_ERROR);
    }

    #[test]
    fn invalid_request_not_an_object() {
        let err = as_error(parse_str("42"));
        assert_eq!(err.error.as_ref().map(|e| e.code), Some(error_codes::INVALID_REQUEST));
    }

    #[test]
    fn invalid_request_missing_jsonrpc() {
        let err = as_error(parse_str(
            r#"{"method":"ping","id":1}"#,
        ));
        assert_eq!(err.error.as_ref().map(|e| e.code), Some(error_codes::INVALID_REQUEST));
    }

    #[test]
    fn invalid_request_wrong_jsonrpc_version() {
        let err = as_error(parse_str(
            r#"{"jsonrpc":"1.0","method":"ping","id":1}"#,
        ));
        assert_eq!(err.error.as_ref().map(|e| e.code), Some(error_codes::INVALID_REQUEST));
        // id should be echoed back.
        assert_eq!(err.id, Some(json!(1)));
    }

    #[test]
    fn invalid_request_missing_method() {
        let err = as_error(parse_str(
            r#"{"jsonrpc":"2.0","id":1}"#,
        ));
        assert_eq!(err.error.as_ref().map(|e| e.code), Some(error_codes::INVALID_REQUEST));
    }

    #[test]
    fn method_not_found() {
        let err = as_error(parse_str(
            r#"{"jsonrpc":"2.0","method":"unknown/method","id":1}"#,
        ));
        assert_eq!(err.error.as_ref().map(|e| e.code), Some(error_codes::METHOD_NOT_FOUND));
        assert!(err.error.as_ref().map(|e| e.message.contains("unknown/method")).unwrap_or(false));
    }

    #[test]
    fn oversized_payload_returns_invalid_request_with_extension() {
        let oversized = vec![b'x'; MAX_BODY_SIZE + 1];
        let err = as_error(match parse(&oversized) {
            Message::Single(r) => r,
            Message::Batch(_) => panic!("expected Single"),
        });
        let e = err.error.as_ref().expect("error field");
        assert_eq!(e.code, error_codes::INVALID_REQUEST);
        assert_eq!(
            e.data,
            Some(json!({"reason": "payload_too_large"}))
        );
    }

    #[test]
    fn empty_batch_returns_invalid_request() {
        let err = as_error(parse_str("[]"));
        assert_eq!(err.error.as_ref().map(|e| e.code), Some(error_codes::INVALID_REQUEST));
    }

    #[test]
    fn all_recognized_methods_accepted() {
        for method in RECOGNIZED_METHODS {
            let body = format!(r#"{{"jsonrpc":"2.0","method":"{method}","id":1}}"#);
            let result = parse_str(&body);
            assert!(
                matches!(result, ParseResult::Request(_)),
                "method '{method}' should be recognized"
            );
        }
    }

    #[test]
    fn params_preserved_as_raw_value() {
        let raw_params = json!({"nested": {"array": [1, 2, 3], "bool": true}});
        let body = json!({
            "jsonrpc": "2.0",
            "method": "tools/call",
            "id": 1,
            "params": raw_params,
        })
        .to_string();
        let req = as_request(parse_str(&body));
        assert_eq!(req.params, Some(raw_params));
    }

    #[test]
    fn batch_order_preserved_for_large_batch() {
        let items_json: Vec<String> = (0..20)
            .map(|i| format!(r#"{{"jsonrpc":"2.0","method":"ping","id":{i}}}"#))
            .collect();
        let batch = format!("[{}]", items_json.join(","));
        let results = parse_batch_str(&batch);
        assert_eq!(results.len(), 20);
        for (i, result) in results.into_iter().enumerate() {
            let req = as_request(result);
            assert_eq!(req.id, Some(json!(i as u64)));
        }
    }
}
