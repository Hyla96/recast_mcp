//! Per-server circuit breaker with Closed / Open / HalfOpen state machine.
//!
//! Each upstream API server has an independent [`CircuitBreaker`] keyed by `server_id`.
//! The [`CircuitBreakerRegistry`] lazily creates breakers on first access.
//!
//! State transitions:
//! - Closed  → Open      after [`FAILURE_THRESHOLD`] consecutive failures
//! - Open    → HalfOpen  lazily after [`OPEN_DURATION_SECS`] seconds (checked on next request)
//! - HalfOpen → Closed   after [`HALF_OPEN_SUCCESS_THRESHOLD`] consecutive probe successes
//! - HalfOpen → Open     on any failure (resets the 30s timer)
//!
//! Only HTTP 5xx, timeouts, connection errors, and HTTP 429 count as failures.
//! HTTP 4xx responses (except 429) do not affect the breaker state.
//!
//! When Open, callers receive [`CircuitError::Open`] without any upstream call being made.
//! When HalfOpen, at most one concurrent probe is allowed via [`AtomicBool`]; further
//! callers receive the same Open error.

use dashmap::DashMap;
use std::sync::{
    atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering},
    Arc,
};
use uuid::Uuid;

/// Number of consecutive failures required to open the circuit.
const FAILURE_THRESHOLD: u32 = 5;
/// Number of consecutive probe successes required to close from HalfOpen.
const HALF_OPEN_SUCCESS_THRESHOLD: u32 = 3;
/// Seconds to keep the circuit Open before transitioning to HalfOpen.
const OPEN_DURATION_SECS: i64 = 30;
/// Default retry-after value in milliseconds reported in the circuit error.
const DEFAULT_RETRY_AFTER_MS: u64 = 30_000;

// Numeric codes for atomic state storage.
const STATE_CLOSED: u32 = 0;
const STATE_OPEN: u32 = 1;
const STATE_HALF_OPEN: u32 = 2;

// ----------------------------------------------------------------------------
// Public types
// ----------------------------------------------------------------------------

/// Observable state of a circuit breaker (for Prometheus and logging).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakerState {
    /// Normal operation; all requests are forwarded upstream.
    Closed = 0,
    /// Fast-fail mode; upstream calls are blocked until the recovery window expires.
    Open = 1,
    /// Recovery probe mode; exactly one probe request is allowed through.
    HalfOpen = 2,
}

/// Error returned by [`CircuitBreaker::check`] when the circuit is open.
#[derive(Debug, thiserror::Error)]
pub enum CircuitError {
    /// Circuit is Open or HalfOpen with probe already in flight.
    #[error("upstream temporarily unavailable; retry after {retry_after_ms}ms")]
    Open {
        /// Milliseconds the caller should wait before retrying.
        retry_after_ms: u64,
    },
}

// ----------------------------------------------------------------------------
// CircuitBreaker
// ----------------------------------------------------------------------------

/// Single per-server circuit breaker.
///
/// All fields are atomic so the struct can be shared across threads without a
/// `Mutex`.  State transitions use `compare_exchange` to prevent races.
pub struct CircuitBreaker {
    server_id: Uuid,
    /// Atomic state: 0=Closed, 1=Open, 2=HalfOpen.
    state: AtomicU32,
    /// Consecutive upstream failures (meaningful in Closed state).
    consecutive_failures: AtomicU32,
    /// Consecutive probe successes (meaningful in HalfOpen state).
    consecutive_successes: AtomicU32,
    /// Unix timestamp (seconds) when the circuit last opened.
    open_since_secs: AtomicI64,
    /// True while exactly one probe request is in-flight in HalfOpen state.
    probe_in_progress: AtomicBool,
    /// Retry-after value (ms) reported in [`CircuitError::Open`].
    /// Updated from the upstream Retry-After header on HTTP 429 failures.
    retry_after_ms: AtomicU64,
}

impl CircuitBreaker {
    /// Create a new, closed circuit breaker for the given server.
    pub fn new(server_id: Uuid) -> Arc<Self> {
        Arc::new(Self {
            server_id,
            state: AtomicU32::new(STATE_CLOSED),
            consecutive_failures: AtomicU32::new(0),
            consecutive_successes: AtomicU32::new(0),
            open_since_secs: AtomicI64::new(0),
            probe_in_progress: AtomicBool::new(false),
            retry_after_ms: AtomicU64::new(DEFAULT_RETRY_AFTER_MS),
        })
    }

    /// Return the current observable state.
    pub fn state(&self) -> BreakerState {
        match self.state.load(Ordering::Acquire) {
            STATE_CLOSED => BreakerState::Closed,
            STATE_OPEN => BreakerState::Open,
            STATE_HALF_OPEN => BreakerState::HalfOpen,
            _ => BreakerState::Closed,
        }
    }

    /// Return the current consecutive failure count.
    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures.load(Ordering::Acquire)
    }

    // ------------------------------------------------------------------
    // Core API
    // ------------------------------------------------------------------

    /// Check whether the circuit allows this request to proceed.
    ///
    /// - **Closed**: returns `Ok(())` unconditionally.
    /// - **Open**: checks elapsed time. If ≥ 30 s, transitions to HalfOpen and
    ///   allows a single probe. Otherwise returns `Err(Open)`.
    /// - **HalfOpen**: allows at most one concurrent probe via `AtomicBool`.
    ///   Additional callers receive `Err(Open)`.
    pub fn check(&self) -> Result<(), CircuitError> {
        let state = self.state.load(Ordering::Acquire);
        match state {
            STATE_CLOSED => Ok(()),
            STATE_OPEN => {
                let open_since = self.open_since_secs.load(Ordering::Acquire);
                let elapsed = now_unix_secs().saturating_sub(open_since);
                if elapsed >= OPEN_DURATION_SECS {
                    // Attempt lazy Open → HalfOpen transition.
                    if self
                        .state
                        .compare_exchange(
                            STATE_OPEN,
                            STATE_HALF_OPEN,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                        .is_ok()
                    {
                        // Reset probe counters for the new HalfOpen window.
                        self.consecutive_successes.store(0, Ordering::Release);
                        self.probe_in_progress.store(false, Ordering::Release);
                        tracing::warn!(
                            server_id = %self.server_id,
                            event = "circuit_breaker_half_open",
                            "circuit breaker entering half-open state"
                        );
                        self.publish_metrics();
                        return self.try_acquire_probe();
                    }
                    // Another thread beat us to the transition; fall through to
                    // current state check.
                    let current = self.state.load(Ordering::Acquire);
                    if current == STATE_HALF_OPEN {
                        return self.try_acquire_probe();
                    }
                }
                Err(CircuitError::Open {
                    retry_after_ms: self.retry_after_ms.load(Ordering::Acquire),
                })
            }
            STATE_HALF_OPEN => self.try_acquire_probe(),
            _ => Ok(()),
        }
    }

    /// Record a successful upstream response.
    ///
    /// - **Closed**: resets the consecutive failure counter.
    /// - **HalfOpen**: increments the success counter; if ≥ threshold, transitions
    ///   to Closed; otherwise releases the probe lock for the next probe.
    /// - **Open**: no-op (probe was not granted).
    pub fn on_success(&self) {
        let state = self.state.load(Ordering::Acquire);
        match state {
            STATE_CLOSED => {
                self.consecutive_failures.store(0, Ordering::Release);
                self.publish_metrics();
            }
            STATE_HALF_OPEN => {
                let successes =
                    self.consecutive_successes.fetch_add(1, Ordering::AcqRel) + 1;
                if successes >= HALF_OPEN_SUCCESS_THRESHOLD {
                    if self
                        .state
                        .compare_exchange(
                            STATE_HALF_OPEN,
                            STATE_CLOSED,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                        .is_ok()
                    {
                        self.consecutive_failures.store(0, Ordering::Release);
                        self.probe_in_progress.store(false, Ordering::Release);
                        self.retry_after_ms
                            .store(DEFAULT_RETRY_AFTER_MS, Ordering::Release);
                        tracing::warn!(
                            server_id = %self.server_id,
                            event = "circuit_breaker_closed",
                            "circuit breaker closed after successful probes"
                        );
                        self.publish_metrics();
                    }
                } else {
                    // Release lock so the next probe can proceed.
                    self.probe_in_progress.store(false, Ordering::Release);
                }
            }
            _ => {}
        }
    }

    /// Record a failed upstream request.
    ///
    /// Pass `retry_after_ms = Some(ms)` when the upstream returned HTTP 429
    /// with a `Retry-After` header; this value is echoed back in
    /// [`CircuitError::Open`] when the circuit opens.
    ///
    /// - **Closed**: increments consecutive failures; if ≥ threshold, opens circuit.
    /// - **HalfOpen**: any failure immediately re-opens the circuit (resets timer).
    /// - **Open**: no-op.
    pub fn on_failure(&self, retry_after_ms: Option<u64>) {
        if let Some(ms) = retry_after_ms {
            self.retry_after_ms.store(ms, Ordering::Release);
        }

        let state = self.state.load(Ordering::Acquire);
        match state {
            STATE_CLOSED => {
                let failures =
                    self.consecutive_failures.fetch_add(1, Ordering::AcqRel) + 1;
                self.publish_metrics();
                if failures >= FAILURE_THRESHOLD
                    && self
                        .state
                        .compare_exchange(
                            STATE_CLOSED,
                            STATE_OPEN,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                        .is_ok()
                {
                    self.open_since_secs
                        .store(now_unix_secs(), Ordering::Release);
                    tracing::warn!(
                        server_id = %self.server_id,
                        event = "circuit_breaker_opened",
                        consecutive_failures = failures,
                        "circuit breaker opened"
                    );
                    self.publish_metrics();
                }
            }
            STATE_HALF_OPEN => {
                if self
                    .state
                    .compare_exchange(
                        STATE_HALF_OPEN,
                        STATE_OPEN,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    )
                    .is_ok()
                {
                    self.open_since_secs
                        .store(now_unix_secs(), Ordering::Release);
                    self.consecutive_failures.store(1, Ordering::Release);
                    self.probe_in_progress.store(false, Ordering::Release);
                    tracing::warn!(
                        server_id = %self.server_id,
                        event = "circuit_breaker_opened",
                        "circuit breaker re-opened from half-open after probe failure"
                    );
                    self.publish_metrics();
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------
    // Internal helpers
    // ------------------------------------------------------------------

    /// Attempt to acquire the single-probe slot in HalfOpen state.
    fn try_acquire_probe(&self) -> Result<(), CircuitError> {
        if self
            .probe_in_progress
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            Ok(())
        } else {
            Err(CircuitError::Open {
                retry_after_ms: self.retry_after_ms.load(Ordering::Acquire),
            })
        }
    }

    /// Publish current state and failure count to Prometheus.
    fn publish_metrics(&self) {
        let state_val = self.state.load(Ordering::Acquire) as f64;
        let failures = self.consecutive_failures.load(Ordering::Acquire) as f64;
        let id = self.server_id.to_string();
        metrics::gauge!("gateway_circuit_breaker_state", "server_id" => id.clone())
            .set(state_val);
        metrics::gauge!(
            "gateway_circuit_breaker_consecutive_failures",
            "server_id" => id
        )
        .set(failures);
    }

    // ------------------------------------------------------------------
    // Test helpers (cfg(test) only)
    // ------------------------------------------------------------------

    /// Directly set the state for unit tests.
    #[cfg(test)]
    pub(crate) fn force_state(&self, s: BreakerState) {
        self.state.store(s as u32, Ordering::Release);
    }

    /// Set `open_since_secs` to `now - secs_ago` to simulate elapsed time.
    #[cfg(test)]
    pub(crate) fn set_open_since_secs_ago(&self, secs_ago: i64) {
        self.open_since_secs
            .store(now_unix_secs() - secs_ago, Ordering::Release);
    }

    /// Clear the probe-in-progress flag for test setup.
    #[cfg(test)]
    pub(crate) fn clear_probe_flag(&self) {
        self.probe_in_progress.store(false, Ordering::Release);
    }
}

// ----------------------------------------------------------------------------
// CircuitBreakerRegistry
// ----------------------------------------------------------------------------

/// Registry of per-server circuit breakers, keyed by `server_id`.
///
/// Breakers are created lazily on first access and remain in the map until
/// explicitly removed (e.g. when a server is deleted from the config cache).
#[derive(Default)]
pub struct CircuitBreakerRegistry {
    breakers: DashMap<Uuid, Arc<CircuitBreaker>>,
}

impl CircuitBreakerRegistry {
    /// Create a new, empty registry.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            breakers: DashMap::new(),
        })
    }

    /// Return (or lazily create) the [`CircuitBreaker`] for `server_id`.
    pub fn get(&self, server_id: Uuid) -> Arc<CircuitBreaker> {
        self.breakers
            .entry(server_id)
            .or_insert_with(|| CircuitBreaker::new(server_id))
            .clone()
    }

    /// Remove the circuit breaker for `server_id` (call when server is deleted).
    pub fn remove(&self, server_id: Uuid) {
        self.breakers.remove(&server_id);
    }

    /// Return the number of registered breakers.
    pub fn len(&self) -> usize {
        self.breakers.len()
    }

    /// Return true if the registry has no entries.
    pub fn is_empty(&self) -> bool {
        self.breakers.is_empty()
    }
}

// ----------------------------------------------------------------------------
// Internal utilities
// ----------------------------------------------------------------------------

fn now_unix_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

// ----------------------------------------------------------------------------
// Unit tests
// ----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    fn make_breaker() -> Arc<CircuitBreaker> {
        CircuitBreaker::new(Uuid::new_v4())
    }

    // ------------------------------------------------------------------
    // Closed state
    // ------------------------------------------------------------------

    #[test]
    fn starts_closed() {
        let cb = make_breaker();
        assert_eq!(cb.state(), BreakerState::Closed);
        assert!(cb.check().is_ok());
    }

    #[test]
    fn four_failures_stays_closed() {
        let cb = make_breaker();
        for _ in 0..4 {
            cb.on_failure(None);
        }
        assert_eq!(cb.state(), BreakerState::Closed);
        assert!(cb.check().is_ok());
        assert_eq!(cb.consecutive_failures(), 4);
    }

    #[test]
    fn five_failures_opens_circuit() {
        let cb = make_breaker();
        for _ in 0..5 {
            cb.on_failure(None);
        }
        assert_eq!(cb.state(), BreakerState::Open);
        assert!(matches!(cb.check(), Err(CircuitError::Open { .. })));
    }

    #[test]
    fn success_resets_failure_counter_in_closed() {
        let cb = make_breaker();
        for _ in 0..4 {
            cb.on_failure(None);
        }
        cb.on_success();
        assert_eq!(cb.consecutive_failures(), 0);
        assert_eq!(cb.state(), BreakerState::Closed);
    }

    // ------------------------------------------------------------------
    // Open state
    // ------------------------------------------------------------------

    #[test]
    fn open_returns_error_before_30s() {
        let cb = make_breaker();
        for _ in 0..5 {
            cb.on_failure(None);
        }
        assert_eq!(cb.state(), BreakerState::Open);
        // open_since is "now", so elapsed < 30s
        let result = cb.check();
        assert!(
            matches!(result, Err(CircuitError::Open { retry_after_ms: 30_000 })),
            "expected Open error: {result:?}"
        );
    }

    #[test]
    fn open_transitions_to_half_open_after_30s() {
        let cb = make_breaker();
        for _ in 0..5 {
            cb.on_failure(None);
        }
        // Pretend the circuit opened 31 seconds ago.
        cb.set_open_since_secs_ago(31);
        assert!(
            cb.check().is_ok(),
            "probe should be allowed after 30s"
        );
        assert_eq!(cb.state(), BreakerState::HalfOpen);
    }

    #[test]
    fn http_429_stores_retry_after_in_error() {
        let cb = make_breaker();
        // 5 failures with last one being 429 with Retry-After: 60s
        for _ in 0..4 {
            cb.on_failure(None);
        }
        cb.on_failure(Some(60_000));
        assert_eq!(cb.state(), BreakerState::Open);
        match cb.check() {
            Err(CircuitError::Open { retry_after_ms }) => {
                assert_eq!(retry_after_ms, 60_000);
            }
            other => panic!("expected Open error, got {other:?}"),
        }
    }

    // ------------------------------------------------------------------
    // HalfOpen state
    // ------------------------------------------------------------------

    #[test]
    fn half_open_allows_single_probe() {
        let cb = make_breaker();
        cb.force_state(BreakerState::HalfOpen);
        cb.clear_probe_flag();
        assert!(cb.check().is_ok(), "first probe should succeed");
    }

    #[test]
    fn half_open_second_request_gets_open_error() {
        let cb = make_breaker();
        cb.force_state(BreakerState::HalfOpen);
        cb.clear_probe_flag();
        assert!(cb.check().is_ok());
        // probe_in_progress is now true; second caller should be rejected
        let result = cb.check();
        assert!(
            matches!(result, Err(CircuitError::Open { .. })),
            "concurrent probe should be rejected: {result:?}"
        );
    }

    #[test]
    fn half_open_failure_reopens_circuit() {
        let cb = make_breaker();
        cb.force_state(BreakerState::HalfOpen);
        cb.clear_probe_flag();
        cb.check().expect("probe should be allowed");
        cb.on_failure(None);
        assert_eq!(cb.state(), BreakerState::Open);
    }

    #[test]
    fn half_open_three_successes_closes_circuit() {
        let cb = make_breaker();
        cb.force_state(BreakerState::HalfOpen);
        cb.clear_probe_flag();

        // First probe
        cb.check().expect("probe 1 should be allowed");
        cb.on_success();
        assert_eq!(cb.state(), BreakerState::HalfOpen);

        // Second probe
        cb.check().expect("probe 2 should be allowed");
        cb.on_success();
        assert_eq!(cb.state(), BreakerState::HalfOpen);

        // Third probe → closes
        cb.check().expect("probe 3 should be allowed");
        cb.on_success();
        assert_eq!(cb.state(), BreakerState::Closed);
        assert!(cb.check().is_ok());
    }

    // ------------------------------------------------------------------
    // Registry
    // ------------------------------------------------------------------

    #[test]
    fn registry_creates_breakers_lazily() {
        let reg = CircuitBreakerRegistry::new();
        assert_eq!(reg.len(), 0);
        let id = Uuid::new_v4();
        let cb = reg.get(id);
        assert_eq!(cb.state(), BreakerState::Closed);
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn registry_returns_same_instance() {
        let reg = CircuitBreakerRegistry::new();
        let id = Uuid::new_v4();
        let cb1 = reg.get(id);
        let cb2 = reg.get(id);
        // Both should point to the same allocation.
        assert!(Arc::ptr_eq(&cb1, &cb2));
    }

    #[test]
    fn registry_remove_evicts_entry() {
        let reg = CircuitBreakerRegistry::new();
        let id = Uuid::new_v4();
        reg.get(id);
        assert_eq!(reg.len(), 1);
        reg.remove(id);
        assert_eq!(reg.len(), 0);
    }
}
