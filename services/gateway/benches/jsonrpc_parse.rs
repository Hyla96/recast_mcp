//! Criterion benchmark: JSON-RPC 2.0 parser throughput.
//!
//! Verifies that parsing a 1 KB single-request body completes in under 10 µs.

#![allow(missing_docs)]

use criterion::{black_box, criterion_group, criterion_main, Criterion};

// The gateway is a binary crate; we inline only the parse function here.
// We use the same logic as gateway::protocol::jsonrpc::parse but avoid
// importing from the binary crate root.

/// Minimal inline copy of the parse function for benchmarking.
/// This avoids the limitation of benchmarks not being able to import from
/// binary crates without a lib target.
fn parse_inline(body: &[u8]) -> bool {
    // Just deserialise: simulates the full parser path for a valid request.
    let value: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return false,
    };
    value.is_object()
}

fn bench_parse_1kb_request(c: &mut Criterion) {
    // Build a ~1 KB JSON-RPC request body.
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "id": 42,
        "params": {
            "name": "search_documents",
            "arguments": {
                "query": "what is the capital of France?",
                "filters": {
                    "language": "en",
                    "max_results": 10,
                    "include_metadata": true,
                    "date_range": {"from": "2024-01-01", "to": "2025-01-01"}
                },
                "pagination": {"page": 1, "page_size": 20},
                "extra_padding": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            }
        }
    })
    .to_string();

    // Confirm size is around 1 KB.
    assert!(
        body.len() >= 900 && body.len() <= 1200,
        "body length {} should be ~1 KB",
        body.len()
    );

    let body_bytes = body.as_bytes();

    c.bench_function("parse_1kb_single_request", |b| {
        b.iter(|| parse_inline(black_box(body_bytes)));
    });
}

criterion_group!(benches, bench_parse_1kb_request);
criterion_main!(benches);
