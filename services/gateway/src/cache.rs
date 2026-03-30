//! In-memory configuration cache for active MCP servers.
//!
//! Backed by [`moka::sync::Cache`] (max 500,000 entries, 1-hour time-to-idle).
//! A secondary [`DashMap`] provides O(1) slug → server_id reverse lookups.
//!
//! On cache miss, a single PostgreSQL query is performed; the result is
//! inserted before returning so subsequent lookups hit the cache.
//!
//! # Concurrency model
//!
//! All moka operations are internally lock-free at the read path. The slug
//! [`DashMap`] uses per-shard locks, so `slug_to_id` and `upsert` contend only
//! on the shard covering the relevant slug. Eviction listeners run on a moka
//! background thread and must not block.

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use moka::notification::RemovalCause;
use moka::sync::Cache;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use uuid::Uuid;

/// Maximum number of entries held in the config cache.
const CACHE_MAX_CAPACITY: u64 = 500_000;
/// Cache entries idle for this many seconds are evicted.
const CACHE_TTI_SECS: u64 = 3_600;

// ── ServerConfig ─────────────────────────────────────────────────────────────

/// A single `mcp_servers` row as loaded by the gateway.
///
/// This is the gateway's read-only view of a server configuration. It includes
/// all columns needed for routing, MCP `initialize` responses, and tool schema
/// generation. The `config_json` field stores the full tool definitions and
/// upstream URL config as a raw JSON value.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Server primary key.
    pub id: Uuid,
    /// Owner user ID.
    pub user_id: Uuid,
    /// Human-readable name, surfaced in MCP `initialize` → `serverInfo`.
    pub name: String,
    /// URL-safe slug used to route incoming MCP requests.
    pub slug: String,
    /// Optional human-readable description.
    pub description: Option<String>,
    /// Tool definitions and upstream API config (raw JSONB from the DB).
    pub config_json: serde_json::Value,
    /// Server status: one of `active`, `draft`, `inactive`, or `suspended`.
    pub status: String,
    /// Monotonically increasing version counter. Incremented on every UPDATE.
    /// Used by the hot-reload listener to discard out-of-order notifications.
    pub config_version: i64,
    /// Argon2id PHC hash of the server's Bearer token.
    /// `None` if no token has been configured for this server.
    pub token_hash: Option<String>,
    /// First 8 characters of the raw token, safe to include in logs.
    /// `None` if no token has been configured for this server.
    pub token_prefix: Option<String>,
    /// Maximum simultaneous in-flight connections for this server.
    /// Defaults to 50 if not set in the database.
    pub max_connections: u32,
    /// Row creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last-updated timestamp.
    pub updated_at: DateTime<Utc>,
}

// ── CacheStats ───────────────────────────────────────────────────────────────

/// A snapshot of [`ConfigCache`] statistics.
///
/// Returned by [`ConfigCache::stats`] and published as Prometheus metrics.
#[derive(Debug, Clone)]
pub struct CacheStats {
    /// Current number of entries in the cache at the time of the snapshot.
    pub total_entries: u64,
    /// Successful cache lookups since startup.
    pub hit_count: u64,
    /// Cache misses since startup (includes misses that triggered a DB fetch).
    pub miss_count: u64,
    /// Entries evicted since startup (TTI, explicit remove, or capacity).
    pub eviction_count: u64,
}

// ── ConfigCache ──────────────────────────────────────────────────────────────

/// In-memory cache of active MCP server configurations.
///
/// Wrap in `Arc` and share across request handlers:
/// ```ignore
/// let cache = Arc::new(ConfigCache::new(pool));
/// ```
///
/// All read-path methods (`get`, `slug_to_id`) are synchronous and return in
/// under 1 µs on a warmed cache. Write-path methods (`upsert`, `remove`) are
/// also synchronous. Only `get_or_fetch` and `load_all` are async (DB I/O).
pub struct ConfigCache {
    /// Primary moka cache: server_id → Arc<ServerConfig>.
    pub(crate) inner: Cache<Uuid, Arc<ServerConfig>>,
    /// Secondary index: slug → server_id for O(1) reverse lookups.
    slug_index: Arc<DashMap<String, Uuid>>,
    /// Total successful cache lookups (does not count `get_or_fetch` DB hits).
    hit_count: Arc<AtomicU64>,
    /// Total cache misses (both `get` and `get_or_fetch` paths).
    miss_count: Arc<AtomicU64>,
    /// Total evictions (TTI expiry, explicit `remove`, or capacity pressure).
    eviction_count: Arc<AtomicU64>,
    /// Database pool used for cache-miss queries and `load_all`.
    db_pool: PgPool,
    /// Set to `true` after [`load_all`] completes successfully.
    ///
    /// Used by the `/healthz/ready` probe to indicate the cache is warm.
    cache_loaded: Arc<std::sync::atomic::AtomicBool>,
}

impl ConfigCache {
    /// Construct a new, empty cache backed by `db_pool`.
    ///
    /// Does not perform any I/O. Call [`load_all`] to pre-warm the cache at
    /// startup.
    pub fn new(db_pool: PgPool) -> Self {
        let slug_index: Arc<DashMap<String, Uuid>> = Arc::new(DashMap::new());
        let eviction_count = Arc::new(AtomicU64::new(0));

        let slug_index_ev = Arc::clone(&slug_index);
        let eviction_count_ev = Arc::clone(&eviction_count);

        let inner = Cache::builder()
            .max_capacity(CACHE_MAX_CAPACITY)
            .time_to_idle(Duration::from_secs(CACHE_TTI_SECS))
            .eviction_listener(move |_key: Arc<Uuid>, value: Arc<ServerConfig>, cause| {
                eviction_count_ev.fetch_add(1, Ordering::Relaxed);
                metrics::counter!("gateway_cache_evictions_total").increment(1);
                // Remove the slug index entry only for true evictions — not
                // replacements. For replacements, `upsert()` has already
                // written the new slug mapping before the old entry is evicted.
                if cause != RemovalCause::Replaced {
                    let server_id = value.id;
                    // Guard: only remove if the slug still points to this server.
                    // This handles the case where two servers swap slugs.
                    let still_owned = slug_index_ev
                        .get(&value.slug)
                        .map(|v| *v == server_id)
                        .unwrap_or(false);
                    if still_owned {
                        slug_index_ev.remove(&value.slug);
                    }
                }
            })
            .build();

        Self {
            inner,
            slug_index,
            hit_count: Arc::new(AtomicU64::new(0)),
            miss_count: Arc::new(AtomicU64::new(0)),
            eviction_count,
            db_pool,
            cache_loaded: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Returns `true` if [`load_all`] has completed successfully at least once.
    ///
    /// Used by the `/healthz/ready` endpoint to confirm the cache is warm.
    pub fn is_loaded(&self) -> bool {
        self.cache_loaded.load(Ordering::Acquire)
    }

    /// Current number of entries in the cache.
    ///
    /// May briefly lag insertions/removals — moka processes pending tasks
    /// asynchronously. For tests, call `run_pending_tasks()` first if exact
    /// counts are required.
    pub fn entry_count(&self) -> u64 {
        self.inner.entry_count()
    }

    /// Test-only helper: mark the cache as loaded without performing a DB query.
    ///
    /// Used in unit tests that need the readiness probe to see the cache as warm.
    #[cfg(test)]
    pub fn mark_loaded_for_testing(&self) {
        self.cache_loaded
            .store(true, std::sync::atomic::Ordering::Release);
    }

    /// Look up a server by ID in the in-memory cache only.
    ///
    /// Returns in under 1 µs on a warmed cache. Does **not** touch the
    /// database on a miss. Use [`get_or_fetch`] when a DB fallback is needed.
    pub fn get(&self, server_id: Uuid) -> Option<Arc<ServerConfig>> {
        let result = self.inner.get(&server_id);
        if result.is_some() {
            self.hit_count.fetch_add(1, Ordering::Relaxed);
            metrics::counter!("gateway_cache_hits_total").increment(1);
        } else {
            self.miss_count.fetch_add(1, Ordering::Relaxed);
            metrics::counter!("gateway_cache_misses_total").increment(1);
        }
        result
    }

    /// Look up a server by ID, querying PostgreSQL on cache miss.
    ///
    /// If found in the DB with `status = 'active'`, the row is inserted into
    /// the cache before returning. Returns `Ok(None)` if the server does not
    /// exist or is not active.
    pub async fn get_or_fetch(
        &self,
        server_id: Uuid,
    ) -> Result<Option<Arc<ServerConfig>>, sqlx::Error> {
        if let Some(cfg) = self.inner.get(&server_id) {
            self.hit_count.fetch_add(1, Ordering::Relaxed);
            metrics::counter!("gateway_cache_hits_total").increment(1);
            return Ok(Some(cfg));
        }

        self.miss_count.fetch_add(1, Ordering::Relaxed);
        metrics::counter!("gateway_cache_misses_total").increment(1);

        let maybe_row = sqlx::query(
            "SELECT id, user_id, name, slug, description, config_json, \
             status, config_version, token_hash, token_prefix, max_connections, \
             created_at, updated_at \
             FROM mcp_servers \
             WHERE id = $1 AND status = 'active'",
        )
        .bind(server_id)
        .fetch_optional(&self.db_pool)
        .await?;

        let Some(row) = maybe_row else {
            return Ok(None);
        };

        let config = Arc::new(row_to_server_config(&row)?);
        self.upsert(Arc::clone(&config));
        Ok(Some(config))
    }

    /// Insert or atomically replace a server config in the cache.
    ///
    /// The slug index is updated before the moka entry is replaced, so that if
    /// the eviction listener fires for the old entry it sees the updated slug
    /// mapping and skips removal.
    ///
    /// Concurrent readers will observe either the old or the new config, never
    /// a partial / torn state.
    pub fn upsert(&self, config: Arc<ServerConfig>) {
        // Update slug index before inserting into moka so the eviction listener
        // (which fires for the replaced old entry) sees the correct mapping.
        self.slug_index.insert(config.slug.clone(), config.id);
        self.inner.insert(config.id, config);
        metrics::gauge!("gateway_cache_entries").set(self.inner.entry_count() as f64);
    }

    /// Remove a server config from the cache and the slug index.
    ///
    /// Subsequent calls to [`get`] or [`get_or_fetch`] will return `None`.
    /// The slug index entry is removed synchronously; the moka entry is
    /// invalidated (processed on next moka maintenance cycle).
    pub fn remove(&self, server_id: Uuid) {
        // Read the current entry to find its slug before invalidating.
        if let Some(cfg) = self.inner.get(&server_id) {
            // Remove the slug index entry only if it still maps to this server.
            self.slug_index
                .remove_if(&cfg.slug, |_, v| *v == server_id);
        }
        self.inner.invalidate(&server_id);
        metrics::gauge!("gateway_cache_entries").set(self.inner.entry_count() as f64);
    }

    /// Translate a slug to its server ID using the O(1) secondary index.
    ///
    /// Returns `None` if no active server is registered under that slug.
    pub fn slug_to_id(&self, slug: &str) -> Option<Uuid> {
        self.slug_index.get(slug).map(|v| *v)
    }

    /// Return a statistics snapshot and refresh Prometheus gauge values.
    pub fn stats(&self) -> CacheStats {
        let total_entries = self.inner.entry_count();
        let hit_count = self.hit_count.load(Ordering::Relaxed);
        let miss_count = self.miss_count.load(Ordering::Relaxed);
        let eviction_count = self.eviction_count.load(Ordering::Relaxed);

        metrics::gauge!("gateway_cache_entries").set(total_entries as f64);
        metrics::gauge!("gateway_cache_hit_count").set(hit_count as f64);
        metrics::gauge!("gateway_cache_miss_count").set(miss_count as f64);
        metrics::gauge!("gateway_cache_eviction_count").set(eviction_count as f64);

        CacheStats {
            total_entries,
            hit_count,
            miss_count,
            eviction_count,
        }
    }

    /// Load all active servers from PostgreSQL into the cache.
    ///
    /// Called once at startup. Returns the number of entries loaded.
    /// Completes within 5 seconds for up to 100,000 rows over a healthy DB
    /// connection.
    pub async fn load_all(&self) -> Result<usize, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT id, user_id, name, slug, description, config_json, \
             status, config_version, token_hash, token_prefix, max_connections, \
             created_at, updated_at \
             FROM mcp_servers \
             WHERE status = 'active'",
        )
        .fetch_all(&self.db_pool)
        .await?;

        let count = rows.len();
        for row in &rows {
            let config = row_to_server_config(row)?;
            self.upsert(Arc::new(config));
        }

        tracing::info!(count, "gateway config cache loaded from database");
        metrics::gauge!("gateway_cache_entries").set(self.inner.entry_count() as f64);
        self.cache_loaded
            .store(true, std::sync::atomic::Ordering::Release);
        Ok(count)
    }
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Decode a [`ServerConfig`] from a Postgres row returned by the cache queries.
///
/// The SELECT must include: id, user_id, name, slug, description, config_json,
/// status, config_version, token_hash, token_prefix, created_at, updated_at.
pub(crate) fn row_to_server_config(row: &sqlx::postgres::PgRow) -> Result<ServerConfig, sqlx::Error> {
    use sqlx::Row;
    // max_connections is stored as PostgreSQL INTEGER (i32); cast to u32.
    let max_connections: i32 = row.try_get("max_connections").unwrap_or(50);
    Ok(ServerConfig {
        id: row.try_get("id")?,
        user_id: row.try_get("user_id")?,
        name: row.try_get("name")?,
        slug: row.try_get("slug")?,
        description: row.try_get("description")?,
        config_json: row.try_get("config_json")?,
        status: row.try_get("status")?,
        config_version: row.try_get("config_version")?,
        token_hash: row.try_get("token_hash")?,
        token_prefix: row.try_get("token_prefix")?,
        max_connections: max_connections.max(0) as u32,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
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

    /// Build a minimal [`ServerConfig`] for testing.
    fn make_config(id: Uuid, slug: &str) -> Arc<ServerConfig> {
        Arc::new(ServerConfig {
            id,
            user_id: Uuid::new_v4(),
            name: format!("Server {slug}"),
            slug: slug.to_string(),
            description: None,
            config_json: serde_json::json!({}),
            status: "active".to_string(),
            config_version: 1,
            token_hash: None,
            token_prefix: None,
            max_connections: 50,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        })
    }

    /// Build a [`ConfigCache`] with a lazy (non-connecting) DB pool for tests
    /// that do not exercise the DB fetch path.
    fn make_cache() -> ConfigCache {
        // connect_lazy does not open a DB connection until first use,
        // so this is safe in unit tests that never call get_or_fetch/load_all.
        let pool = sqlx::PgPool::connect_lazy("postgres://localhost/test")
            .expect("lazy pool construction must not fail");
        ConfigCache::new(pool)
    }

    // ── populate and hit 1,000 entries ────────────────────────────────────────

    #[tokio::test]
    async fn populate_and_hit_1000_entries() {
        let cache = make_cache();
        let ids: Vec<Uuid> = (0..1000).map(|_| Uuid::new_v4()).collect();

        for (i, &id) in ids.iter().enumerate() {
            cache.upsert(make_config(id, &format!("slug-{i}")));
        }

        // Run pending moka maintenance so entry_count is accurate.
        cache.inner.run_pending_tasks();
        assert_eq!(cache.inner.entry_count(), 1000, "cache must hold all 1,000 entries");

        for &id in &ids {
            assert!(cache.get(id).is_some(), "every inserted entry must be retrievable");
        }

        let stats = cache.stats();
        assert_eq!(stats.hit_count, 1000, "hit_count must equal number of reads");
    }

    // ── upsert replaces old value ─────────────────────────────────────────────

    #[tokio::test]
    async fn upsert_replaces_old_value() {
        let cache = make_cache();
        let ids: Vec<Uuid> = (0..10).map(|_| Uuid::new_v4()).collect();

        for (i, &id) in ids.iter().enumerate() {
            cache.upsert(make_config(id, &format!("slug-{i}")));
        }

        // Replace with updated configs (same IDs and slugs, different names).
        for (i, &id) in ids.iter().enumerate() {
            let updated = Arc::new(ServerConfig {
                id,
                user_id: Uuid::new_v4(),
                name: format!("Updated-{i}"),
                slug: format!("slug-{i}"),
                description: None,
                config_json: serde_json::json!({"version": 2}),
                status: "active".to_string(),
                config_version: 2,
                token_hash: None,
                token_prefix: None,
                max_connections: 50,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            });
            cache.upsert(updated);
        }

        cache.inner.run_pending_tasks();

        for (i, &id) in ids.iter().enumerate() {
            let cfg = cache.get(id).expect("entry must exist after upsert");
            assert!(
                cfg.name.starts_with("Updated-"),
                "get() must return updated value at index {i}; got name={}",
                cfg.name
            );
        }
    }

    // ── remove evicts entries and increments miss_count ───────────────────────

    #[tokio::test]
    async fn remove_evicts_entries() {
        let cache = make_cache();
        let ids: Vec<Uuid> = (0..5).map(|_| Uuid::new_v4()).collect();

        for (i, &id) in ids.iter().enumerate() {
            cache.upsert(make_config(id, &format!("remove-slug-{i}")));
        }

        for &id in &ids {
            cache.remove(id);
        }

        // Let moka process pending invalidations before reading.
        cache.inner.run_pending_tasks();

        for (i, &id) in ids.iter().enumerate() {
            assert!(
                cache.get(id).is_none(),
                "entry {i} must be absent after remove()"
            );
        }

        let stats = cache.stats();
        assert_eq!(
            stats.miss_count,
            ids.len() as u64,
            "each removed entry must count as a miss"
        );
    }

    // ── slug_to_id reverse lookup ─────────────────────────────────────────────

    #[tokio::test]
    async fn slug_to_id_returns_correct_id() {
        let cache = make_cache();
        let id = Uuid::new_v4();
        cache.upsert(make_config(id, "my-server"));

        assert_eq!(
            cache.slug_to_id("my-server"),
            Some(id),
            "slug must resolve to the inserted server_id"
        );
        assert_eq!(
            cache.slug_to_id("nonexistent"),
            None,
            "unknown slug must return None"
        );
    }

    // ── slug index cleared on remove ──────────────────────────────────────────

    #[tokio::test]
    async fn slug_index_cleared_on_remove() {
        let cache = make_cache();
        let id = Uuid::new_v4();
        cache.upsert(make_config(id, "removable-slug"));
        assert_eq!(cache.slug_to_id("removable-slug"), Some(id));

        cache.remove(id);
        // Slug index removal is synchronous inside remove().
        assert_eq!(
            cache.slug_to_id("removable-slug"),
            None,
            "slug must be gone from the index after remove()"
        );
    }

    // ── stats snapshot reflects operations ───────────────────────────────────

    #[tokio::test]
    async fn stats_reflect_operations() {
        let cache = make_cache();
        let id = Uuid::new_v4();

        // One miss (id not in cache).
        let _ = cache.get(id);
        // One hit (after insert).
        cache.upsert(make_config(id, "stats-slug"));
        let _ = cache.get(id);

        // Flush moka pending tasks so entry_count is accurate.
        cache.inner.run_pending_tasks();

        let stats = cache.stats();
        assert_eq!(stats.miss_count, 1);
        assert_eq!(stats.hit_count, 1);
        assert!(stats.total_entries >= 1);
    }
}
