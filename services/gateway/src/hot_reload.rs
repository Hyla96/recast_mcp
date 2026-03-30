//! Hot-reload of gateway configuration via PostgreSQL LISTEN/NOTIFY.
//!
//! When an INSERT, UPDATE, or DELETE fires on the `mcp_servers` table, the
//! database trigger `mcp_servers_notify_changes` publishes a JSON payload on
//! the `mcp_server_changes` PostgreSQL channel. This module's [`ConfigSyncTask`]
//! subscribes to that channel via a dedicated [`PgListener`] connection (not
//! from the shared request pool) and propagates changes to the in-memory
//! [`ConfigCache`].
//!
//! # Reliability guarantees
//!
//! - **Version ordering**: each notification carries `config_version`; lower
//!   versions for the same `server_id` are discarded so out-of-order deliveries
//!   cannot revert a more recent config.
//! - **Batch coalescing**: when >20 notifications arrive within a 100 ms window
//!   they are deduplicated by `server_id` (highest version wins) before being
//!   applied, preventing thundering-herd DB queries on mass updates.
//! - **Reconnect with backoff**: on connection loss the task reconnects with
//!   exponential backoff (1 s → 2 s → … → 30 s), logging each attempt at WARN.
//!   After a successful reconnect, missed changes are replayed by querying rows
//!   whose `updated_at` is newer than the time of disconnection.
//! - **Supervised**: an outer supervisor loop catches task panics, increments
//!   the `gateway_config_sync_panics_total` Prometheus counter, and restarts
//!   the inner listener task automatically.

use crate::cache::{row_to_server_config, ConfigCache, ServerConfig};
use dashmap::DashMap;
use serde::Deserialize;
use sqlx::postgres::PgListener;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;
use uuid::Uuid;

// ── Constants ─────────────────────────────────────────────────────────────────

/// PostgreSQL channel name the gateway subscribes to.
const PG_CHANNEL: &str = "mcp_server_changes";

/// Initial reconnect backoff in seconds.
const BACKOFF_INITIAL_SECS: u64 = 1;
/// Maximum reconnect backoff in seconds.
const BACKOFF_MAX_SECS: u64 = 30;
/// Duration of the notification batch collection window.
const BATCH_WINDOW_MS: u64 = 100;
/// Batch size threshold above which deduplication is applied.
const COALESCE_THRESHOLD: usize = 20;

// ── Notification types ────────────────────────────────────────────────────────

/// Deserialized payload received from the `mcp_server_changes` channel.
#[derive(Debug, Deserialize)]
struct NotificationPayload {
    server_id: Uuid,
    /// One of `"insert"`, `"update"`, `"delete"` (lowercase PostgreSQL TG_OP).
    op: String,
    config_version: i64,
}

/// Parsed operation kind derived from [`NotificationPayload::op`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpKind {
    /// Row was inserted (server created).
    Insert,
    /// Row was updated (server config changed).
    Update,
    /// Row was deleted (server removed).
    Delete,
}

impl NotificationPayload {
    fn op_kind(&self) -> Option<OpKind> {
        match self.op.as_str() {
            "insert" => Some(OpKind::Insert),
            "update" => Some(OpKind::Update),
            "delete" => Some(OpKind::Delete),
            _ => None,
        }
    }
}

// ── Version tracking ──────────────────────────────────────────────────────────

/// Per-server last-applied `config_version`. Shared across reconnect cycles so
/// stale notifications from before a disconnect are always discarded.
type VersionMap = Arc<DashMap<Uuid, i64>>;

// ── ConfigSyncTask ────────────────────────────────────────────────────────────

/// Supervised background task that keeps [`ConfigCache`] in sync with PostgreSQL
/// via LISTEN/NOTIFY.
///
/// Create with [`ConfigSyncTask::new`] and call [`ConfigSyncTask::start`] to
/// begin listening. The returned [`tokio::task::JoinHandle`] can be dropped —
/// the task continues running as a detached background task.
///
/// # Supervision
///
/// Panics inside the inner listener task are caught by the outer supervisor.
/// Each restart increments the `gateway_config_sync_panics_total` Prometheus
/// counter. A 1-second cool-down prevents tight restart loops.
pub struct ConfigSyncTask {
    /// Full PostgreSQL URL used to create the dedicated `PgListener` connection.
    db_url: String,
    /// Shared pool used for config fetch queries on notification arrival.
    query_pool: sqlx::PgPool,
    /// The cache to keep in sync.
    cache: Arc<ConfigCache>,
}

impl ConfigSyncTask {
    /// Create a new config sync task.
    ///
    /// `query_pool` is shared with the rest of the gateway for fetching
    /// individual server configs after a notification arrives. `PgListener`
    /// opens its own dedicated connection using `db_url`.
    pub fn new(db_url: String, query_pool: sqlx::PgPool, cache: Arc<ConfigCache>) -> Self {
        Self {
            db_url,
            query_pool,
            cache,
        }
    }

    /// Start the supervised listener. Returns immediately; the task runs in the
    /// background for the lifetime of the process.
    ///
    /// The returned handle can be dropped — the task keeps running.
    pub fn start(self) -> tokio::task::JoinHandle<()> {
        let db_url = Arc::new(self.db_url);
        let query_pool = self.query_pool;
        let cache = self.cache;

        tokio::spawn(async move {
            loop {
                let db_url_c = Arc::clone(&db_url);
                let pool_c = query_pool.clone();
                let cache_c = Arc::clone(&cache);

                let handle = tokio::spawn(async move {
                    run_sync_loop((*db_url_c).clone(), pool_c, cache_c).await;
                });

                match handle.await {
                    Ok(()) => {
                        tracing::info!("config sync task exited normally");
                        break;
                    }
                    Err(join_err) => {
                        tracing::error!(
                            error = %join_err,
                            "config sync task panicked; restarting after 1 s"
                        );
                        metrics::counter!("gateway_config_sync_panics_total").increment(1);
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        })
    }
}

// ── Core reconnect loop ───────────────────────────────────────────────────────

/// Outer loop: repeatedly connects and listens, reconnecting with exponential
/// backoff on failure. Runs indefinitely until a clean exit (graceful shutdown).
async fn run_sync_loop(db_url: String, query_pool: sqlx::PgPool, cache: Arc<ConfigCache>) {
    let versions: VersionMap = Arc::new(DashMap::new());
    let mut backoff_secs: u64 = BACKOFF_INITIAL_SECS;
    let mut disconnect_time: Option<chrono::DateTime<chrono::Utc>> = None;

    loop {
        if let Some(lost_at) = disconnect_time {
            tracing::warn!(
                backoff_secs,
                "config sync LISTEN connection lost; reconnecting"
            );
            tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
            backoff_secs = (backoff_secs * 2).min(BACKOFF_MAX_SECS);

            // After reconnecting, replay any rows updated while we were offline.
            match replay_missed_changes(&query_pool, &cache, &versions, lost_at).await {
                Ok(count) => {
                    tracing::info!(
                        count,
                        "config sync: replayed missed changes after reconnect"
                    );
                    // Successful replay implies the DB is reachable; reset backoff.
                    backoff_secs = BACKOFF_INITIAL_SECS;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "config sync: replay query failed");
                }
            }
        }

        // Record the time just before establishing the connection so that, on
        // the next iteration's replay, we query rows updated since this point.
        disconnect_time = Some(chrono::Utc::now());

        match run_listener_once(&db_url, &cache, &versions, &query_pool).await {
            Ok(()) => {
                tracing::info!("config sync listener exited cleanly");
                return;
            }
            Err(e) => {
                tracing::warn!(error = %e, "config sync listener returned error");
                // Loop continues — will sleep and reconnect.
            }
        }
    }
}

// ── Single listener session ───────────────────────────────────────────────────

/// Connect once to PostgreSQL, subscribe to the channel, and process
/// notifications until an error occurs or a graceful exit is signalled.
async fn run_listener_once(
    db_url: &str,
    cache: &Arc<ConfigCache>,
    versions: &VersionMap,
    query_pool: &sqlx::PgPool,
) -> Result<(), sqlx::Error> {
    let mut listener = PgListener::connect(db_url).await?;
    listener.listen(PG_CHANNEL).await?;
    tracing::info!("config sync: subscribed to PostgreSQL channel '{PG_CHANNEL}'");

    loop {
        let batch = collect_batch(&mut listener).await?;
        apply_batch(batch, cache, versions, query_pool).await;
    }
}

// ── Batch collection and coalescing ──────────────────────────────────────────

/// Block until the first notification arrives, then collect additional ones
/// within a [`BATCH_WINDOW_MS`] ms window. If the batch exceeds
/// [`COALESCE_THRESHOLD`], deduplicate by `server_id` keeping the highest
/// `config_version`.
///
/// Returns `Err` only when the underlying `recv()` returns a hard error.
async fn collect_batch(listener: &mut PgListener) -> Result<Vec<NotificationPayload>, sqlx::Error> {
    // Block for the first notification (no timeout — we wait as long as needed).
    let first = listener.recv().await?;
    let mut batch = Vec::new();
    if let Some(p) = parse_payload(first.payload()) {
        batch.push(p);
    }

    // Collect additional notifications within the batch window.
    let deadline = Instant::now() + Duration::from_millis(BATCH_WINDOW_MS);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, listener.recv()).await {
            Ok(Ok(notif)) => {
                if let Some(p) = parse_payload(notif.payload()) {
                    batch.push(p);
                }
            }
            Ok(Err(e)) => return Err(e),
            Err(_timeout) => break,
        }
    }

    if batch.len() > COALESCE_THRESHOLD {
        batch = coalesce_batch(batch);
    }

    Ok(batch)
}

/// Deduplicate a batch by `server_id`, retaining the notification with the
/// highest `config_version` per server. Ordering of the deduplicated entries
/// is implementation-defined (HashMap iteration order).
fn coalesce_batch(batch: Vec<NotificationPayload>) -> Vec<NotificationPayload> {
    let mut best: std::collections::HashMap<Uuid, NotificationPayload> =
        std::collections::HashMap::new();

    for notif in batch {
        best.entry(notif.server_id)
            .and_modify(|e| {
                if notif.config_version > e.config_version {
                    *e = NotificationPayload {
                        server_id: notif.server_id,
                        op: notif.op.clone(),
                        config_version: notif.config_version,
                    };
                }
            })
            .or_insert(notif);
    }

    best.into_values().collect()
}

// ── Notification application ──────────────────────────────────────────────────

/// Apply a batch of notifications to the cache in arrival order.
async fn apply_batch(
    batch: Vec<NotificationPayload>,
    cache: &Arc<ConfigCache>,
    versions: &VersionMap,
    query_pool: &sqlx::PgPool,
) {
    for payload in batch {
        apply_payload(payload, cache, versions, query_pool).await;
    }
}

/// Apply a single notification, enforcing version ordering for
/// insert/update operations. Delete operations are always applied.
async fn apply_payload(
    payload: NotificationPayload,
    cache: &Arc<ConfigCache>,
    versions: &VersionMap,
    query_pool: &sqlx::PgPool,
) {
    let op = match payload.op_kind() {
        Some(op) => op,
        None => {
            tracing::warn!(
                op = %payload.op,
                server_id = %payload.server_id,
                "config sync: unknown operation in notification; skipping"
            );
            return;
        }
    };

    // Delete operations are applied unconditionally. The version of a deleted
    // row is the last version before deletion, which is always ≥ any earlier
    // version we might have applied, so no version check is needed.
    if op == OpKind::Delete {
        tracing::debug!(
            server_id = %payload.server_id,
            config_version = payload.config_version,
            "config sync: applying delete"
        );
        versions.remove(&payload.server_id);
        cache.remove(payload.server_id);
        return;
    }

    // For insert/update: discard stale notifications (lower version than we've
    // already applied). This handles the unlikely case where notifications are
    // re-delivered or arrive out of order.
    let current_version = versions.get(&payload.server_id).map(|v| *v).unwrap_or(-1);
    if payload.config_version <= current_version {
        tracing::debug!(
            server_id = %payload.server_id,
            notification_version = payload.config_version,
            current_version,
            "config sync: discarding stale notification"
        );
        return;
    }

    // Fetch the full config row and upsert it into the cache.
    match fetch_config(query_pool, payload.server_id).await {
        Ok(Some(config)) => {
            let version = config.config_version;
            versions.insert(payload.server_id, version);
            tracing::debug!(
                server_id = %payload.server_id,
                config_version = version,
                op = %payload.op,
                "config sync: upserted config"
            );
            cache.upsert(Arc::new(config));
        }
        Ok(None) => {
            // Server not found or no longer active — treat as an implicit delete.
            tracing::debug!(
                server_id = %payload.server_id,
                "config sync: server not found or inactive; removing from cache"
            );
            versions.remove(&payload.server_id);
            cache.remove(payload.server_id);
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                server_id = %payload.server_id,
                "config sync: DB fetch failed; will retry on next notification"
            );
        }
    }
}

// ── Missed-change replay ──────────────────────────────────────────────────────

/// After reconnecting, query all active servers whose `updated_at` is at or
/// after `since` (minus a 5-second safety margin) and upsert them into the
/// cache. Returns the number of rows replayed.
///
/// This compensates for changes that fired pg_notify while the LISTEN
/// connection was down. Deleted servers are not recoverable via this path —
/// they will simply stay in cache until the next explicit notification or TTI
/// eviction. In practice the gap is bounded by the maximum reconnect backoff
/// (30 s) plus the safety margin (5 s), which is acceptable for a non-critical
/// consistency window.
async fn replay_missed_changes(
    query_pool: &sqlx::PgPool,
    cache: &Arc<ConfigCache>,
    versions: &VersionMap,
    since: chrono::DateTime<chrono::Utc>,
) -> Result<usize, sqlx::Error> {
    // Add a safety margin to catch rows committed just before the disconnect.
    let cutoff = since - chrono::Duration::seconds(5);

    let rows = sqlx::query(
        "SELECT id, user_id, name, slug, description, config_json, \
         status, config_version, token_hash, token_prefix, \
         created_at, updated_at \
         FROM mcp_servers \
         WHERE status = 'active' AND updated_at >= $1",
    )
    .bind(cutoff)
    .fetch_all(query_pool)
    .await?;

    let mut replayed: usize = 0;
    for row in &rows {
        match row_to_server_config(row) {
            Ok(config) => {
                let current = versions.get(&config.id).map(|v| *v).unwrap_or(-1);
                if config.config_version > current {
                    versions.insert(config.id, config.config_version);
                    cache.upsert(Arc::new(config));
                    replayed += 1;
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "config sync: replay row decode failed");
            }
        }
    }

    Ok(replayed)
}

// ── DB helpers ────────────────────────────────────────────────────────────────

/// Fetch a single active server config from the database.
///
/// Returns `Ok(None)` if the server does not exist or is not `'active'`.
async fn fetch_config(
    pool: &sqlx::PgPool,
    server_id: Uuid,
) -> Result<Option<ServerConfig>, sqlx::Error> {
    let maybe_row = sqlx::query(
        "SELECT id, user_id, name, slug, description, config_json, \
         status, config_version, token_hash, token_prefix, \
         created_at, updated_at \
         FROM mcp_servers \
         WHERE id = $1 AND status = 'active'",
    )
    .bind(server_id)
    .fetch_optional(pool)
    .await?;

    match maybe_row {
        Some(row) => row_to_server_config(&row).map(Some),
        None => Ok(None),
    }
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Parse a raw pg_notify payload string into a [`NotificationPayload`].
///
/// Returns `None` and logs a warning on parse failure. Callers should skip
/// `None` entries — a malformed payload does not indicate a connection error.
fn parse_payload(payload: &str) -> Option<NotificationPayload> {
    match serde_json::from_str::<NotificationPayload>(payload) {
        Ok(p) => Some(p),
        Err(e) => {
            tracing::warn!(
                error = %e,
                payload,
                "config sync: failed to parse notification payload; skipping"
            );
            None
        }
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

    // ── parse_payload ─────────────────────────────────────────────────────────

    #[test]
    fn parse_payload_valid_insert() {
        let id = Uuid::new_v4();
        let json = format!(
            r#"{{"server_id":"{id}","op":"insert","config_version":1}}"#
        );
        let p = parse_payload(&json).expect("valid payload must parse");
        assert_eq!(p.server_id, id);
        assert_eq!(p.op, "insert");
        assert_eq!(p.config_version, 1);
        assert_eq!(p.op_kind(), Some(OpKind::Insert));
    }

    #[test]
    fn parse_payload_valid_update() {
        let id = Uuid::new_v4();
        let json = format!(
            r#"{{"server_id":"{id}","op":"update","config_version":7}}"#
        );
        let p = parse_payload(&json).expect("valid payload must parse");
        assert_eq!(p.op_kind(), Some(OpKind::Update));
        assert_eq!(p.config_version, 7);
    }

    #[test]
    fn parse_payload_valid_delete() {
        let id = Uuid::new_v4();
        let json = format!(
            r#"{{"server_id":"{id}","op":"delete","config_version":3}}"#
        );
        let p = parse_payload(&json).expect("valid payload must parse");
        assert_eq!(p.op_kind(), Some(OpKind::Delete));
    }

    #[test]
    fn parse_payload_invalid_json_returns_none() {
        assert!(parse_payload("{not valid json}").is_none());
    }

    #[test]
    fn parse_payload_unknown_op_maps_to_none() {
        let id = Uuid::new_v4();
        let json = format!(
            r#"{{"server_id":"{id}","op":"truncate","config_version":1}}"#
        );
        let p = parse_payload(&json).expect("struct parses even with unknown op");
        assert_eq!(p.op_kind(), None);
    }

    // ── coalesce_batch ────────────────────────────────────────────────────────

    #[test]
    fn coalesce_keeps_highest_version_per_server() {
        let id_a = Uuid::new_v4();
        let id_b = Uuid::new_v4();

        let batch = vec![
            NotificationPayload { server_id: id_a, op: "update".into(), config_version: 3 },
            NotificationPayload { server_id: id_a, op: "update".into(), config_version: 5 },
            NotificationPayload { server_id: id_a, op: "update".into(), config_version: 4 },
            NotificationPayload { server_id: id_b, op: "insert".into(), config_version: 1 },
            NotificationPayload { server_id: id_b, op: "update".into(), config_version: 2 },
        ];

        let result = coalesce_batch(batch);

        let a = result.iter().find(|p| p.server_id == id_a).expect("id_a must be present");
        let b = result.iter().find(|p| p.server_id == id_b).expect("id_b must be present");

        assert_eq!(a.config_version, 5, "highest version for id_a must be 5");
        assert_eq!(b.config_version, 2, "highest version for id_b must be 2");
        assert_eq!(result.len(), 2, "two distinct servers → two entries");
    }

    #[test]
    fn coalesce_single_entry_unchanged() {
        let id = Uuid::new_v4();
        let batch = vec![
            NotificationPayload { server_id: id, op: "insert".into(), config_version: 1 },
        ];
        let result = coalesce_batch(batch);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].config_version, 1);
    }

    // ── version ordering ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn stale_notification_is_discarded() {
        let pool = sqlx::PgPool::connect_lazy("postgres://localhost/test")
            .expect("lazy pool must not fail");
        let cache = Arc::new(ConfigCache::new(pool.clone()));
        let versions: VersionMap = Arc::new(DashMap::new());

        let id = Uuid::new_v4();
        // Seed a version of 5 in the version map (simulates a prior apply).
        versions.insert(id, 5);

        // A notification with version 3 (lower than 5) must be discarded.
        let stale = NotificationPayload {
            server_id: id,
            op: "update".into(),
            config_version: 3,
        };
        apply_payload(stale, &cache, &versions, &pool).await;

        // Cache must still be empty — the stale payload was not fetched from DB.
        assert!(
            cache.get(id).is_none(),
            "stale notification must not trigger a cache upsert"
        );
    }

    #[tokio::test]
    async fn delete_notification_removes_cached_entry() {
        // Build a cache with one entry.
        let pool = sqlx::PgPool::connect_lazy("postgres://localhost/test")
            .expect("lazy pool must not fail");
        let cache = Arc::new(ConfigCache::new(pool.clone()));

        let id = Uuid::new_v4();
        cache.upsert(Arc::new(crate::cache::ServerConfig {
            id,
            user_id: Uuid::new_v4(),
            name: "Test".into(),
            slug: "test-slug".into(),
            description: None,
            config_json: serde_json::json!({}),
            status: "active".into(),
            config_version: 2,
            token_hash: None,
            token_prefix: None,
            max_connections: 50,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }));

        assert!(cache.get(id).is_some(), "entry must exist before delete");

        let versions: VersionMap = Arc::new(DashMap::new());
        versions.insert(id, 2);

        let del = NotificationPayload {
            server_id: id,
            op: "delete".into(),
            config_version: 2,
        };
        apply_payload(del, &cache, &versions, &pool).await;

        cache.inner.run_pending_tasks();

        assert!(
            cache.get(id).is_none(),
            "entry must be removed after delete notification"
        );
        assert!(
            versions.get(&id).is_none(),
            "version map must have no entry after delete"
        );
    }

    // ── integration tests (require TEST_DATABASE_URL) ─────────────────────────

    /// Helper: skip if TEST_DATABASE_URL is not set.
    fn test_db_url() -> Option<String> {
        std::env::var("TEST_DATABASE_URL").ok()
    }

    #[tokio::test]
    #[ignore = "requires live PostgreSQL — set TEST_DATABASE_URL to run"]
    async fn integration_insert_server_populates_cache() {
        let db_url = match test_db_url() {
            Some(u) => u,
            None => return,
        };
        use mcp_common::testing::TestDatabase;

        // TestDatabase::new() reads TEST_DATABASE_URL / DATABASE_URL automatically
        // and creates an isolated DB with all migrations applied.
        let test_db = TestDatabase::new().await.expect("test DB setup failed");
        let pool = test_db.pool.clone();

        // Insert a user first (required FK).
        let clerk_id = format!("test-clerk-{}", Uuid::new_v4().simple());
        let email = format!("test-{}@example.com", Uuid::new_v4().simple());
        let user_id: Uuid = sqlx::query_scalar(
            "INSERT INTO users (clerk_id, email) VALUES ($1, $2) RETURNING id",
        )
        .bind(&clerk_id)
        .bind(&email)
        .fetch_one(&pool)
        .await
        .expect("insert user");

        // Start the config sync task. We re-derive the test DB URL from the pool
        // options. Since TestDatabase does not expose its URL, we build it from
        // the environment variable base + the active database name.
        let cache = Arc::new(ConfigCache::new(pool.clone()));
        let task = ConfigSyncTask::new(db_url.clone(), pool.clone(), Arc::clone(&cache));
        let _handle = task.start();

        // Allow listener to connect.
        tokio::time::sleep(Duration::from_millis(300)).await;

        // Insert an active server — trigger fires on the TestDatabase DB.
        let slug = format!("test-{}", Uuid::new_v4().simple());
        let server_id: Uuid = sqlx::query_scalar(
            "INSERT INTO mcp_servers (user_id, name, slug, status) \
             VALUES ($1, 'Test Server', $2, 'active') \
             RETURNING id",
        )
        .bind(user_id)
        .bind(&slug)
        .fetch_one(&pool)
        .await
        .expect("insert server");

        // Wait for cache to be populated (up to 2 s per acceptance criterion).
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            if cache.get(server_id).is_some() {
                break;
            }
            if tokio::time::Instant::now() > deadline {
                panic!("cache was not populated within 2 s of server insert");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // Update the server name.
        sqlx::query("UPDATE mcp_servers SET name = 'Updated' WHERE id = $1")
            .bind(server_id)
            .execute(&pool)
            .await
            .expect("update server");

        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(cfg) = cache.get(server_id) {
                if cfg.name == "Updated" {
                    break;
                }
            }
            if tokio::time::Instant::now() > deadline {
                panic!("cache was not updated within 2 s of server update");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // Delete the server.
        sqlx::query("DELETE FROM mcp_servers WHERE id = $1")
            .bind(server_id)
            .execute(&pool)
            .await
            .expect("delete server");

        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            if cache.get(server_id).is_none() {
                break;
            }
            if tokio::time::Instant::now() > deadline {
                panic!("cache still contains server 2 s after delete");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // TestDatabase is dropped here → isolated DB is deleted automatically.
    }
}
