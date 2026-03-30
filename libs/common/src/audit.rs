//! Async batched audit logging service.
//!
//! [`AuditLogger`] enqueues events non-blockingly and writes them to
//! PostgreSQL in batches via a background tokio task. The batch is flushed
//! when it reaches 50 events or every 100 ms, whichever occurs first.
//!
//! # Example
//!
//! ```rust,no_run
//! # use mcp_common::audit::{AuditLogger, AuditEvent, AuditAction};
//! # use sqlx::PgPool;
//! # async fn example(pool: PgPool) {
//! let logger = AuditLogger::new(pool);
//!
//! logger.log(AuditEvent {
//!     action: AuditAction::AuthSuccess,
//!     user_id: None,
//!     server_id: None,
//!     success: true,
//!     error_msg: None,
//!     metadata: None,
//!     correlation_id: None,
//! });
//!
//! logger.shutdown().await;
//! # }
//! ```

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use serde::Serialize;
use sqlx::PgPool;
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
use uuid::Uuid;

use crate::SanitizedErrorMsg;

// ── Constants ──────────────────────────────────────────────────────────────────

/// Capacity of the in-memory event channel.
/// At a sustained 100 events/ms a full channel represents only ~40ms of
/// backpressure; events beyond this are dropped with a warning.
const CHANNEL_CAPACITY: usize = 4_096;

/// Number of events to accumulate before flushing to the database.
const BATCH_SIZE: usize = 50;

/// Maximum time (ms) to hold events before flushing to the database.
const FLUSH_INTERVAL_MS: u64 = 100;

/// Maximum time (s) to wait for the background writer to drain on shutdown.
const SHUTDOWN_TIMEOUT_SECS: u64 = 5;

// ── AuditAction ───────────────────────────────────────────────────────────────

/// All auditable action types in the MVP.
///
/// Serializes to snake_case strings for storage in the `audit_log.action`
/// column.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditAction {
    // ── Authentication ────────────────────────────────────────────────────────
    /// A user successfully authenticated.
    AuthSuccess,
    /// A user authentication attempt failed.
    AuthFailure,
    /// A Clerk webhook arrived with an invalid signature.
    WebhookAuthFailure,

    // ── Credentials ──────────────────────────────────────────────────────────
    /// A credential was created.
    CredentialCreate,
    /// A credential was rotated (value replaced).
    CredentialRotate,
    /// A credential was deleted.
    CredentialDelete,
    /// A credential was accessed by the injector sidecar (success).
    CredentialAccess,
    /// A credential access attempt by the injector sidecar failed.
    CredentialAccessFailure,

    // ── Security events ───────────────────────────────────────────────────────
    /// An outgoing request was blocked by SSRF protection.
    SsrfBlock,
    /// A caller exceeded their rate limit.
    RateLimitExceeded,

    // ── Server management ─────────────────────────────────────────────────────
    /// An MCP server configuration was created.
    ServerCreate,
    /// An MCP server configuration was updated.
    ServerUpdate,
    /// An MCP server configuration was deleted.
    ServerDelete,
    /// A server access token was generated.
    ServerTokenGenerate,
    /// A server access token was revoked.
    ServerTokenRevoke,

    // ── Proxy ─────────────────────────────────────────────────────────────────
    /// A builder proxy test call was dispatched on behalf of a user.
    ProxyTest,
}

impl AuditAction {
    /// Returns the canonical snake_case string stored in the database.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AuthSuccess => "auth_success",
            Self::AuthFailure => "auth_failure",
            Self::WebhookAuthFailure => "webhook_auth_failure",
            Self::CredentialCreate => "credential_create",
            Self::CredentialRotate => "credential_rotate",
            Self::CredentialDelete => "credential_delete",
            Self::CredentialAccess => "credential_access",
            Self::CredentialAccessFailure => "credential_access_failure",
            Self::SsrfBlock => "ssrf_block",
            Self::RateLimitExceeded => "rate_limit_exceeded",
            Self::ServerCreate => "server_create",
            Self::ServerUpdate => "server_update",
            Self::ServerDelete => "server_delete",
            Self::ServerTokenGenerate => "server_token_generate",
            Self::ServerTokenRevoke => "server_token_revoke",
            Self::ProxyTest => "proxy_test",
        }
    }
}

// ── AuditEvent ─────────────────────────────────────────────────────────────────

/// A single audit event to be persisted in the `audit_log` table.
#[derive(Debug, Clone)]
pub struct AuditEvent {
    /// The type of action being recorded.
    pub action: AuditAction,
    /// The authenticated user who performed the action, if known.
    /// Maps to `audit_log.actor_id`.
    pub user_id: Option<Uuid>,
    /// The MCP server affected by the action, if applicable.
    /// Maps to `audit_log.resource_id`.
    pub server_id: Option<Uuid>,
    /// Whether the action completed successfully.
    pub success: bool,
    /// A sanitized error message for failed actions.
    ///
    /// Using [`SanitizedErrorMsg`] (not `String`) enforces at compile time that
    /// only pre-approved, sanitized strings enter the audit log — never raw SQL
    /// errors, stack traces, or other internal details.
    pub error_msg: Option<SanitizedErrorMsg>,
    /// Arbitrary structured metadata for the event.
    pub metadata: Option<serde_json::Value>,
    /// An optional correlation ID linking this event to a distributed trace.
    pub correlation_id: Option<String>,
}

impl AuditEvent {
    /// Merges `success`, `error_msg`, and `correlation_id` into the
    /// caller-supplied `metadata` for storage in the single JSONB column.
    ///
    /// If `metadata` is already a JSON object, the extra fields are injected
    /// into it (preserving existing keys). Non-object values are wrapped under
    /// the key `"data"` before the extra fields are added.
    fn merged_metadata(&self) -> serde_json::Value {
        let mut map: serde_json::Map<String, serde_json::Value> = match &self.metadata {
            Some(serde_json::Value::Object(m)) => m.clone(),
            Some(other) => {
                let mut m = serde_json::Map::new();
                m.insert("data".to_string(), other.clone());
                m
            }
            None => serde_json::Map::new(),
        };

        map.insert("success".to_string(), serde_json::Value::Bool(self.success));

        if let Some(ref msg) = self.error_msg {
            map.insert(
                "error_msg".to_string(),
                serde_json::Value::String(msg.as_str().to_string()),
            );
        }

        if let Some(ref cid) = self.correlation_id {
            map.insert(
                "correlation_id".to_string(),
                serde_json::Value::String(cid.clone()),
            );
        }

        serde_json::Value::Object(map)
    }
}

// ── AuditLogger ───────────────────────────────────────────────────────────────

struct AuditLoggerInner {
    sender: mpsc::Sender<AuditEvent>,
    /// One-shot shutdown signal. Consumed on the first `shutdown()` call.
    shutdown_tx: Mutex<Option<oneshot::Sender<()>>>,
    /// Handle for the background writer task. Taken (and awaited) by `shutdown()`.
    task: Mutex<Option<JoinHandle<()>>>,
}

/// Async batched audit logger.
///
/// Events are enqueued non-blockingly via [`log`](AuditLogger::log) and
/// flushed to PostgreSQL by a background tokio task. The flush occurs when
/// the batch reaches [`BATCH_SIZE`] events or every [`FLUSH_INTERVAL_MS`] ms.
///
/// `AuditLogger` is **cheaply cloneable**: all clones share the same
/// underlying channel and background task.
///
/// Call [`shutdown`](AuditLogger::shutdown) during graceful shutdown to
/// ensure all buffered events are flushed before the process exits.
#[derive(Clone)]
pub struct AuditLogger(Arc<AuditLoggerInner>);

impl AuditLogger {
    /// Creates a new `AuditLogger` and spawns the background writer task.
    ///
    /// Must be called from within a tokio runtime context.
    pub fn new(pool: PgPool) -> Self {
        let (sender, receiver) = mpsc::channel(CHANNEL_CAPACITY);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        let task = tokio::task::spawn(run_writer(pool, receiver, shutdown_rx));

        Self(Arc::new(AuditLoggerInner {
            sender,
            shutdown_tx: Mutex::new(Some(shutdown_tx)),
            task: Mutex::new(Some(task)),
        }))
    }

    /// Enqueues an audit event for asynchronous persistence.
    ///
    /// This method **never blocks**. If the internal channel is full (more than
    /// 4 096 pending events), the event is silently dropped and a `warn`-level
    /// log message is emitted. This design ensures audit logging never
    /// back-pressures the primary request path.
    pub fn log(&self, event: AuditEvent) {
        match self.0.sender.try_send(event) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                tracing::warn!("audit channel full — event dropped");
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                // Background task has already exited (post-shutdown).
                tracing::warn!("audit channel closed — event dropped");
            }
        }
    }

    /// Signals the background writer to flush all buffered events and stop.
    ///
    /// Waits up to 5 seconds for the background task to complete. If the
    /// drain timeout expires, a warning is logged and the method returns —
    /// allowing the process to continue its shutdown sequence.
    ///
    /// Subsequent calls to `shutdown()` are no-ops (idempotent).
    pub async fn shutdown(&self) {
        // Take the shutdown sender; subsequent calls become no-ops.
        let tx = self
            .0
            .shutdown_tx
            .lock()
            .ok()
            .and_then(|mut guard| guard.take());

        if let Some(tx) = tx {
            // Ignore the error if the receiver was already dropped (task panic).
            let _ = tx.send(());
        }

        // Take the task handle and await it with a timeout.
        let handle = self.0.task.lock().ok().and_then(|mut guard| guard.take());

        if let Some(handle) = handle {
            match tokio::time::timeout(Duration::from_secs(SHUTDOWN_TIMEOUT_SECS), handle).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::warn!(error = %e, "audit writer task panicked during shutdown");
                }
                Err(_elapsed) => {
                    tracing::warn!(
                        "audit writer task did not finish within {}s drain timeout",
                        SHUTDOWN_TIMEOUT_SECS
                    );
                }
            }
        }
    }
}

// ── Background writer ─────────────────────────────────────────────────────────

/// Entry point for the background writer task.
///
/// Reads events from `receiver`, accumulates them into a batch, and flushes
/// to PostgreSQL when the batch is full or the flush timer fires.
async fn run_writer(
    pool: PgPool,
    mut receiver: mpsc::Receiver<AuditEvent>,
    mut shutdown_rx: oneshot::Receiver<()>,
) {
    let mut flush_timer =
        tokio::time::interval(Duration::from_millis(FLUSH_INTERVAL_MS));
    // Skip ticks that fired while we were busy flushing.
    flush_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut buffer: Vec<AuditEvent> = Vec::with_capacity(BATCH_SIZE);

    loop {
        tokio::select! {
            // `biased` gives shutdown the highest priority: once signalled,
            // we drain remaining events from the channel and exit cleanly.
            biased;

            // ── Shutdown signal ───────────────────────────────────────────
            _ = &mut shutdown_rx => {
                // Drain any events already queued in the channel.
                while let Ok(event) = receiver.try_recv() {
                    buffer.push(event);
                }
                if !buffer.is_empty() {
                    flush_batch(&pool, std::mem::take(&mut buffer)).await;
                }
                break;
            }

            // ── New event received ────────────────────────────────────────
            maybe_event = receiver.recv() => {
                match maybe_event {
                    Some(event) => {
                        buffer.push(event);
                        if buffer.len() >= BATCH_SIZE {
                            flush_batch(&pool, std::mem::take(&mut buffer)).await;
                        }
                    }
                    // All senders dropped (shouldn't happen before shutdown).
                    None => {
                        if !buffer.is_empty() {
                            flush_batch(&pool, std::mem::take(&mut buffer)).await;
                        }
                        break;
                    }
                }
            }

            // ── Periodic flush ────────────────────────────────────────────
            _ = flush_timer.tick() => {
                if !buffer.is_empty() {
                    flush_batch(&pool, std::mem::take(&mut buffer)).await;
                }
            }
        }
    }
}

/// Writes a batch of events to PostgreSQL inside a single transaction.
///
/// Individual insert errors are logged but do not abort the transaction:
/// we attempt to commit whatever succeeded. If the commit itself fails,
/// the error is logged and the batch is lost (this is acceptable for audit
/// logging — we never retry to avoid double-writes).
async fn flush_batch(pool: &PgPool, batch: Vec<AuditEvent>) {
    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!(
                error = %e,
                batch_size = batch.len(),
                "audit log: failed to begin transaction"
            );
            return;
        }
    };

    for event in &batch {
        let action = event.action.as_str();
        let metadata = event.merged_metadata();

        let result = sqlx::query(
            "INSERT INTO audit_log (actor_id, action, resource_id, metadata)
             VALUES ($1, $2, $3, $4)",
        )
        .bind(event.user_id)
        .bind(action)
        .bind(event.server_id)
        .bind(metadata)
        .execute(&mut *tx)
        .await;

        if let Err(e) = result {
            tracing::error!(
                error = %e,
                action = action,
                "audit log: failed to insert event"
            );
            // Continue to next event; don't abort the transaction since
            // other events in the batch may still be insertable.
        }
    }

    if let Err(e) = tx.commit().await {
        tracing::error!(
            error = %e,
            batch_size = batch.len(),
            "audit log: failed to commit batch"
        );
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use std::time::Instant;

    use super::*;

    fn make_event(action: AuditAction) -> AuditEvent {
        AuditEvent {
            action,
            user_id: None,
            server_id: None,
            success: true,
            error_msg: None,
            metadata: None,
            correlation_id: None,
        }
    }

    // ── AuditAction serialization ─────────────────────────────────────────────

    #[test]
    fn test_audit_action_as_str_auth_success() {
        assert_eq!(AuditAction::AuthSuccess.as_str(), "auth_success");
    }

    #[test]
    fn test_audit_action_as_str_auth_failure() {
        assert_eq!(AuditAction::AuthFailure.as_str(), "auth_failure");
    }

    #[test]
    fn test_audit_action_as_str_webhook_auth_failure() {
        assert_eq!(AuditAction::WebhookAuthFailure.as_str(), "webhook_auth_failure");
    }

    #[test]
    fn test_audit_action_as_str_credential_create() {
        assert_eq!(AuditAction::CredentialCreate.as_str(), "credential_create");
    }

    #[test]
    fn test_audit_action_as_str_credential_rotate() {
        assert_eq!(AuditAction::CredentialRotate.as_str(), "credential_rotate");
    }

    #[test]
    fn test_audit_action_as_str_credential_delete() {
        assert_eq!(AuditAction::CredentialDelete.as_str(), "credential_delete");
    }

    #[test]
    fn test_audit_action_as_str_credential_access() {
        assert_eq!(AuditAction::CredentialAccess.as_str(), "credential_access");
    }

    #[test]
    fn test_audit_action_as_str_credential_access_failure() {
        assert_eq!(
            AuditAction::CredentialAccessFailure.as_str(),
            "credential_access_failure"
        );
    }

    #[test]
    fn test_audit_action_as_str_ssrf_block() {
        assert_eq!(AuditAction::SsrfBlock.as_str(), "ssrf_block");
    }

    #[test]
    fn test_audit_action_as_str_rate_limit_exceeded() {
        assert_eq!(AuditAction::RateLimitExceeded.as_str(), "rate_limit_exceeded");
    }

    #[test]
    fn test_audit_action_as_str_server_create() {
        assert_eq!(AuditAction::ServerCreate.as_str(), "server_create");
    }

    #[test]
    fn test_audit_action_as_str_server_update() {
        assert_eq!(AuditAction::ServerUpdate.as_str(), "server_update");
    }

    #[test]
    fn test_audit_action_as_str_server_delete() {
        assert_eq!(AuditAction::ServerDelete.as_str(), "server_delete");
    }

    #[test]
    fn test_audit_action_as_str_server_token_generate() {
        assert_eq!(AuditAction::ServerTokenGenerate.as_str(), "server_token_generate");
    }

    #[test]
    fn test_audit_action_as_str_server_token_revoke() {
        assert_eq!(AuditAction::ServerTokenRevoke.as_str(), "server_token_revoke");
    }

    #[test]
    fn test_audit_action_serializes_to_snake_case() {
        let json = serde_json::to_string(&AuditAction::AuthSuccess).unwrap();
        assert_eq!(json, r#""auth_success""#);
        let json = serde_json::to_string(&AuditAction::CredentialAccessFailure).unwrap();
        assert_eq!(json, r#""credential_access_failure""#);
    }

    // ── AuditEvent::merged_metadata ───────────────────────────────────────────

    #[test]
    fn test_merged_metadata_always_includes_success() {
        let event = make_event(AuditAction::AuthSuccess);
        let meta = event.merged_metadata();
        assert_eq!(meta["success"], serde_json::Value::Bool(true));
    }

    #[test]
    fn test_merged_metadata_includes_error_msg() {
        let mut event = make_event(AuditAction::AuthFailure);
        event.success = false;
        event.error_msg = Some(SanitizedErrorMsg::new("invalid token"));
        let meta = event.merged_metadata();
        assert_eq!(meta["error_msg"], "invalid token");
        assert_eq!(meta["success"], serde_json::Value::Bool(false));
    }

    #[test]
    fn test_merged_metadata_includes_correlation_id() {
        let mut event = make_event(AuditAction::ServerCreate);
        event.correlation_id = Some("trace-abc123".to_string());
        let meta = event.merged_metadata();
        assert_eq!(meta["correlation_id"], "trace-abc123");
    }

    #[test]
    fn test_merged_metadata_merges_with_user_object() {
        let mut event = make_event(AuditAction::CredentialCreate);
        event.metadata = Some(serde_json::json!({"key_name": "stripe_api_key"}));
        let meta = event.merged_metadata();
        assert_eq!(meta["key_name"], "stripe_api_key");
        assert_eq!(meta["success"], serde_json::Value::Bool(true));
    }

    #[test]
    fn test_merged_metadata_wraps_non_object_metadata() {
        let mut event = make_event(AuditAction::AuthSuccess);
        event.metadata = Some(serde_json::Value::String("some string".to_string()));
        let meta = event.merged_metadata();
        assert_eq!(meta["data"], "some string");
        assert!(meta["success"].is_boolean());
    }

    // ── AuditLogger: channel and shutdown ─────────────────────────────────────

    /// AuditLogger::log p99 latency must be < 1ms for 1000 calls.
    ///
    /// The method only pushes to an mpsc channel (non-blocking). This
    /// should be well under 1 µs per call on any hardware, so the 1 ms
    /// budget is very conservative.
    #[tokio::test]
    async fn test_audit_logger_log_p99_under_1ms() {
        // Use connect_lazy so no real DB connection is made.
        let pool = sqlx::PgPool::connect_lazy("postgres://localhost/test")
            .expect("connect_lazy failed");
        let logger = AuditLogger::new(pool);

        let n = 1_000usize;
        let mut latencies: Vec<std::time::Duration> = Vec::with_capacity(n);

        for _ in 0..n {
            let t = Instant::now();
            logger.log(make_event(AuditAction::AuthSuccess));
            latencies.push(t.elapsed());
        }

        latencies.sort_unstable();
        let p99 = latencies[n * 99 / 100]; // index 990 = 99th percentile of 1000
        assert!(
            p99.as_millis() < 1,
            "p99 log() latency must be < 1ms; got {:?}",
            p99
        );

        // Shut down cleanly (DB writes will fail since there's no real DB —
        // the background task logs errors and exits).
        logger.shutdown().await;
    }

    #[tokio::test]
    async fn test_audit_logger_channel_overflow_drops_event() {
        let pool = sqlx::PgPool::connect_lazy("postgres://localhost/test")
            .expect("connect_lazy failed");
        let logger = AuditLogger::new(pool);

        // Fill channel beyond capacity. Should not panic or block.
        for _ in 0..CHANNEL_CAPACITY + 100 {
            logger.log(make_event(AuditAction::RateLimitExceeded));
        }

        logger.shutdown().await;
    }

    #[tokio::test]
    async fn test_audit_logger_shutdown_is_idempotent() {
        let pool = sqlx::PgPool::connect_lazy("postgres://localhost/test")
            .expect("connect_lazy failed");
        let logger = AuditLogger::new(pool);

        // Multiple shutdown() calls must not panic.
        logger.shutdown().await;
        logger.shutdown().await;
    }

    #[tokio::test]
    async fn test_audit_logger_clone_shares_channel() {
        let pool = sqlx::PgPool::connect_lazy("postgres://localhost/test")
            .expect("connect_lazy failed");
        let logger = AuditLogger::new(pool);
        let logger2 = logger.clone();

        // Both references enqueue to the same channel without panicking.
        logger.log(make_event(AuditAction::ServerCreate));
        logger2.log(make_event(AuditAction::ServerDelete));

        logger.shutdown().await;
    }
}
