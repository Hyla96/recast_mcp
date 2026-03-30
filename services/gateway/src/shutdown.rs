//! Graceful shutdown coordination for the MCP gateway.
//!
//! # Shutdown sequence
//!
//! 1. **Signal**: SIGTERM or SIGINT is received.
//! 2. **Phase A — shutdown_initiated**: `is_shutting_down` is set to `true`;
//!    the MCP transport handler returns HTTP 503 + `Connection: close` for any
//!    new MCP requests; the readiness probe returns 503 (via the wrapped
//!    db_checker returned by [`make_shutdown_db_checker`]).
//! 3. **Phase B — lb_drain_complete**: 5-second LB drain window.
//!    The load balancer detects the 503 readiness probe and stops sending
//!    new traffic to this instance while in-flight requests continue.
//! 4. **Phase C — connections_drained**: [`ConnectionTracker::drain`] is
//!    awaited with a 30-second timeout. On timeout a WARN is emitted with the
//!    count of forcibly dropped connections.
//! 5. **Phase F**: The hot-reload listener task is aborted.
//! 6. **Phase D/E — logs_flushed**: The async request logger is flushed via a
//!    oneshot sentinel; all buffered records are written before this resolves.
//! 7. **Exiting**: Structured INFO log; remaining resources are released when
//!    `main()` returns.
//!
//! Total wall time is bounded: 5 s (LB drain) + 30 s (connection drain) + a
//! few ms for I/O flush = well under the 40-second SLA.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Instant;

use mcp_common::health::{DbCheckFuture, DbCheckerFn};

use crate::connections::ConnectionTracker;
use crate::logging::RequestLogger;

// ── await_shutdown_signal ─────────────────────────────────────────────────────

/// Block until SIGTERM or SIGINT is received.
///
/// Uses `tokio::select!` to race both signals; whichever arrives first
/// triggers the shutdown sequence.
pub async fn await_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};

        match signal(SignalKind::terminate()) {
            Ok(mut sigterm) => {
                tokio::select! {
                    _ = sigterm.recv() => {
                        tracing::info!(signal = "SIGTERM", "shutdown signal received");
                    }
                    result = tokio::signal::ctrl_c() => {
                        match result {
                            Ok(()) => tracing::info!(signal = "SIGINT", "shutdown signal received"),
                            Err(e) => tracing::error!(error = %e, "ctrl_c handler error"),
                        }
                    }
                }
            }
            Err(e) => {
                // SIGTERM registration failed (rare; e.g., running in a restricted
                // sandbox). Fall back to SIGINT only.
                tracing::warn!(error = %e, "failed to register SIGTERM handler; shutdown via SIGINT only");
                match tokio::signal::ctrl_c().await {
                    Ok(()) => tracing::info!(signal = "SIGINT", "shutdown signal received"),
                    Err(err) => tracing::error!(error = %err, "ctrl_c handler error"),
                }
            }
        }
    }

    #[cfg(not(unix))]
    {
        match tokio::signal::ctrl_c().await {
            Ok(()) => tracing::info!(signal = "SIGINT", "shutdown signal received"),
            Err(e) => tracing::error!(error = %e, "ctrl_c handler error"),
        }
    }
}

// ── run_shutdown_sequence ─────────────────────────────────────────────────────

/// Run the graceful-shutdown sequence after the HTTP server stops accepting
/// new connections.
///
/// # Phases
///
/// | Phase constant          | Action                                                  |
/// |-------------------------|---------------------------------------------------------|
/// | `connections_drained`   | [`ConnectionTracker::drain`] with 30 s timeout.        |
/// | `logs_flushed`          | Flush async log writer channel (oneshot sentinel).      |
/// | `exiting`               | Final structured log; caller returns to drop resources. |
///
/// Called from `main()` **after**
/// `axum::serve(...).with_graceful_shutdown(...).await` returns — by that
/// point axum has already drained its HTTP connections, so the 30-second
/// timeout should complete immediately in practice.
pub async fn run_shutdown_sequence(
    connection_tracker: Arc<ConnectionTracker>,
    logger: Arc<RequestLogger>,
    sync_handle: tokio::task::JoinHandle<()>,
) {
    let t0 = Instant::now();

    // ── Phase C: drain in-flight connections (30 s hard timeout) ─────────────
    let drain_result = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        connection_tracker.drain(),
    )
    .await;

    match drain_result {
        Ok(()) => {
            tracing::info!(
                phase = "connections_drained",
                elapsed_ms = t0.elapsed().as_millis() as u64,
                "all in-flight connections drained"
            );
        }
        Err(_timeout) => {
            let remaining = connection_tracker.global_active();
            tracing::warn!(
                phase = "connections_drained",
                elapsed_ms = t0.elapsed().as_millis() as u64,
                forcibly_dropped = remaining,
                "drain timeout; forcibly dropping {remaining} in-flight connections"
            );
        }
    }

    // ── Phase F: stop the hot-reload listener ─────────────────────────────────
    // The task is supervised, so aborting it is safe — it will not leave the
    // cache in an inconsistent state (we are already shutting down).
    sync_handle.abort();

    // ── Phase D/E: flush the async log writer ─────────────────────────────────
    // `flush()` sends a sentinel through the mpsc channel and waits for the
    // background writer to process all preceding log records.
    logger.flush().await;

    tracing::info!(
        phase = "logs_flushed",
        elapsed_ms = t0.elapsed().as_millis() as u64,
        "log writer flushed"
    );

    // ── Exiting ────────────────────────────────────────────────────────────────
    // Remaining resources (PgPool, SidecarPool, moka caches, etc.) are released
    // by Drop when `main()` returns after this function.
    tracing::info!(
        phase = "exiting",
        elapsed_ms = t0.elapsed().as_millis() as u64,
        "gateway shutdown complete"
    );
}

// ── make_shutdown_db_checker ──────────────────────────────────────────────────

/// Wrap a `DbCheckerFn` so that `/health/ready` returns HTTP 503 immediately
/// once `is_shutting_down` is set to `true`.
///
/// This causes the load balancer to detect the failing readiness probe and
/// stop routing new connections to this instance during the drain window
/// (Phase B of the shutdown sequence).
///
/// The liveness probe (`/health/live`) is **not** affected: it returns 200
/// for the entire lifetime of the process, including during shutdown.
pub fn make_shutdown_db_checker(
    inner: DbCheckerFn,
    is_shutting_down: Arc<AtomicBool>,
) -> DbCheckerFn {
    Arc::new(move || -> DbCheckFuture {
        let inner = inner.clone();
        let flag = Arc::clone(&is_shutting_down);
        Box::pin(async move {
            if flag.load(Ordering::SeqCst) {
                return Err("gateway is shutting down".to_string());
            }
            inner().await
        })
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unimplemented,
        clippy::todo
    )]

    use super::*;
    use std::sync::atomic::AtomicBool;

    // ── make_shutdown_db_checker ───────────────────────────────────────────────

    #[tokio::test]
    async fn db_checker_returns_ok_when_not_shutting_down() {
        let inner: DbCheckerFn = Arc::new(|| Box::pin(async { Ok(()) }));
        let flag = Arc::new(AtomicBool::new(false));
        let wrapped = make_shutdown_db_checker(inner, flag);
        assert!(wrapped().await.is_ok());
    }

    #[tokio::test]
    async fn db_checker_returns_err_when_shutting_down() {
        let inner: DbCheckerFn = Arc::new(|| Box::pin(async { Ok(()) }));
        let flag = Arc::new(AtomicBool::new(true)); // already shutting down
        let wrapped = make_shutdown_db_checker(inner, flag);
        let result = wrapped().await;
        assert!(result.is_err(), "must return Err when is_shutting_down=true");
        assert!(
            result
                .unwrap_err()
                .contains("shutting down"),
            "error message must mention 'shutting down'"
        );
    }

    #[tokio::test]
    async fn db_checker_transitions_from_ok_to_err_on_flag_set() {
        let inner: DbCheckerFn = Arc::new(|| Box::pin(async { Ok(()) }));
        let flag = Arc::new(AtomicBool::new(false));
        let wrapped = make_shutdown_db_checker(inner, Arc::clone(&flag));

        // Before shutdown: probe passes.
        assert!(wrapped().await.is_ok(), "must pass before shutdown signal");

        // Set the flag (simulating SIGTERM receipt).
        flag.store(true, Ordering::SeqCst);

        // After shutdown: probe fails.
        assert!(wrapped().await.is_err(), "must fail after shutdown signal");
    }

    #[tokio::test]
    async fn db_checker_still_propagates_inner_error_when_not_shutting_down() {
        let inner: DbCheckerFn =
            Arc::new(|| Box::pin(async { Err("connection refused".to_string()) }));
        let flag = Arc::new(AtomicBool::new(false));
        let wrapped = make_shutdown_db_checker(inner, flag);
        let result = wrapped().await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "connection refused");
    }

    // ── run_shutdown_sequence (unit-level: drain + flush) ─────────────────────

    #[tokio::test]
    async fn shutdown_sequence_completes_with_no_active_connections() {
        use crate::connections::ConnectionTracker;
        use crate::logging::{LogLevel, RequestLogger};

        let tracker = ConnectionTracker::new(100);
        let logger = RequestLogger::new("test-instance".to_string(), LogLevel::Info);

        // Spawn a dummy task to use as sync_handle.
        let dummy_handle = tokio::spawn(async {
            // Runs until aborted.
            let () = futures_util::future::pending().await;
        });

        // Must complete quickly when there are no in-flight connections.
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            run_shutdown_sequence(tracker, logger, dummy_handle),
        )
        .await
        .expect("shutdown sequence must complete within 2 s when no connections active");
    }

    #[tokio::test]
    async fn shutdown_sequence_drains_connection_before_completing() {
        use crate::connections::ConnectionTracker;
        use crate::logging::{LogLevel, RequestLogger};
        use uuid::Uuid;

        let tracker = ConnectionTracker::new(100);
        let logger = RequestLogger::new("test-instance".to_string(), LogLevel::Info);
        let dummy_handle = tokio::spawn(async {
            let () = futures_util::future::pending().await;
        });

        let server_id = Uuid::new_v4();
        let guard = tracker.try_acquire(server_id, 50).unwrap();

        let tracker_clone = Arc::clone(&tracker);
        let logger_clone = Arc::clone(&logger);

        let seq_task = tokio::spawn(async move {
            run_shutdown_sequence(tracker_clone, logger_clone, dummy_handle).await;
        });

        // Give the sequence task a moment to start waiting on drain().
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        // Dropping the guard signals drain().
        drop(guard);

        tokio::time::timeout(
            std::time::Duration::from_millis(500),
            seq_task,
        )
        .await
        .expect("timeout waiting for shutdown sequence")
        .expect("shutdown task must not panic");
    }

    /// Integration-level test: spawn a minimal HTTP server, open 5 SSE
    /// connections, send SIGTERM to self, verify all connections complete and
    /// the server exits cleanly within 40 s.
    ///
    /// Ignored by default — requires a full runtime environment.
    #[tokio::test]
    #[ignore = "integration test: requires full gateway runtime; run manually"]
    async fn graceful_shutdown_with_concurrent_sse_connections() {
        // Placeholder: a full integration test would:
        // 1. Bind a real axum server with the transport router.
        // 2. Open 5 SSE clients concurrently.
        // 3. Send SIGTERM to the current process (or set is_shutting_down flag directly).
        // 4. Assert all 5 SSE streams complete within 40 s and the server stops accepting.
        todo!("implement full process-level integration test");
    }
}
