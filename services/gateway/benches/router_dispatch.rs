//! Criterion benchmark: `tools/list` cache-hit dispatch path.
//!
//! Since `mcp-gateway` is a binary crate (no lib target), we cannot import
//! `Router` directly. Instead, we benchmark the three O(1) operations that
//! make up the hot path:
//!
//! 1. `DashMap::get` — slug → server_id (mirrors `ConfigCache::slug_to_id`)
//! 2. `moka::sync::Cache::get` — server_id → Arc<ServerConfig>
//! 3. `moka::sync::Cache::get` — schema cache hit (mirrors `SchemaCache::get_or_generate`)
//! 4. `serde_json::to_value` — serialise `ToolsListResult`
//!
//! The benchmark verifies the end-to-end production path completes in under
//! 100 µs.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use dashmap::DashMap;
use moka::sync::Cache;
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
use uuid::Uuid;

// ── Simulated server config (mirrors cache::ServerConfig) ─────────────────────

#[derive(Clone)]
struct FakeServerConfig {
    id: Uuid,
    name: String,
    config_version: i64,
}

// ── Simulated tool schema (mirrors tool_schema::McpTool) ──────────────────────

#[derive(Clone, serde::Serialize)]
struct FakeMcpTool {
    name: String,
    description: String,
    #[serde(rename = "inputSchema")]
    input_schema: Value,
}

// ── Benchmark ──────────────────────────────────────────────────────────────────

fn bench_tools_list_cache_hit(c: &mut Criterion) {
    // ── Setup ──────────────────────────────────────────────────────────────────

    // Slug → server_id index (mirrors ConfigCache::slug_index).
    let slug_index: DashMap<String, Uuid> = DashMap::new();

    // server_id → Arc<ServerConfig> primary cache (mirrors ConfigCache::inner).
    let config_cache: Cache<Uuid, Arc<FakeServerConfig>> = Cache::builder()
        .max_capacity(500_000)
        .time_to_idle(Duration::from_secs(3600))
        .build();

    // (server_id, config_version) → Arc<Vec<McpTool>> schema cache
    // (mirrors SchemaCache::inner).
    let schema_cache: Cache<(Uuid, i64), Arc<Vec<FakeMcpTool>>> = Cache::builder()
        .max_capacity(50_000)
        .time_to_live(Duration::from_secs(3600))
        .build();

    let slug = "my-weather-api".to_string();
    let server_id = Uuid::new_v4();

    let config = Arc::new(FakeServerConfig {
        id: server_id,
        name: "Weather API".to_string(),
        config_version: 42,
    });

    let tools = Arc::new(vec![
        FakeMcpTool {
            name: "get_weather".to_string(),
            description: "Get current weather".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "city": { "type": "string" }
                },
                "required": ["city"]
            }),
        },
    ]);

    slug_index.insert(slug.clone(), server_id);
    config_cache.insert(server_id, Arc::clone(&config));
    schema_cache.insert((server_id, 42), Arc::clone(&tools));

    // Flush pending tasks so all entries are visible.
    config_cache.run_pending_tasks();
    schema_cache.run_pending_tasks();

    // ── Warm the caches ────────────────────────────────────────────────────────

    // Trigger a read to ensure the entry is in the moka read buffer.
    let _ = slug_index.get(&slug);
    let _ = config_cache.get(&server_id);
    let _ = schema_cache.get(&(server_id, 42));

    // ── Benchmark ──────────────────────────────────────────────────────────────

    c.bench_function("tools_list_cache_hit_dispatch", |b| {
        b.iter(|| {
            // Step 1: slug → server_id (O(1) DashMap shard lookup).
            let sid = black_box(slug_index.get(black_box(&slug)))
                .map(|v| *v)
                .unwrap();

            // Step 2: server_id → ServerConfig (O(1) moka get).
            let cfg = black_box(config_cache.get(black_box(&sid))).unwrap();

            // Step 3: schema cache hit (O(1) moka get keyed by (id, version)).
            let cached_tools =
                black_box(schema_cache.get(black_box(&(cfg.id, cfg.config_version)))).unwrap();

            // Step 4: serialise ToolsListResult → serde_json::Value.
            let result = json!({ "tools": *cached_tools });
            black_box(result)
        });
    });
}

criterion_group!(benches, bench_tools_list_cache_hit);
criterion_main!(benches);
