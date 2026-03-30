//! Per-server and global connection counting with RAII guards.
//!
//! # Design
//!
//! [`ConnectionTracker`] enforces two independent limits:
//!
//! 1. **Global limit** — a `tokio::sync::Semaphore` with `GATEWAY_MAX_CONNECTIONS`
//!    permits. One permit is acquired per in-flight request. When the semaphore is
//!    exhausted, [`CapacityError::GlobalLimitReached`] is returned immediately.
//!
//! 2. **Per-server limit** — a `DashMap<Uuid, Arc<AtomicUsize>>` tracking current
//!    in-flight requests per server. If the count would exceed
//!    `ServerConfig::max_connections`, [`CapacityError::ServerLimitReached`] is
//!    returned and the global permit is released.
//!
//! [`ConnectionGuard`] is the RAII handle returned on success. Dropping it
//! (including on panic unwind) atomically decrements both the per-server counter
//! and the global counter, releases the semaphore permit, and notifies any
//! [`ConnectionTracker::drain`] callers.
//!
//! # Panic safety
//!
//! Rust's destructor mechanism (`Drop`) is called during stack unwinding, so
//! `ConnectionGuard::drop` runs even when the owning task panics. This satisfies
//! the same guarantee that `scopeguard::defer!` provides.
//!
//! # Prometheus metrics
//!
//! - `gateway_active_connections{server_id}` — per-server in-flight count.
//! - `gateway_active_connections_total` — global in-flight count.

use dashmap::DashMap;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

/// Default maximum in-flight connections per MCP server.
pub const DEFAULT_SERVER_MAX_CONNECTIONS: u32 = 50;

/// Default global maximum in-flight connections across all servers.
pub const DEFAULT_GLOBAL_MAX_CONNECTIONS: usize = 10_000;

/// After emitting a capacity-limit WARN log for a server, suppress further
/// warnings for this many seconds (rate-limiting to avoid log floods).
const WARN_RATE_SECS: u64 = 10;

// ── CapacityError ─────────────────────────────────────────────────────────────

/// Reason a connection was refused.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CapacityError {
    /// The target server has reached its per-server connection limit.
    #[error("server at capacity")]
    ServerLimitReached,
    /// The gateway has reached its global connection limit.
    #[error("gateway at capacity")]
    GlobalLimitReached,
}

// ── ConnectionGuard ───────────────────────────────────────────────────────────

/// RAII guard that releases connection slots when dropped.
///
/// Construct via [`ConnectionTracker::try_acquire`]. Keep it alive for the
/// duration of the request (including SSE sessions) so the counted slot stays
/// reserved. The guard decrements both the per-server atomic and the global
/// atomic, and releases the global semaphore permit on drop.
///
/// # Panic safety
///
/// `Drop` is always called during stack unwinding, so the decrement happens
/// even when the owning task panics. This provides the same guarantee as
/// `scopeguard::defer!`.
pub struct ConnectionGuard {
    server_id: Uuid,
    /// Shared per-server counter.
    server_counter: Arc<AtomicUsize>,
    /// Shared global counter (mirrors semaphore usage for drain tracking).
    global_count: Arc<AtomicUsize>,
    /// Releasing this permit decrements the semaphore, opening a slot for the
    /// next request. Field is never read; it is held purely for its drop effect.
    _global_permit: OwnedSemaphorePermit,
    /// Notified on every decrement so `drain()` can wake up.
    drain_notify: Arc<Notify>,
}

impl std::fmt::Debug for ConnectionGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionGuard")
            .field("server_id", &self.server_id)
            .finish_non_exhaustive()
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        // Decrement per-server counter.
        let prev_server = self.server_counter.fetch_sub(1, Ordering::SeqCst);
        metrics::gauge!(
            "gateway_active_connections",
            "server_id" => self.server_id.to_string()
        )
        .set(prev_server.saturating_sub(1) as f64);

        // Decrement global counter.
        let prev_global = self.global_count.fetch_sub(1, Ordering::SeqCst);
        metrics::gauge!("gateway_active_connections_total")
            .set(prev_global.saturating_sub(1) as f64);

        // Wake drain() if this was the last connection.
        // notify_waiters() wakes all registered Notified futures.
        self.drain_notify.notify_waiters();

        // `_global_permit` is dropped here, releasing the semaphore slot.
    }
}

// ── ConnectionTracker ─────────────────────────────────────────────────────────

/// Shared connection-limit tracker for both transports.
///
/// Construct once at startup:
/// ```ignore
/// let tracker = ConnectionTracker::new(10_000);
/// ```
///
/// Then call [`try_acquire`] in every request handler after authentication:
/// ```ignore
/// let _guard = tracker.try_acquire(server_id, config.max_connections)
///     .map_err(|e| capacity_503(e))?;
/// ```
pub struct ConnectionTracker {
    /// Per-server in-flight counters. Entries are created lazily and persist
    /// for the process lifetime; the set of servers is bounded in practice.
    per_server: DashMap<Uuid, Arc<AtomicUsize>>,
    /// Global semaphore: one permit per allowed concurrent connection.
    global_semaphore: Arc<Semaphore>,
    /// Total in-flight connections (mirrors semaphore usage without borrowing it).
    global_count: Arc<AtomicUsize>,
    /// Maximum global connections (used by `drain()` to detect quiescence).
    pub global_max: usize,
    /// Notified on each connection drop so `drain()` can recheck.
    drain_notify: Arc<Notify>,
    /// Most recent WARN emission per server (rate-limited to once per 10 s).
    last_warn: DashMap<Uuid, Instant>,
}

impl ConnectionTracker {
    /// Create a tracker enforcing `global_max` total concurrent connections.
    ///
    /// Returns an `Arc` because the tracker is shared across handler tasks.
    pub fn new(global_max: usize) -> Arc<Self> {
        Arc::new(Self {
            per_server: DashMap::new(),
            global_semaphore: Arc::new(Semaphore::new(global_max)),
            global_count: Arc::new(AtomicUsize::new(0)),
            global_max,
            drain_notify: Arc::new(Notify::new()),
            last_warn: DashMap::new(),
        })
    }

    /// Attempt to reserve a connection slot for `server_id`.
    ///
    /// Checks the global limit first (non-blocking semaphore try), then the
    /// per-server limit. Returns a [`ConnectionGuard`] on success; the guard
    /// releases all slots when it is dropped (including on panic unwind).
    ///
    /// # Errors
    ///
    /// - [`CapacityError::GlobalLimitReached`] — all global permits exhausted.
    /// - [`CapacityError::ServerLimitReached`] — server is at `server_max`.
    ///
    /// On `ServerLimitReached`, the already-acquired global permit is released
    /// before returning.
    pub fn try_acquire(
        &self,
        server_id: Uuid,
        server_max: u32,
    ) -> Result<ConnectionGuard, CapacityError> {
        // ── 1. Global limit (non-blocking) ────────────────────────────────────
        let permit = Arc::clone(&self.global_semaphore)
            .try_acquire_owned()
            .map_err(|_| CapacityError::GlobalLimitReached)?;

        // ── 2. Per-server limit ───────────────────────────────────────────────
        //
        // get-or-create the counter, then try to CAS-increment below the limit.
        let counter: Arc<AtomicUsize> = {
            let ref_mut = self
                .per_server
                .entry(server_id)
                .or_insert_with(|| Arc::new(AtomicUsize::new(0)));
            Arc::clone(&*ref_mut)
            // ref_mut (shard lock) released here
        };

        let max = server_max as usize;
        let result = counter.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
            if current < max {
                Some(current + 1)
            } else {
                None
            }
        });

        match result {
            Ok(prev_val) => {
                let new_server_count = prev_val + 1;
                metrics::gauge!(
                    "gateway_active_connections",
                    "server_id" => server_id.to_string()
                )
                .set(new_server_count as f64);

                let prev_global = self.global_count.fetch_add(1, Ordering::SeqCst);
                metrics::gauge!("gateway_active_connections_total")
                    .set((prev_global + 1) as f64);

                Ok(ConnectionGuard {
                    server_id,
                    server_counter: counter,
                    global_count: Arc::clone(&self.global_count),
                    _global_permit: permit,
                    drain_notify: Arc::clone(&self.drain_notify),
                })
            }
            Err(_) => {
                // Release global permit before returning.
                drop(permit);
                // Emit a WARN log (rate-limited to once per 10 s per server).
                self.maybe_warn_capacity(server_id, server_max);
                Err(CapacityError::ServerLimitReached)
            }
        }
    }

    /// Returns a future that resolves once all active connections have dropped.
    ///
    /// Used by the graceful-shutdown sequence to wait for in-flight requests.
    pub async fn drain(&self) {
        loop {
            // Create the notification future BEFORE checking the count.
            // This eliminates the race where the last connection drops between
            // the count-check and the `.await`, causing us to miss the wakeup.
            let notified = self.drain_notify.notified();

            if self.global_count.load(Ordering::SeqCst) == 0 {
                return;
            }

            notified.await;
        }
    }

    /// Current per-server in-flight count (for tests and metrics queries).
    pub fn server_count(&self, server_id: Uuid) -> usize {
        self.per_server
            .get(&server_id)
            .map(|c| c.load(Ordering::SeqCst))
            .unwrap_or(0)
    }

    /// Current global in-flight count (for tests and metrics queries).
    pub fn global_active(&self) -> usize {
        self.global_count.load(Ordering::SeqCst)
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    /// Emit a capacity WARN log for `server_id`, rate-limited to once per 10 s.
    fn maybe_warn_capacity(&self, server_id: Uuid, max: u32) {
        let now = Instant::now();
        let should_warn = self
            .last_warn
            .get(&server_id)
            .map(|last| now.duration_since(*last) >= Duration::from_secs(WARN_RATE_SECS))
            .unwrap_or(true);

        if should_warn {
            self.last_warn.insert(server_id, now);
            tracing::warn!(
                server_id = %server_id,
                max_connections = max,
                "server has reached its connection limit"
            );
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
    use std::panic::AssertUnwindSafe;

    fn server_id() -> Uuid {
        Uuid::new_v4()
    }

    // ── Counter increments / decrements ───────────────────────────────────────

    #[test]
    fn counter_increments_on_acquire() {
        let tracker = ConnectionTracker::new(100);
        let sid = server_id();

        let _guard = tracker.try_acquire(sid, 10).unwrap();
        assert_eq!(tracker.server_count(sid), 1);
        assert_eq!(tracker.global_active(), 1);
    }

    #[test]
    fn counter_decrements_on_drop() {
        let tracker = ConnectionTracker::new(100);
        let sid = server_id();

        {
            let _g1 = tracker.try_acquire(sid, 10).unwrap();
            let _g2 = tracker.try_acquire(sid, 10).unwrap();
            assert_eq!(tracker.server_count(sid), 2);
            assert_eq!(tracker.global_active(), 2);
        } // both guards drop here

        assert_eq!(tracker.server_count(sid), 0);
        assert_eq!(tracker.global_active(), 0);
    }

    // ── Per-server 503 when limit exceeded ────────────────────────────────────

    #[test]
    fn server_limit_respected() {
        let tracker = ConnectionTracker::new(100);
        let sid = server_id();

        let _g1 = tracker.try_acquire(sid, 2).unwrap();
        let _g2 = tracker.try_acquire(sid, 2).unwrap();

        let err = tracker.try_acquire(sid, 2).unwrap_err();
        assert_eq!(err, CapacityError::ServerLimitReached);

        // Global count must not have been incremented by the failed attempt.
        assert_eq!(tracker.global_active(), 2);
    }

    #[test]
    fn server_slot_freed_allows_next_request() {
        let tracker = ConnectionTracker::new(100);
        let sid = server_id();

        let g1 = tracker.try_acquire(sid, 1).unwrap();
        assert!(tracker.try_acquire(sid, 1).is_err());

        drop(g1);

        // Now it should succeed again.
        let _g2 = tracker.try_acquire(sid, 1).unwrap();
        assert_eq!(tracker.server_count(sid), 1);
    }

    // ── Global limit respected ─────────────────────────────────────────────────

    #[test]
    fn global_limit_respected() {
        let tracker = ConnectionTracker::new(2);
        let sid1 = server_id();
        let sid2 = server_id();
        let sid3 = server_id();

        let _g1 = tracker.try_acquire(sid1, 50).unwrap();
        let _g2 = tracker.try_acquire(sid2, 50).unwrap();

        let err = tracker.try_acquire(sid3, 50).unwrap_err();
        assert_eq!(err, CapacityError::GlobalLimitReached);
    }

    #[test]
    fn global_limit_frees_after_drop() {
        let tracker = ConnectionTracker::new(1);
        let sid = server_id();

        let g = tracker.try_acquire(sid, 50).unwrap();
        assert!(tracker.try_acquire(server_id(), 50).is_err());

        drop(g);

        let _g2 = tracker.try_acquire(server_id(), 50).unwrap();
        assert_eq!(tracker.global_active(), 1);
    }

    // ── Multiple servers are independent ─────────────────────────────────────

    #[test]
    fn per_server_counters_are_independent() {
        let tracker = ConnectionTracker::new(100);
        let sid_a = server_id();
        let sid_b = server_id();

        let _ga = tracker.try_acquire(sid_a, 1).unwrap();
        // sid_a is at its limit but sid_b is not
        let _gb = tracker.try_acquire(sid_b, 1).unwrap();

        assert_eq!(tracker.server_count(sid_a), 1);
        assert_eq!(tracker.server_count(sid_b), 1);
        assert_eq!(tracker.global_active(), 2);

        // sid_a is still at limit
        assert_eq!(
            tracker.try_acquire(sid_a, 1).unwrap_err(),
            CapacityError::ServerLimitReached
        );
    }

    // ── Decrement on panic (Rust Drop = unwind-safe, same guarantee as scopeguard::defer!) ──

    #[test]
    fn decrement_on_panic_via_drop_unwind() {
        let tracker = ConnectionTracker::new(100);
        let sid = server_id();
        let tracker_clone = Arc::clone(&tracker);

        let result = std::panic::catch_unwind(AssertUnwindSafe(move || {
            let _guard = tracker_clone.try_acquire(sid, 10).unwrap();
            assert_eq!(tracker_clone.server_count(sid), 1);
            // Panic while holding the guard — Drop must still run.
            panic!("intentional test panic");
        }));

        // Verify catch_unwind caught the panic.
        assert!(result.is_err(), "catch_unwind must return Err on panic");

        // Verify the guard was dropped (counter decremented) during unwind.
        assert_eq!(
            tracker.server_count(sid),
            0,
            "server_count must be 0 after panic unwind (Drop called)"
        );
        assert_eq!(
            tracker.global_active(),
            0,
            "global_active must be 0 after panic unwind"
        );
    }

    // ── drain() resolves when all connections drop ─────────────────────────────

    #[tokio::test]
    async fn drain_resolves_when_empty() {
        let tracker = ConnectionTracker::new(100);
        // No connections — drain should resolve immediately.
        tokio::time::timeout(std::time::Duration::from_millis(50), tracker.drain())
            .await
            .expect("drain must resolve immediately when no connections are active");
    }

    #[tokio::test]
    async fn drain_waits_for_active_connections() {
        let tracker = ConnectionTracker::new(100);
        let sid = server_id();

        let guard = tracker.try_acquire(sid, 10).unwrap();
        let tracker_clone = Arc::clone(&tracker);

        // Spawn a task that drains once the guard drops.
        let drain_task = tokio::spawn(async move {
            tracker_clone.drain().await;
        });

        // Give the drain task a moment to start waiting.
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        drop(guard);

        tokio::time::timeout(std::time::Duration::from_millis(100), drain_task)
            .await
            .expect("timeout")
            .expect("drain task must not panic");
    }
}
