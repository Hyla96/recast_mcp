//! Benchmark: 10-operation transform pipeline on a ~10 KB JSON response.
//!
//! Since `mcp-gateway` is a binary crate, this bench duplicates the minimal
//! types needed to exercise the transformation logic inline.  The logic is
//! identical to the production code in `gateway::transform`.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use serde_json::{json, Map, Value};

// ── Inline transform logic (mirrors gateway::transform) ───────────────────────

const MAX_RESPONSE_SIZE: usize = 100 * 1_024;

enum Op {
    DropField(String),
    SelectFields(Vec<String>),
    FieldRename { from: String, to: String },
    ArithmeticAdd { field: String, operand: f64 },
    ArithmeticMultiply { field: String, operand: f64 },
    JsonpathExtract { path: std::sync::Arc<dyn Fn(&Value) -> Value + Send + Sync>, target: String },
}

fn apply_pipeline(ops: &[Op], input: &str) -> Option<Value> {
    if input.len() > MAX_RESPONSE_SIZE {
        return None;
    }
    let mut doc: Value = serde_json::from_str(input).ok()?;

    for op in ops {
        match op {
            Op::DropField(f) => {
                if let Value::Object(m) = &mut doc {
                    m.remove(f.as_str());
                }
            }
            Op::SelectFields(fields) => {
                if let Value::Object(m) = &mut doc {
                    let keep: std::collections::HashSet<&str> =
                        fields.iter().map(String::as_str).collect();
                    m.retain(|k, _| keep.contains(k.as_str()));
                }
            }
            Op::FieldRename { from, to } => {
                if let Value::Object(m) = &mut doc {
                    if let Some(v) = m.remove(from.as_str()) {
                        m.insert(to.clone(), v);
                    }
                }
            }
            Op::ArithmeticAdd { field, operand } => {
                if let Value::Object(m) = &mut doc {
                    if let Some(Value::Number(n)) = m.get(field) {
                        if let Some(f) = n.as_f64() {
                            let result = f + operand;
                            if let Some(num) = serde_json::Number::from_f64(result) {
                                m.insert(field.clone(), Value::Number(num));
                            }
                        }
                    }
                }
            }
            Op::ArithmeticMultiply { field, operand } => {
                if let Value::Object(m) = &mut doc {
                    if let Some(Value::Number(n)) = m.get(field) {
                        if let Some(f) = n.as_f64() {
                            let result = f * operand;
                            if let Some(num) = serde_json::Number::from_f64(result) {
                                m.insert(field.clone(), Value::Number(num));
                            }
                        }
                    }
                }
            }
            Op::JsonpathExtract { path, target } => {
                let result_val = path(&doc);
                let matches: Vec<Value> = match result_val {
                    Value::Array(arr) => arr,
                    other if other.is_null() => vec![],
                    other => vec![other],
                };
                if !matches.is_empty() {
                    let extracted = if matches.len() == 1 {
                        matches.into_iter().next().unwrap_or(Value::Null)
                    } else {
                        Value::Array(matches)
                    };
                    if !doc.is_object() {
                        doc = Value::Object(Map::new());
                    }
                    if let Value::Object(m) = &mut doc {
                        m.insert(target.clone(), extracted);
                    }
                }
            }
        }
    }

    // Wrap as MCP content
    let text = serde_json::to_string(&doc).unwrap_or_else(|_| "null".to_string());
    Some(json!([{ "type": "text", "text": text }]))
}

// ── Benchmark setup ───────────────────────────────────────────────────────────

fn build_10kb_payload() -> String {
    let items: Vec<Value> = (0..200)
        .map(|i| {
            json!({
                "id": i,
                "name": format!("item_{:06}", i),
                "value": i * 3,
                "tags": [format!("tag_a_{}", i), format!("tag_b_{}", i)],
                "active": true,
                "score": i as f64 * 1.5,
                "internal_secret": "redacted",
                "nested": { "depth": 1, "payload": format!("data_{}", i) }
            })
        })
        .collect();
    serde_json::to_string(&json!({ "items": items, "count": 200 }))
        .expect("serialize payload")
}

fn build_pipeline() -> Vec<Op> {
    // Pre-compile JSONPath expression once
    let compiled_path = jsonpath_rust::JsonPath::<Value>::try_from("$.items[*].id")
        .expect("valid jsonpath");
    let compiled: std::sync::Arc<dyn Fn(&Value) -> Value + Send + Sync> =
        std::sync::Arc::new(move |v: &Value| compiled_path.find(v));

    vec![
        // 1. JSONPath extract all item ids
        Op::JsonpathExtract { path: compiled, target: "all_ids".to_string() },
        // 2. Drop an internal field
        Op::DropField("internal_secret".to_string()),
        // 3. Rename count → total
        Op::FieldRename { from: "count".to_string(), to: "total".to_string() },
        // 4. Add 1 to total
        Op::ArithmeticAdd { field: "total".to_string(), operand: 1.0 },
        // 5. Select top-level fields
        Op::SelectFields(vec![
            "items".to_string(),
            "total".to_string(),
            "all_ids".to_string(),
        ]),
        // 6. Drop all_ids
        Op::DropField("all_ids".to_string()),
        // 7. Drop a non-existent field (exercises code path)
        Op::DropField("nonexistent".to_string()),
        // 8. Multiply total by 1
        Op::ArithmeticMultiply { field: "total".to_string(), operand: 1.0 },
        // 9. Rename total → item_count
        Op::FieldRename { from: "total".to_string(), to: "item_count".to_string() },
        // 10. Final select
        Op::SelectFields(vec!["items".to_string(), "item_count".to_string()]),
    ]
}

fn bench_transform_pipeline(c: &mut Criterion) {
    let payload = build_10kb_payload();
    let ops = build_pipeline();

    // Sanity: confirm payload is in the expected size range
    assert!(
        payload.len() > 8_000 && payload.len() < 100_000,
        "payload size unexpected: {} bytes",
        payload.len()
    );

    c.bench_function("transform_pipeline_10ops_10kb", |b| {
        b.iter(|| {
            let result = apply_pipeline(black_box(&ops), black_box(&payload));
            black_box(result)
        });
    });
}

criterion_group!(benches, bench_transform_pipeline);
criterion_main!(benches);
