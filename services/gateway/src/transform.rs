//! Response transformation engine.
//!
//! Applies a declarative pipeline of [`TransformOp`] steps to a raw upstream
//! API response, then wraps the final document as an MCP content array.
//!
//! # Design
//!
//! - **Pure**: [`apply`] has no I/O, no async, no side effects.
//! - **Pre-compiled**: JSONPath expressions are compiled once when building a
//!   [`TransformPipeline`] (at config-load time), not on every request.
//! - **Non-fatal ops**: individual transform failures emit [`TransformWarning`]
//!   values and continue; the pipeline never aborts mid-way.
//! - **MCP wrapping**: the final document is always returned as
//!   `[{"type":"text","text":"<json>"}]` regardless of pipeline length.
//!
//! # Usage
//!
//! ```ignore
//! let cfg: TransformPipelineConfig = serde_json::from_value(raw_json)?;
//! let pipeline = TransformPipeline::new(cfg)?;
//! // per-request:
//! let (mcp_content, warnings) = pipeline.apply(response_body)?;
//! ```

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Maximum upstream response size this engine will process (100 KB).
pub const MAX_RESPONSE_SIZE: usize = 100 * 1_024;

// ── Error / warning types ─────────────────────────────────────────────────────

/// Fatal errors that prevent the pipeline from running at all.
#[derive(Debug, Error)]
pub enum TransformError {
    /// The input exceeds [`MAX_RESPONSE_SIZE`].
    #[error("response body is {size} bytes; maximum allowed is {max} bytes")]
    ResponseTooLarge {
        /// Actual size of the input in bytes.
        size: usize,
        /// Maximum allowed size in bytes.
        max: usize,
    },
    /// The input is not valid JSON.
    #[error("upstream response is not valid JSON: {0}")]
    InvalidJson(#[from] serde_json::Error),
}

/// A non-fatal warning emitted when an individual operation cannot be applied.
///
/// The pipeline continues after producing a warning.
#[derive(Debug, Clone)]
pub struct TransformWarning {
    /// Zero-based index of the failing operation in the pipeline.
    pub op_index: usize,
    /// Canonical snake_case name of the operation type.
    pub op_name: &'static str,
    /// Human-readable description of the failure.
    pub message: String,
}

// ── Serde-derived op types ────────────────────────────────────────────────────

/// Type coercion target for [`TransformOp::TypeCoerce`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypeCoercion {
    /// Parse a JSON string as a number.
    StringToNumber,
    /// Stringify a JSON number.
    NumberToString,
    /// Parse `"true"/"false"/"1"/"0"/"yes"/"no"/"on"/"off"` as a boolean.
    StringToBool,
    /// Stringify a JSON boolean.
    BoolToString,
    /// Re-format a date string from one strftime pattern to another.
    DateFormat {
        /// strftime pattern the input string is parsed with.
        from_format: String,
        /// strftime pattern the output string is formatted with.
        to_format: String,
    },
}

/// Arithmetic operation for [`TransformOp::Arithmetic`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArithOp {
    /// `field = field + operand`
    Add,
    /// `field = field - operand`
    Subtract,
    /// `field = field * operand`
    Multiply,
    /// `field = field / operand` — divide-by-zero emits a warning, field unchanged.
    Divide,
}

/// String transformation for [`TransformOp::StringOp`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StringOperation {
    /// Append `suffix` to the field value.
    Concat {
        /// The string to append to the field value.
        suffix: String,
    },
    /// Strip leading and trailing ASCII whitespace.
    Trim,
    /// Convert to uppercase.
    Uppercase,
    /// Convert to lowercase.
    Lowercase,
    /// Truncate to at most `max_len` Unicode scalar values.
    Truncate {
        /// Maximum number of Unicode scalar values to retain.
        max_len: usize,
    },
}

/// One step in a declarative transform pipeline.
///
/// Variants are distinguished by an `"op"` field in their JSON representation,
/// e.g. `{"op":"field_rename","from":"x","to":"y"}`.
///
/// # Note on field naming
///
/// The `Arithmetic` and `StringOp` variants use `"kind"` internally instead
/// of `"op"` to avoid a serde conflict with the outer `tag = "op"`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum TransformOp {
    /// Apply a JSONPath expression to the document and store the matched
    /// value(s) at `target_field` in the (possibly promoted) root object.
    ///
    /// Multiple matches are stored as a JSON array. No match emits a warning.
    JsonpathExtract {
        /// JSONPath expression, e.g. `"$.items[*].id"`.
        path: String,
        /// Key at which to store the result in the root object.
        target_field: String,
    },
    /// Rename a top-level key in the root object.
    FieldRename {
        /// The key to rename.
        from: String,
        /// The new key name.
        to: String,
    },
    /// Coerce a top-level field to a different JSON type.
    TypeCoerce {
        /// Field to coerce (must exist in the root object).
        field: String,
        /// How to coerce it.
        coerce: TypeCoercion,
    },
    /// Apply arithmetic to a numeric field.
    ///
    /// Integer-valued numbers use `checked_add`/`checked_sub`/`checked_mul`
    /// to detect overflow precisely. A warning is emitted and the field is
    /// left unchanged on overflow or divide-by-zero.
    Arithmetic {
        /// Field to modify (must be a JSON number).
        field: String,
        /// The arithmetic operation to apply.
        kind: ArithOp,
        /// Literal operand.
        operand: f64,
    },
    /// Apply a string operation to a string field.
    StringOp {
        /// Field to modify (must be a JSON string).
        field: String,
        /// The string operation to apply.
        kind: StringOperation,
    },
    /// Flatten a nested array field one level deep.
    ///
    /// `[[1,2],[3,4]]` → `[1,2,3,4]`. Non-array inner elements pass through.
    ArrayFlatten {
        /// Field to flatten (must be a JSON array).
        field: String,
    },
    /// Keep only the listed fields in the root object; remove all others.
    SelectFields {
        /// Fields to retain.
        fields: Vec<String>,
    },
    /// Remove the listed fields from the root object.
    DropFields {
        /// Fields to remove (non-existent fields are silently ignored).
        fields: Vec<String>,
    },
}

/// Serialisable pipeline configuration — store this in `mcp_servers.config_json`.
///
/// Convert to a [`TransformPipeline`] with [`TransformPipeline::new`] to
/// pre-compile JSONPath expressions.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TransformPipelineConfig {
    /// Ordered list of transform operations.
    #[serde(default)]
    pub ops: Vec<TransformOp>,
}

// ── Pipeline build error ──────────────────────────────────────────────────────

/// Error returned when building a [`TransformPipeline`] from config fails.
#[derive(Debug, Error)]
pub enum PipelineBuildError {
    /// A JSONPath expression is syntactically invalid.
    #[error("invalid JSONPath expression at op[{index}]: {reason}")]
    InvalidJsonPath {
        /// Op index within the config ops slice.
        index: usize,
        /// Human-readable parse error.
        reason: String,
    },
}

// ── Runtime pipeline ──────────────────────────────────────────────────────────

/// Type-erased compiled JSONPath function.
///
/// `jsonpath_rust::JsonPath::find` returns a `Value` (a JSON array of all
/// matched nodes). The closure is `Send + Sync` so the pipeline can be shared
/// across request-handler tasks.
type CompiledJsonPath = Arc<dyn Fn(&Value) -> Value + Send + Sync>;

/// Pre-compiled transform pipeline.
///
/// Build once when loading server config; call [`TransformPipeline::apply`]
/// on every request.
pub struct TransformPipeline {
    config: TransformPipelineConfig,
    /// Compiled JSONPath closures, keyed by op index in `config.ops`.
    compiled_paths: HashMap<usize, CompiledJsonPath>,
}

impl TransformPipeline {
    /// Build a pipeline from config, compiling all JSONPath expressions.
    ///
    /// # Errors
    ///
    /// Returns [`PipelineBuildError::InvalidJsonPath`] if any `jsonpath_extract`
    /// op contains an invalid JSONPath expression.
    pub fn new(config: TransformPipelineConfig) -> Result<Self, PipelineBuildError> {
        let mut compiled_paths: HashMap<usize, CompiledJsonPath> = HashMap::new();

        for (i, op) in config.ops.iter().enumerate() {
            if let TransformOp::JsonpathExtract { path, .. } = op {
                let compiled = jsonpath_rust::JsonPath::<Value>::try_from(path.as_str())
                    .map_err(|e| PipelineBuildError::InvalidJsonPath {
                        index: i,
                        reason: e.to_string(),
                    })?;
                let boxed: CompiledJsonPath = Arc::new(move |v: &Value| compiled.find(v));
                compiled_paths.insert(i, boxed);
            }
        }

        Ok(Self { config, compiled_paths })
    }

    /// Return the original serialisable config.
    pub fn config(&self) -> &TransformPipelineConfig {
        &self.config
    }

    /// Apply the pipeline to a raw upstream response body.
    ///
    /// # Errors
    ///
    /// Returns [`TransformError::ResponseTooLarge`] if `input` exceeds
    /// [`MAX_RESPONSE_SIZE`].
    ///
    /// Returns [`TransformError::InvalidJson`] if `input` is not valid JSON.
    ///
    /// # Returns
    ///
    /// `(mcp_content, warnings)` where `mcp_content` is
    /// `[{"type":"text","text":"<json>"}]` and `warnings` lists any non-fatal
    /// operation failures.
    pub fn apply(
        &self,
        input: &str,
    ) -> Result<(Value, Vec<TransformWarning>), TransformError> {
        apply(self, input)
    }
}

// ── Core apply function ───────────────────────────────────────────────────────

/// Apply a pipeline to a raw upstream response body.
///
/// This free function is the implementation backing [`TransformPipeline::apply`].
pub fn apply(
    pipeline: &TransformPipeline,
    input: &str,
) -> Result<(Value, Vec<TransformWarning>), TransformError> {
    let size = input.len();
    if size > MAX_RESPONSE_SIZE {
        return Err(TransformError::ResponseTooLarge { size, max: MAX_RESPONSE_SIZE });
    }

    let mut doc: Value = serde_json::from_str(input)?;
    let mut warnings: Vec<TransformWarning> = Vec::new();

    for (i, op) in pipeline.config.ops.iter().enumerate() {
        match op {
            TransformOp::JsonpathExtract { target_field, .. } => {
                if let Some(compiled) = pipeline.compiled_paths.get(&i) {
                    apply_jsonpath_extract(&mut doc, compiled, target_field, i, &mut warnings);
                }
            }
            TransformOp::FieldRename { from, to } => {
                apply_field_rename(&mut doc, from, to, i, &mut warnings);
            }
            TransformOp::TypeCoerce { field, coerce } => {
                apply_type_coerce(&mut doc, field, coerce, i, &mut warnings);
            }
            TransformOp::Arithmetic { field, kind, operand } => {
                apply_arithmetic(&mut doc, field, kind, *operand, i, &mut warnings);
            }
            TransformOp::StringOp { field, kind } => {
                apply_string_op(&mut doc, field, kind, i, &mut warnings);
            }
            TransformOp::ArrayFlatten { field } => {
                apply_array_flatten(&mut doc, field, i, &mut warnings);
            }
            TransformOp::SelectFields { fields } => {
                apply_select_fields(&mut doc, fields, i, &mut warnings);
            }
            TransformOp::DropFields { fields } => {
                apply_drop_fields(&mut doc, fields, i, &mut warnings);
            }
        }
    }

    Ok((wrap_as_mcp_content(doc), warnings))
}

// ── Individual operation helpers ──────────────────────────────────────────────

fn apply_jsonpath_extract(
    doc: &mut Value,
    compiled: &CompiledJsonPath,
    target_field: &str,
    op_index: usize,
    warnings: &mut Vec<TransformWarning>,
) {
    // jsonpath_rust::JsonPath::find() returns a Value::Array of all matches.
    let result_value = compiled(doc);

    let matches: Vec<Value> = match result_value {
        Value::Array(arr) => arr,
        other if other.is_null() => vec![],
        other => vec![other],
    };

    if matches.is_empty() {
        warnings.push(TransformWarning {
            op_index,
            op_name: "jsonpath_extract",
            message: format!(
                "JSONPath matched no values; '{}' will not be set",
                target_field
            ),
        });
        return;
    }

    let extracted = if matches.len() == 1 {
        // SAFETY: just checked len == 1
        #[allow(clippy::unwrap_used)]
        matches.into_iter().next().unwrap()
    } else {
        Value::Array(matches)
    };

    // Promote non-object documents to an empty object before writing.
    if !doc.is_object() {
        *doc = Value::Object(Map::new());
    }
    if let Value::Object(map) = doc {
        map.insert(target_field.to_string(), extracted);
    }
}

fn apply_field_rename(
    doc: &mut Value,
    from: &str,
    to: &str,
    op_index: usize,
    warnings: &mut Vec<TransformWarning>,
) {
    match doc {
        Value::Object(map) => {
            if let Some(val) = map.remove(from) {
                map.insert(to.to_string(), val);
            } else {
                warnings.push(TransformWarning {
                    op_index,
                    op_name: "field_rename",
                    message: format!("field '{}' not found", from),
                });
            }
        }
        _ => {
            warnings.push(TransformWarning {
                op_index,
                op_name: "field_rename",
                message: "document is not an object".to_string(),
            });
        }
    }
}

fn apply_type_coerce(
    doc: &mut Value,
    field: &str,
    coerce: &TypeCoercion,
    op_index: usize,
    warnings: &mut Vec<TransformWarning>,
) {
    let map = match doc {
        Value::Object(m) => m,
        _ => {
            warnings.push(TransformWarning {
                op_index,
                op_name: "type_coerce",
                message: "document is not an object".to_string(),
            });
            return;
        }
    };

    let current = match map.get(field) {
        Some(v) => v.clone(),
        None => {
            warnings.push(TransformWarning {
                op_index,
                op_name: "type_coerce",
                message: format!("field '{}' not found", field),
            });
            return;
        }
    };

    match coerce_value(&current, coerce) {
        Ok(new_val) => {
            map.insert(field.to_string(), new_val);
        }
        Err(msg) => {
            warnings.push(TransformWarning {
                op_index,
                op_name: "type_coerce",
                message: format!("field '{}': {}", field, msg),
            });
        }
    }
}

fn coerce_value(val: &Value, coerce: &TypeCoercion) -> Result<Value, String> {
    match coerce {
        TypeCoercion::StringToNumber => {
            let s = val.as_str().ok_or_else(|| "value is not a string".to_string())?;
            let n: f64 =
                s.parse().map_err(|_| format!("'{}' cannot be parsed as a number", s))?;
            let num = serde_json::Number::from_f64(n)
                .ok_or_else(|| format!("'{}' is not a finite number", s))?;
            Ok(Value::Number(num))
        }
        TypeCoercion::NumberToString => match val {
            Value::Number(n) => Ok(Value::String(n.to_string())),
            _ => Err("value is not a number".to_string()),
        },
        TypeCoercion::StringToBool => {
            let s = val.as_str().ok_or_else(|| "value is not a string".to_string())?;
            match s.to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" | "on" => Ok(Value::Bool(true)),
                "false" | "0" | "no" | "off" => Ok(Value::Bool(false)),
                other => Err(format!("'{}' cannot be parsed as a boolean", other)),
            }
        }
        TypeCoercion::BoolToString => match val {
            Value::Bool(b) => Ok(Value::String(b.to_string())),
            _ => Err("value is not a boolean".to_string()),
        },
        TypeCoercion::DateFormat { from_format, to_format } => {
            let s = val.as_str().ok_or_else(|| "value is not a string".to_string())?;
            let dt = NaiveDateTime::parse_from_str(s, from_format).map_err(|e| {
                format!("date parse error with format '{}': {}", from_format, e)
            })?;
            Ok(Value::String(dt.format(to_format).to_string()))
        }
    }
}

fn apply_arithmetic(
    doc: &mut Value,
    field: &str,
    op: &ArithOp,
    operand: f64,
    op_index: usize,
    warnings: &mut Vec<TransformWarning>,
) {
    let map = match doc {
        Value::Object(m) => m,
        _ => {
            warnings.push(TransformWarning {
                op_index,
                op_name: "arithmetic",
                message: "document is not an object".to_string(),
            });
            return;
        }
    };

    let current = match map.get(field) {
        Some(Value::Number(n)) => match n.as_f64() {
            Some(f) => f,
            None => {
                warnings.push(TransformWarning {
                    op_index,
                    op_name: "arithmetic",
                    message: format!("field '{}' is not representable as f64", field),
                });
                return;
            }
        },
        Some(_) => {
            warnings.push(TransformWarning {
                op_index,
                op_name: "arithmetic",
                message: format!("field '{}' is not a number", field),
            });
            return;
        }
        None => {
            warnings.push(TransformWarning {
                op_index,
                op_name: "arithmetic",
                message: format!("field '{}' not found", field),
            });
            return;
        }
    };

    match compute_arithmetic(current, op, operand) {
        Ok(result) => {
            // Preserve integer representation when the result is whole-valued.
            let number = if result.fract() == 0.0
                && result >= i64::MIN as f64
                && result <= i64::MAX as f64
            {
                serde_json::Number::from(result as i64)
            } else {
                match serde_json::Number::from_f64(result) {
                    Some(n) => n,
                    None => {
                        warnings.push(TransformWarning {
                            op_index,
                            op_name: "arithmetic",
                            message: format!(
                                "field '{}': result {} is not a valid JSON number",
                                field, result
                            ),
                        });
                        return;
                    }
                }
            };
            map.insert(field.to_string(), Value::Number(number));
        }
        Err(msg) => {
            warnings.push(TransformWarning {
                op_index,
                op_name: "arithmetic",
                message: format!("field '{}': {}", field, msg),
            });
        }
    }
}

/// Compute an arithmetic result.
///
/// Uses `i64::checked_add/sub/mul` when both operands are integer-valued to
/// detect overflow precisely.  Returns `Err` on divide-by-zero or overflow.
fn compute_arithmetic(current: f64, op: &ArithOp, operand: f64) -> Result<f64, String> {
    match op {
        ArithOp::Divide => {
            if operand == 0.0 {
                return Err("division by zero".to_string());
            }
            let result = current / operand;
            if result.is_finite() { Ok(result) } else { Err("arithmetic overflow".to_string()) }
        }
        ArithOp::Add => {
            if let (Some(a), Some(b)) = (as_i64(current), as_i64(operand)) {
                a.checked_add(b)
                    .map(|r| r as f64)
                    .ok_or_else(|| "integer overflow in addition".to_string())
            } else {
                let r = current + operand;
                if r.is_finite() { Ok(r) } else { Err("arithmetic overflow".to_string()) }
            }
        }
        ArithOp::Subtract => {
            if let (Some(a), Some(b)) = (as_i64(current), as_i64(operand)) {
                a.checked_sub(b)
                    .map(|r| r as f64)
                    .ok_or_else(|| "integer overflow in subtraction".to_string())
            } else {
                let r = current - operand;
                if r.is_finite() { Ok(r) } else { Err("arithmetic overflow".to_string()) }
            }
        }
        ArithOp::Multiply => {
            if let (Some(a), Some(b)) = (as_i64(current), as_i64(operand)) {
                a.checked_mul(b)
                    .map(|r| r as f64)
                    .ok_or_else(|| "integer overflow in multiplication".to_string())
            } else {
                let r = current * operand;
                if r.is_finite() { Ok(r) } else { Err("arithmetic overflow".to_string()) }
            }
        }
    }
}

/// Try to represent an `f64` as `i64` without loss of precision.
fn as_i64(v: f64) -> Option<i64> {
    if v.fract() == 0.0 && v >= i64::MIN as f64 && v <= i64::MAX as f64 {
        Some(v as i64)
    } else {
        None
    }
}

fn apply_string_op(
    doc: &mut Value,
    field: &str,
    op: &StringOperation,
    op_index: usize,
    warnings: &mut Vec<TransformWarning>,
) {
    let map = match doc {
        Value::Object(m) => m,
        _ => {
            warnings.push(TransformWarning {
                op_index,
                op_name: "string_op",
                message: "document is not an object".to_string(),
            });
            return;
        }
    };

    let current: String = match map.get(field) {
        Some(Value::String(s)) => s.clone(),
        Some(_) => {
            warnings.push(TransformWarning {
                op_index,
                op_name: "string_op",
                message: format!("field '{}' is not a string", field),
            });
            return;
        }
        None => {
            warnings.push(TransformWarning {
                op_index,
                op_name: "string_op",
                message: format!("field '{}' not found", field),
            });
            return;
        }
    };

    let new_val = match op {
        StringOperation::Concat { suffix } => format!("{}{}", current, suffix),
        StringOperation::Trim => current.trim().to_string(),
        StringOperation::Uppercase => current.to_uppercase(),
        StringOperation::Lowercase => current.to_lowercase(),
        StringOperation::Truncate { max_len } => {
            current.chars().take(*max_len).collect::<String>()
        }
    };

    map.insert(field.to_string(), Value::String(new_val));
}

fn apply_array_flatten(
    doc: &mut Value,
    field: &str,
    op_index: usize,
    warnings: &mut Vec<TransformWarning>,
) {
    let map = match doc {
        Value::Object(m) => m,
        _ => {
            warnings.push(TransformWarning {
                op_index,
                op_name: "array_flatten",
                message: "document is not an object".to_string(),
            });
            return;
        }
    };

    let arr = match map.get(field) {
        Some(Value::Array(a)) => a.clone(),
        Some(_) => {
            warnings.push(TransformWarning {
                op_index,
                op_name: "array_flatten",
                message: format!("field '{}' is not an array", field),
            });
            return;
        }
        None => {
            warnings.push(TransformWarning {
                op_index,
                op_name: "array_flatten",
                message: format!("field '{}' not found", field),
            });
            return;
        }
    };

    let mut flattened: Vec<Value> = Vec::with_capacity(arr.len());
    for item in arr {
        match item {
            Value::Array(inner) => flattened.extend(inner),
            other => flattened.push(other),
        }
    }

    map.insert(field.to_string(), Value::Array(flattened));
}

fn apply_select_fields(
    doc: &mut Value,
    fields: &[String],
    op_index: usize,
    warnings: &mut Vec<TransformWarning>,
) {
    match doc {
        Value::Object(map) => {
            let keep: std::collections::HashSet<&str> =
                fields.iter().map(String::as_str).collect();
            map.retain(|k, _| keep.contains(k.as_str()));
        }
        _ => {
            warnings.push(TransformWarning {
                op_index,
                op_name: "select_fields",
                message: "document is not an object".to_string(),
            });
        }
    }
}

fn apply_drop_fields(
    doc: &mut Value,
    fields: &[String],
    _op_index: usize,
    _warnings: &mut Vec<TransformWarning>,
) {
    if let Value::Object(map) = doc {
        for field in fields {
            map.remove(field.as_str());
        }
    }
}

// ── MCP content wrapping ──────────────────────────────────────────────────────

/// Wrap a JSON value as an MCP text content item.
///
/// The document is serialised to a JSON string and embedded as the `text`
/// property of a `{"type":"text","text":"..."}` object.
fn wrap_as_mcp_content(doc: Value) -> Value {
    // Serialisation is infallible for any well-formed serde_json::Value.
    let text = serde_json::to_string(&doc).unwrap_or_else(|_| "null".to_string());
    Value::Array(vec![serde_json::json!({ "type": "text", "text": text })])
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use serde_json::json;

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn make_pipeline(ops: Vec<TransformOp>) -> TransformPipeline {
        TransformPipeline::new(TransformPipelineConfig { ops }).expect("build pipeline")
    }

    fn run(pipeline: &TransformPipeline, input: &str) -> (Value, Vec<TransformWarning>) {
        pipeline.apply(input).expect("apply pipeline")
    }

    fn unwrap_text(mcp: &Value) -> Value {
        let text = mcp[0]["text"].as_str().expect("text field");
        serde_json::from_str(text).expect("parse text as JSON")
    }

    // ── Empty pipeline ────────────────────────────────────────────────────────

    #[test]
    fn empty_pipeline_returns_input_verbatim_wrapped() {
        let pipeline = make_pipeline(vec![]);
        let input = r#"{"name":"alice","age":30}"#;
        let (content, warnings) = run(&pipeline, input);
        assert!(warnings.is_empty());
        let doc = unwrap_text(&content);
        assert_eq!(doc["name"], "alice");
        assert_eq!(doc["age"], 30);
    }

    // ── Size limit / invalid JSON ─────────────────────────────────────────────

    #[test]
    fn oversized_input_returns_too_large_error() {
        let pipeline = make_pipeline(vec![]);
        let big = "x".repeat(MAX_RESPONSE_SIZE + 1);
        let result = pipeline.apply(&big);
        assert!(
            matches!(result, Err(TransformError::ResponseTooLarge { .. })),
            "expected ResponseTooLarge"
        );
    }

    #[test]
    fn invalid_json_returns_error() {
        let pipeline = make_pipeline(vec![]);
        let result = pipeline.apply("{not valid json");
        assert!(matches!(result, Err(TransformError::InvalidJson(_))));
    }

    // ── jsonpath_extract ──────────────────────────────────────────────────────

    #[test]
    fn jsonpath_extract_single_match() {
        let pipeline = make_pipeline(vec![TransformOp::JsonpathExtract {
            path: "$.user.id".to_string(),
            target_field: "user_id".to_string(),
        }]);
        let input = json!({ "user": { "id": 42, "name": "bob" } }).to_string();
        let (content, warnings) = run(&pipeline, &input);
        assert!(warnings.is_empty(), "unexpected warnings: {:?}", warnings);
        let doc = unwrap_text(&content);
        assert_eq!(doc["user_id"], 42);
    }

    #[test]
    fn jsonpath_extract_no_match_emits_warning() {
        let pipeline = make_pipeline(vec![TransformOp::JsonpathExtract {
            path: "$.missing.path".to_string(),
            target_field: "x".to_string(),
        }]);
        let input = json!({ "a": 1 }).to_string();
        let (content, warnings) = run(&pipeline, &input);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].op_name, "jsonpath_extract");
        let doc = unwrap_text(&content);
        assert!(doc.get("x").is_none());
    }

    // ── field_rename ──────────────────────────────────────────────────────────

    #[test]
    fn field_rename_valid() {
        let pipeline = make_pipeline(vec![TransformOp::FieldRename {
            from: "old_name".to_string(),
            to: "new_name".to_string(),
        }]);
        let input = json!({ "old_name": "value" }).to_string();
        let (content, warnings) = run(&pipeline, &input);
        assert!(warnings.is_empty());
        let doc = unwrap_text(&content);
        assert_eq!(doc["new_name"], "value");
        assert!(doc.get("old_name").is_none());
    }

    #[test]
    fn field_rename_missing_field_emits_warning() {
        let pipeline = make_pipeline(vec![TransformOp::FieldRename {
            from: "nonexistent".to_string(),
            to: "new".to_string(),
        }]);
        let input = json!({ "a": 1 }).to_string();
        let (_, warnings) = run(&pipeline, &input);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].op_name, "field_rename");
    }

    // ── type_coerce ───────────────────────────────────────────────────────────

    #[test]
    fn type_coerce_string_to_number_valid() {
        let pipeline = make_pipeline(vec![TransformOp::TypeCoerce {
            field: "n".to_string(),
            coerce: TypeCoercion::StringToNumber,
        }]);
        let input = json!({ "n": "3.14" }).to_string();
        let (content, warnings) = run(&pipeline, &input);
        assert!(warnings.is_empty());
        let doc = unwrap_text(&content);
        let val = doc["n"].as_f64().expect("numeric");
        assert!((val - 3.14).abs() < 1e-9);
    }

    #[test]
    fn type_coerce_string_to_number_invalid_emits_warning() {
        let pipeline = make_pipeline(vec![TransformOp::TypeCoerce {
            field: "n".to_string(),
            coerce: TypeCoercion::StringToNumber,
        }]);
        let input = json!({ "n": "not-a-number" }).to_string();
        let (_, warnings) = run(&pipeline, &input);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].op_name, "type_coerce");
    }

    #[test]
    fn type_coerce_number_to_string_valid() {
        let pipeline = make_pipeline(vec![TransformOp::TypeCoerce {
            field: "n".to_string(),
            coerce: TypeCoercion::NumberToString,
        }]);
        let input = json!({ "n": 99 }).to_string();
        let (content, warnings) = run(&pipeline, &input);
        assert!(warnings.is_empty());
        let doc = unwrap_text(&content);
        assert_eq!(doc["n"], "99");
    }

    #[test]
    fn type_coerce_string_to_bool_valid() {
        let pipeline = make_pipeline(vec![TransformOp::TypeCoerce {
            field: "b".to_string(),
            coerce: TypeCoercion::StringToBool,
        }]);
        let input = json!({ "b": "yes" }).to_string();
        let (content, warnings) = run(&pipeline, &input);
        assert!(warnings.is_empty());
        let doc = unwrap_text(&content);
        assert_eq!(doc["b"], true);
    }

    #[test]
    fn type_coerce_bool_to_string_valid() {
        let pipeline = make_pipeline(vec![TransformOp::TypeCoerce {
            field: "flag".to_string(),
            coerce: TypeCoercion::BoolToString,
        }]);
        let input = json!({ "flag": false }).to_string();
        let (content, warnings) = run(&pipeline, &input);
        assert!(warnings.is_empty());
        let doc = unwrap_text(&content);
        assert_eq!(doc["flag"], "false");
    }

    #[test]
    fn type_coerce_date_format_valid() {
        let pipeline = make_pipeline(vec![TransformOp::TypeCoerce {
            field: "ts".to_string(),
            coerce: TypeCoercion::DateFormat {
                from_format: "%Y-%m-%d %H:%M:%S".to_string(),
                to_format: "%d/%m/%Y".to_string(),
            },
        }]);
        let input = json!({ "ts": "2024-06-15 10:30:00" }).to_string();
        let (content, warnings) = run(&pipeline, &input);
        assert!(warnings.is_empty());
        let doc = unwrap_text(&content);
        assert_eq!(doc["ts"], "15/06/2024");
    }

    #[test]
    fn type_coerce_date_format_invalid_emits_warning() {
        let pipeline = make_pipeline(vec![TransformOp::TypeCoerce {
            field: "ts".to_string(),
            coerce: TypeCoercion::DateFormat {
                from_format: "%Y-%m-%d".to_string(),
                to_format: "%d/%m/%Y".to_string(),
            },
        }]);
        let input = json!({ "ts": "not-a-date" }).to_string();
        let (_, warnings) = run(&pipeline, &input);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].op_name, "type_coerce");
    }

    // ── arithmetic ────────────────────────────────────────────────────────────

    #[test]
    fn arithmetic_add_valid() {
        let pipeline = make_pipeline(vec![TransformOp::Arithmetic {
            field: "n".to_string(),
            kind: ArithOp::Add,
            operand: 10.0,
        }]);
        let input = json!({ "n": 5 }).to_string();
        let (content, warnings) = run(&pipeline, &input);
        assert!(warnings.is_empty());
        let doc = unwrap_text(&content);
        assert_eq!(doc["n"], 15);
    }

    #[test]
    fn arithmetic_subtract_valid() {
        let pipeline = make_pipeline(vec![TransformOp::Arithmetic {
            field: "val".to_string(),
            kind: ArithOp::Subtract,
            operand: 3.0,
        }]);
        let input = json!({ "val": 10 }).to_string();
        let (content, warnings) = run(&pipeline, &input);
        assert!(warnings.is_empty());
        let doc = unwrap_text(&content);
        assert_eq!(doc["val"], 7);
    }

    #[test]
    fn arithmetic_multiply_valid() {
        let pipeline = make_pipeline(vec![TransformOp::Arithmetic {
            field: "price".to_string(),
            kind: ArithOp::Multiply,
            operand: 2.0,
        }]);
        let input = json!({ "price": 5 }).to_string();
        let (content, warnings) = run(&pipeline, &input);
        assert!(warnings.is_empty());
        let doc = unwrap_text(&content);
        assert_eq!(doc["price"], 10);
    }

    #[test]
    fn arithmetic_divide_valid() {
        let pipeline = make_pipeline(vec![TransformOp::Arithmetic {
            field: "n".to_string(),
            kind: ArithOp::Divide,
            operand: 4.0,
        }]);
        let input = json!({ "n": 20 }).to_string();
        let (content, warnings) = run(&pipeline, &input);
        assert!(warnings.is_empty());
        let doc = unwrap_text(&content);
        assert_eq!(doc["n"], 5);
    }

    #[test]
    fn arithmetic_divide_by_zero_emits_warning_field_unchanged() {
        let pipeline = make_pipeline(vec![TransformOp::Arithmetic {
            field: "n".to_string(),
            kind: ArithOp::Divide,
            operand: 0.0,
        }]);
        let input = json!({ "n": 42 }).to_string();
        let (content, warnings) = run(&pipeline, &input);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].op_name, "arithmetic");
        assert!(warnings[0].message.contains("zero"), "message: {}", warnings[0].message);
        // Field must be unchanged
        let doc = unwrap_text(&content);
        assert_eq!(doc["n"], 42);
    }

    #[test]
    fn arithmetic_integer_overflow_emits_warning_field_unchanged() {
        let pipeline = make_pipeline(vec![TransformOp::Arithmetic {
            field: "n".to_string(),
            kind: ArithOp::Multiply,
            operand: i64::MAX as f64,
        }]);
        let input = json!({ "n": i64::MAX }).to_string();
        let (content, warnings) = run(&pipeline, &input);
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].message.contains("overflow"),
            "message: {}",
            warnings[0].message
        );
        let doc = unwrap_text(&content);
        assert_eq!(doc["n"], i64::MAX);
    }

    #[test]
    fn arithmetic_on_non_number_emits_warning() {
        let pipeline = make_pipeline(vec![TransformOp::Arithmetic {
            field: "s".to_string(),
            kind: ArithOp::Add,
            operand: 1.0,
        }]);
        let input = json!({ "s": "hello" }).to_string();
        let (_, warnings) = run(&pipeline, &input);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].op_name, "arithmetic");
    }

    // ── string_op ─────────────────────────────────────────────────────────────

    #[test]
    fn string_op_uppercase_valid() {
        let pipeline = make_pipeline(vec![TransformOp::StringOp {
            field: "s".to_string(),
            kind: StringOperation::Uppercase,
        }]);
        let input = json!({ "s": "hello" }).to_string();
        let (content, warnings) = run(&pipeline, &input);
        assert!(warnings.is_empty());
        let doc = unwrap_text(&content);
        assert_eq!(doc["s"], "HELLO");
    }

    #[test]
    fn string_op_lowercase_valid() {
        let pipeline = make_pipeline(vec![TransformOp::StringOp {
            field: "s".to_string(),
            kind: StringOperation::Lowercase,
        }]);
        let input = json!({ "s": "WORLD" }).to_string();
        let (content, warnings) = run(&pipeline, &input);
        assert!(warnings.is_empty());
        let doc = unwrap_text(&content);
        assert_eq!(doc["s"], "world");
    }

    #[test]
    fn string_op_trim_valid() {
        let pipeline = make_pipeline(vec![TransformOp::StringOp {
            field: "s".to_string(),
            kind: StringOperation::Trim,
        }]);
        let input = json!({ "s": "  padded  " }).to_string();
        let (content, warnings) = run(&pipeline, &input);
        assert!(warnings.is_empty());
        let doc = unwrap_text(&content);
        assert_eq!(doc["s"], "padded");
    }

    #[test]
    fn string_op_concat_valid() {
        let pipeline = make_pipeline(vec![TransformOp::StringOp {
            field: "s".to_string(),
            kind: StringOperation::Concat { suffix: "_suffix".to_string() },
        }]);
        let input = json!({ "s": "base" }).to_string();
        let (content, warnings) = run(&pipeline, &input);
        assert!(warnings.is_empty());
        let doc = unwrap_text(&content);
        assert_eq!(doc["s"], "base_suffix");
    }

    #[test]
    fn string_op_truncate_valid() {
        let pipeline = make_pipeline(vec![TransformOp::StringOp {
            field: "s".to_string(),
            kind: StringOperation::Truncate { max_len: 3 },
        }]);
        let input = json!({ "s": "abcdef" }).to_string();
        let (content, warnings) = run(&pipeline, &input);
        assert!(warnings.is_empty());
        let doc = unwrap_text(&content);
        assert_eq!(doc["s"], "abc");
    }

    #[test]
    fn string_op_on_non_string_emits_warning() {
        let pipeline = make_pipeline(vec![TransformOp::StringOp {
            field: "n".to_string(),
            kind: StringOperation::Uppercase,
        }]);
        let input = json!({ "n": 42 }).to_string();
        let (_, warnings) = run(&pipeline, &input);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].op_name, "string_op");
    }

    // ── array_flatten ─────────────────────────────────────────────────────────

    #[test]
    fn array_flatten_valid() {
        let pipeline = make_pipeline(vec![TransformOp::ArrayFlatten {
            field: "items".to_string(),
        }]);
        let input = json!({ "items": [[1, 2], [3, 4], 5] }).to_string();
        let (content, warnings) = run(&pipeline, &input);
        assert!(warnings.is_empty());
        let doc = unwrap_text(&content);
        assert_eq!(doc["items"], json!([1, 2, 3, 4, 5]));
    }

    #[test]
    fn array_flatten_non_array_emits_warning() {
        let pipeline = make_pipeline(vec![TransformOp::ArrayFlatten {
            field: "x".to_string(),
        }]);
        let input = json!({ "x": "not-an-array" }).to_string();
        let (_, warnings) = run(&pipeline, &input);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].op_name, "array_flatten");
    }

    // ── select_fields ─────────────────────────────────────────────────────────

    #[test]
    fn select_fields_keeps_only_listed() {
        let pipeline = make_pipeline(vec![TransformOp::SelectFields {
            fields: vec!["a".to_string(), "c".to_string()],
        }]);
        let input = json!({ "a": 1, "b": 2, "c": 3 }).to_string();
        let (content, warnings) = run(&pipeline, &input);
        assert!(warnings.is_empty());
        let doc = unwrap_text(&content);
        assert_eq!(doc["a"], 1);
        assert!(doc.get("b").is_none());
        assert_eq!(doc["c"], 3);
    }

    #[test]
    fn select_fields_on_non_object_emits_warning() {
        let pipeline = make_pipeline(vec![TransformOp::SelectFields {
            fields: vec!["a".to_string()],
        }]);
        let input = json!([1, 2, 3]).to_string();
        let (_, warnings) = run(&pipeline, &input);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].op_name, "select_fields");
    }

    // ── drop_fields ───────────────────────────────────────────────────────────

    #[test]
    fn drop_fields_removes_listed() {
        let pipeline = make_pipeline(vec![TransformOp::DropFields {
            fields: vec!["secret".to_string()],
        }]);
        let input = json!({ "id": 1, "secret": "pw123" }).to_string();
        let (content, warnings) = run(&pipeline, &input);
        assert!(warnings.is_empty());
        let doc = unwrap_text(&content);
        assert_eq!(doc["id"], 1);
        assert!(doc.get("secret").is_none());
    }

    #[test]
    fn drop_fields_missing_field_no_warning() {
        let pipeline = make_pipeline(vec![TransformOp::DropFields {
            fields: vec!["nonexistent".to_string()],
        }]);
        let input = json!({ "a": 1 }).to_string();
        let (_, warnings) = run(&pipeline, &input);
        assert!(warnings.is_empty());
    }

    // ── pipeline-level behaviour ──────────────────────────────────────────────

    #[test]
    fn failed_op_does_not_abort_pipeline() {
        let pipeline = make_pipeline(vec![
            TransformOp::FieldRename {
                from: "missing".to_string(),
                to: "x".to_string(),
            },
            TransformOp::FieldRename {
                from: "a".to_string(),
                to: "b".to_string(),
            },
        ]);
        let input = json!({ "a": 1 }).to_string();
        let (content, warnings) = run(&pipeline, &input);
        assert_eq!(warnings.len(), 1, "first op warning only");
        let doc = unwrap_text(&content);
        assert_eq!(doc["b"], 1, "second op still applied");
    }

    #[test]
    fn mcp_content_wrapping_shape() {
        let pipeline = make_pipeline(vec![]);
        let (content, _) = run(&pipeline, r#"{"k":"v"}"#);
        assert!(content.is_array());
        let arr = content.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["type"], "text");
        assert!(arr[0]["text"].is_string());
    }

    #[test]
    fn pipeline_config_roundtrips_via_serde() {
        let config = TransformPipelineConfig {
            ops: vec![
                TransformOp::JsonpathExtract {
                    path: "$.a".to_string(),
                    target_field: "b".to_string(),
                },
                TransformOp::FieldRename {
                    from: "x".to_string(),
                    to: "y".to_string(),
                },
                TransformOp::Arithmetic {
                    field: "n".to_string(),
                    kind: ArithOp::Add,
                    operand: 1.0,
                },
            ],
        };
        let json = serde_json::to_value(&config).expect("serialize");
        let back: TransformPipelineConfig =
            serde_json::from_value(json).expect("deserialize");
        assert_eq!(back.ops.len(), 3);
    }

    #[test]
    fn jsonpath_multiple_matches_stored_as_array() {
        let pipeline = make_pipeline(vec![TransformOp::JsonpathExtract {
            path: "$.items[*].id".to_string(),
            target_field: "ids".to_string(),
        }]);
        let input = json!({ "items": [{"id": 1}, {"id": 2}, {"id": 3}] }).to_string();
        let (content, warnings) = run(&pipeline, &input);
        assert!(warnings.is_empty(), "unexpected warnings: {:?}", warnings);
        let doc = unwrap_text(&content);
        assert_eq!(doc["ids"], json!([1, 2, 3]));
    }
}
