//! Criterion benchmark: ConfigCache::get() hot path.
//!
//! Since `mcp-gateway` is a binary crate (no lib target), we cannot import
//! `ConfigCache` directly. Instead, we benchmark `moka::sync::Cache::get()`
//! with 100,000 `Uuid` entries — the exact underlying call that
//! `ConfigCache::get()` wraps — to verify the < 1 µs acceptance criterion.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use moka::sync::Cache;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

fn bench_cache_get_100k(c: &mut Criterion) {
    const N: usize = 100_000;

    // Build a warmed cache with N entries.
    let cache: Cache<Uuid, Arc<u64>> = Cache::builder()
        .max_capacity(N as u64 + 1)
        .time_to_idle(Duration::from_secs(3600))
        .build();

    let ids: Vec<Uuid> = (0..N).map(|_| Uuid::new_v4()).collect();
    for (i, &id) in ids.iter().enumerate() {
        cache.insert(id, Arc::new(i as u64));
    }
    // Flush pending writes so entry_count is accurate and all entries are
    // visible to subsequent reads.
    cache.run_pending_tasks();

    let probe_id = ids[N / 2];

    let mut group = c.benchmark_group("cache_get");
    group.bench_function("warmed_100k_entries", |b| {
        b.iter(|| {
            let _ = black_box(cache.get(black_box(&probe_id)));
        });
    });
    group.finish();
}

criterion_group!(benches, bench_cache_get_100k);
criterion_main!(benches);
