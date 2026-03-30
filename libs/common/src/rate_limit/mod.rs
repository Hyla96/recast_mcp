//! Token-bucket rate limiter backed by Redis with in-process fallback.
//!
//! # Architecture
//!
//! [`RateLimiter`] tries Redis first for every check. If the Redis pool is absent
//! or the call fails, it transparently falls back to an in-process
//! `Mutex<HashMap>` bucket. A warning is emitted at most once per minute when
//! the fallback is active so operators are notified without flooding logs.
//!
//! # Token bucket parameters
//!
//! - **Initial tokens**: `rate_per_min` (starts at the base rate, not the capacity).
//! - **Capacity**: `rate_per_min × 1.5` (burst headroom accumulated during quiet periods).
//! - **Refill rate**: `rate_per_min / 60` tokens per second.
//!
//! This means a fresh bucket allows `rate_per_min` immediate requests, then
//! throttles. After a quiet period the bucket fills to `capacity` allowing a
//! burst of up to `rate_per_min × 1.5` before throttling again.
//!
//! # Axum integration
//!
//! Use [`rate_limit_middleware`] with `axum::middleware::from_fn_with_state`:
//!
//! ```rust,no_run
//! # use std::sync::Arc;
//! # use mcp_common::rate_limit::{RateLimiter, RateLimitConfig, RateLimitContext, rate_limit_middleware};
//! # use axum::{Router, middleware::from_fn_with_state};
//! let config = Arc::new(RateLimitConfig {
//!     limiter: RateLimiter::new_in_process(),
//!     per_server_rate: 100,
//!     per_user_rate: 1000,
//!     enabled: true,
//!     audit_logger: None,
//! });
//! let router: Router = Router::new()
//!     .layer(from_fn_with_state(config, rate_limit_middleware));
//! ```
//!
//! Set a [`RateLimitContext`] extension on the request (via a preceding middleware)
//! so the rate limiter knows which server/user bucket to check.

mod lua;
use lua::LUA_TOKEN_BUCKET;

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{Request, State},
    http::header,
    middleware::Next,
    response::{IntoResponse, Response},
};
// Re-export redis through deadpool_redis to avoid version conflicts.
use deadpool_redis::redis as redis_crate;
use sha1::{Digest, Sha1};
use tracing::{debug, warn};
use uuid::Uuid;

use crate::{AppError, AuditAction, AuditEvent, AuditLogger, SanitizedErrorMsg};

// ── In-process fallback ───────────────────────────────────────────────────────

struct InProcessBucket {
    tokens: f64,
    last_check: Instant,
    capacity: f64,
    rate_per_sec: f64,
}

// ── RateLimiterInner ──────────────────────────────────────────────────────────

struct RateLimiterInner {
    redis_pool: Option<deadpool_redis::Pool>,
    /// SHA-1 hex of [`LUA_TOKEN_BUCKET`] for EVALSHA.
    script_sha: String,
    in_process: Mutex<HashMap<String, InProcessBucket>>,
    last_redis_warn: Mutex<Option<Instant>>,
    warn_interval: Duration,
}

// ── RateLimiter ───────────────────────────────────────────────────────────────

/// Token-bucket rate limiter with Redis backend and in-process fallback.
///
/// Cheaply cloneable — all clones share the same underlying state.
#[derive(Clone)]
pub struct RateLimiter(Arc<RateLimiterInner>);

impl RateLimiter {
    /// Create a rate limiter backed by a Redis connection pool.
    ///
    /// Falls back to in-process tracking automatically if Redis is unavailable.
    pub fn new_with_redis(pool: deadpool_redis::Pool) -> Self {
        let sha = Self::sha1_hex(LUA_TOKEN_BUCKET);
        Self(Arc::new(RateLimiterInner {
            redis_pool: Some(pool),
            script_sha: sha,
            in_process: Mutex::new(HashMap::new()),
            last_redis_warn: Mutex::new(None),
            warn_interval: Duration::from_secs(60),
        }))
    }

    /// Create a rate limiter that uses only in-process token buckets (no Redis).
    ///
    /// Suitable for tests and for production when Redis is not configured.
    pub fn new_in_process() -> Self {
        let sha = Self::sha1_hex(LUA_TOKEN_BUCKET);
        Self(Arc::new(RateLimiterInner {
            redis_pool: None,
            script_sha: sha,
            in_process: Mutex::new(HashMap::new()),
            last_redis_warn: Mutex::new(None),
            warn_interval: Duration::from_secs(60),
        }))
    }

    /// Check whether a request is permitted under the given key and rate.
    ///
    /// `rate_per_min` is the number of requests allowed per minute.
    /// The bucket capacity is `rate_per_min × 1.5`.
    pub async fn check(&self, key: &str, rate_per_min: u32) -> RateLimitResult {
        if let Some(result) = self.try_redis(key, rate_per_min).await {
            return result;
        }
        self.check_in_process(key, rate_per_min)
    }

    // ── Redis path ────────────────────────────────────────────────────────────

    async fn try_redis(&self, key: &str, rate_per_min: u32) -> Option<RateLimitResult> {
        let pool = self.0.redis_pool.as_ref()?;

        let mut conn = match pool.get().await {
            Ok(c) => c,
            Err(e) => {
                self.maybe_warn(&e.to_string());
                return None;
            }
        };

        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        // Try EVALSHA first; fall back to EVAL on NOSCRIPT.
        let evalsha_result: Result<Vec<i64>, redis_crate::RedisError> =
            redis_crate::cmd("EVALSHA")
                .arg(&self.0.script_sha)
                .arg(1u32)
                .arg(key)
                .arg(rate_per_min as i64)
                .arg(now_ms)
                .query_async(&mut *conn)
                .await;

        let raw: Vec<i64> = match evalsha_result {
            Ok(v) => v,
            Err(ref e) if e.kind() == redis_crate::ErrorKind::NoScriptError => {
                match redis_crate::cmd("EVAL")
                    .arg(LUA_TOKEN_BUCKET)
                    .arg(1u32)
                    .arg(key)
                    .arg(rate_per_min as i64)
                    .arg(now_ms)
                    .query_async::<_, Vec<i64>>(&mut *conn)
                    .await
                {
                    Ok(v) => v,
                    Err(e) => {
                        self.maybe_warn(&e.to_string());
                        return None;
                    }
                }
            }
            Err(e) => {
                self.maybe_warn(&e.to_string());
                return None;
            }
        };

        if raw.len() < 3 {
            self.maybe_warn("unexpected Lua script return length");
            return None;
        }

        let allowed = raw[0] == 1;
        let remaining = u32::try_from(raw[1]).unwrap_or(0);
        let reset_secs = u64::try_from(raw[2]).unwrap_or(1);

        Some(RateLimitResult {
            allowed,
            remaining,
            limit: rate_per_min,
            reset_secs,
        })
    }

    // ── In-process path ───────────────────────────────────────────────────────

    fn check_in_process(&self, key: &str, rate_per_min: u32) -> RateLimitResult {
        let capacity = rate_per_min as f64 * 1.5;
        let rate_per_sec = rate_per_min as f64 / 60.0;

        let mut map = self.0.in_process.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();

        let bucket = map.entry(key.to_string()).or_insert_with(|| InProcessBucket {
            tokens: rate_per_min as f64,
            last_check: now,
            capacity,
            rate_per_sec,
        });

        // Refill based on elapsed time since last check.
        let elapsed = now.duration_since(bucket.last_check).as_secs_f64();
        bucket.tokens = f64::min(bucket.capacity, bucket.tokens + elapsed * bucket.rate_per_sec);
        bucket.last_check = now;

        // Consume one token.
        let allowed = bucket.tokens >= 1.0;
        if allowed {
            bucket.tokens -= 1.0;
        }

        let remaining = bucket.tokens.floor().max(0.0) as u32;
        let reset_secs = if allowed || rate_per_sec <= 0.0 {
            0u64
        } else {
            ((1.0 - bucket.tokens) / rate_per_sec).ceil() as u64
        };

        RateLimitResult { allowed, remaining, limit: rate_per_min, reset_secs }
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn maybe_warn(&self, reason: &str) {
        let mut last = self.0.last_redis_warn.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let should_warn = last
            .map(|t| now.duration_since(t) >= self.0.warn_interval)
            .unwrap_or(true);
        if should_warn {
            *last = Some(now);
            warn!(
                reason = %reason,
                "Redis rate limiter unavailable, using in-process fallback"
            );
        }
    }

    fn sha1_hex(input: &str) -> String {
        let mut h = Sha1::new();
        h.update(input.as_bytes());
        hex::encode(h.finalize())
    }
}

// ── RateLimitResult ───────────────────────────────────────────────────────────

/// Outcome of a single rate-limit check.
#[derive(Debug, Clone)]
pub struct RateLimitResult {
    /// `true` if the request is permitted.
    pub allowed: bool,
    /// Tokens remaining in the bucket after this request.
    pub remaining: u32,
    /// Configured rate limit (requests per minute).
    pub limit: u32,
    /// Seconds until at least one token is available again (0 if `allowed`).
    pub reset_secs: u64,
}

// ── RateLimitContext ──────────────────────────────────────────────────────────

/// Request-scoped context injected by a preceding middleware so the rate-limit
/// middleware knows which server/user bucket to check.
#[derive(Clone, Debug)]
pub struct RateLimitContext {
    /// The MCP server being accessed (drives per-server bucket).
    pub server_id: Option<Uuid>,
    /// The authenticated user making the request (drives per-user bucket).
    pub user_id: Option<Uuid>,
}

// ── RateLimitConfig ───────────────────────────────────────────────────────────

/// Configuration for [`rate_limit_middleware`].
#[derive(Clone)]
pub struct RateLimitConfig {
    /// The shared rate limiter instance.
    pub limiter: RateLimiter,
    /// Maximum requests per minute per MCP server.
    pub per_server_rate: u32,
    /// Maximum requests per minute per user (across all servers).
    pub per_user_rate: u32,
    /// When `false`, the middleware is a no-op and no rate-limit headers are
    /// added. Controlled by the `FEATURE_RATE_LIMIT_ENABLED` env var.
    pub enabled: bool,
    /// Optional audit logger — when `Some`, a `RateLimitExceeded` event is
    /// written whenever a request is blocked.
    pub audit_logger: Option<AuditLogger>,
}

// ── Axum middleware ───────────────────────────────────────────────────────────

/// Axum `from_fn_with_state` middleware that enforces token-bucket rate limits.
///
/// On every response it adds:
/// - `X-RateLimit-Limit`     — configured rate (requests/minute)
/// - `X-RateLimit-Remaining` — tokens left in the bucket
/// - `X-RateLimit-Reset`     — Unix timestamp when the bucket next has a token
///
/// On a rate-limited response (HTTP 429) it additionally adds:
/// - `Retry-After` — seconds to wait before retrying
///
/// If [`RateLimitConfig::enabled`] is `false`, the middleware is transparent.
pub async fn rate_limit_middleware(
    State(config): State<Arc<RateLimitConfig>>,
    req: Request,
    next: Next,
) -> Response {
    if !config.enabled {
        return next.run(req).await;
    }

    let context = req.extensions().get::<RateLimitContext>().cloned();

    let (server_id, user_id) = match context {
        Some(ctx) => (ctx.server_id, ctx.user_id),
        None => {
            debug!("rate_limit_middleware: no RateLimitContext in extensions, skipping");
            return next.run(req).await;
        }
    };

    let now_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // Check per-server limit.
    if let Some(sid) = server_id {
        let key = format!("ratelimit:server:{sid}");
        let result = config.limiter.check(&key, config.per_server_rate).await;

        if !result.allowed {
            if let Some(ref logger) = config.audit_logger {
                logger.log(AuditEvent {
                    action: AuditAction::RateLimitExceeded,
                    user_id,
                    server_id: Some(sid),
                    success: false,
                    error_msg: Some(SanitizedErrorMsg::new(format!(
                        "per-server limit of {} req/min exceeded",
                        config.per_server_rate
                    ))),
                    metadata: None,
                    correlation_id: None,
                });
            }
            return build_429(&result, now_unix);
        }

        let mut resp = next.run(req).await;
        append_rate_limit_headers(resp.headers_mut(), &result, now_unix);
        return resp;
    }

    // Check per-user limit.
    if let Some(uid) = user_id {
        let key = format!("ratelimit:user:{uid}");
        let result = config.limiter.check(&key, config.per_user_rate).await;

        if !result.allowed {
            if let Some(ref logger) = config.audit_logger {
                logger.log(AuditEvent {
                    action: AuditAction::RateLimitExceeded,
                    user_id: Some(uid),
                    server_id,
                    success: false,
                    error_msg: Some(SanitizedErrorMsg::new(format!(
                        "per-user limit of {} req/min exceeded",
                        config.per_user_rate
                    ))),
                    metadata: None,
                    correlation_id: None,
                });
            }
            return build_429(&result, now_unix);
        }

        let mut resp = next.run(req).await;
        append_rate_limit_headers(resp.headers_mut(), &result, now_unix);
        return resp;
    }

    // Neither server nor user id — pass through.
    next.run(req).await
}

fn append_rate_limit_headers(
    headers: &mut axum::http::HeaderMap,
    result: &RateLimitResult,
    now_unix: u64,
) {
    let reset_at = now_unix + result.reset_secs;

    if let Ok(v) = result.limit.to_string().parse() {
        headers.insert("x-ratelimit-limit", v);
    }
    if let Ok(v) = result.remaining.to_string().parse() {
        headers.insert("x-ratelimit-remaining", v);
    }
    if let Ok(v) = reset_at.to_string().parse() {
        headers.insert("x-ratelimit-reset", v);
    }
}

fn build_429(result: &RateLimitResult, now_unix: u64) -> Response {
    let retry_after = result.reset_secs.max(1);
    let err = AppError::RateLimited { retry_after_secs: retry_after };
    let mut resp = err.into_response();
    let headers = resp.headers_mut();

    // Standard headers.
    append_rate_limit_headers(headers, result, now_unix);
    if let Ok(v) = retry_after.to_string().parse::<header::HeaderValue>() {
        headers.insert(header::RETRY_AFTER, v);
    }

    resp
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

    use super::*;

    /// 100 sequential requests on a 100/min bucket → all allowed.
    #[tokio::test]
    async fn in_process_allows_up_to_rate() {
        let limiter = RateLimiter::new_in_process();
        let mut allowed = 0u32;
        for _ in 0..100 {
            if limiter.check("test:server:a", 100).await.allowed {
                allowed += 1;
            }
        }
        assert_eq!(allowed, 100);
    }

    /// 101st request on a 100/min bucket → rejected.
    #[tokio::test]
    async fn in_process_101st_request_rejected() {
        let limiter = RateLimiter::new_in_process();
        let mut allowed = 0u32;
        for _ in 0..151 {
            if limiter.check("test:server:b", 100).await.allowed {
                allowed += 1;
            }
        }
        assert_eq!(allowed, 100, "exactly 100 of 151 requests allowed");
    }

    /// Different keys are isolated — exhausting key A does not affect key B.
    #[tokio::test]
    async fn in_process_keys_are_isolated() {
        let limiter = RateLimiter::new_in_process();
        // Drain key_a
        for _ in 0..100 {
            limiter.check("key:a", 100).await;
        }
        // key_b is fresh
        let result_b = limiter.check("key:b", 100).await;
        assert!(result_b.allowed, "key_b should still be allowed");
    }

    /// 1001 sequential calls on a 1000/min bucket → exactly 1000 allowed.
    #[tokio::test]
    async fn in_process_1000_per_min_bucket() {
        let limiter = RateLimiter::new_in_process();
        let mut allowed = 0u32;
        for _ in 0..1001 {
            if limiter.check("test:user:uid1", 1000).await.allowed {
                allowed += 1;
            }
        }
        assert_eq!(allowed, 1000, "exactly 1000 of 1001 user requests allowed");
    }

    /// RateLimitResult carries headers data with correct field values.
    #[tokio::test]
    async fn rate_limit_result_fields() {
        let limiter = RateLimiter::new_in_process();
        let result = limiter.check("test:fields", 100).await;
        assert!(result.allowed);
        assert_eq!(result.limit, 100);
        // Initial tokens = 100. After 1 consume → 99 remaining.
        assert_eq!(result.remaining, 99);
    }

    /// After exhaustion, `reset_secs` is non-zero.
    #[tokio::test]
    async fn in_process_reset_secs_nonzero_when_exhausted() {
        let limiter = RateLimiter::new_in_process();
        let key = "test:reset";
        for _ in 0..100 {
            limiter.check(key, 100).await;
        }
        let result = limiter.check(key, 100).await;
        assert!(!result.allowed);
        assert!(result.reset_secs > 0, "reset_secs must be > 0 when bucket is exhausted");
    }

    /// 100 concurrent tasks on a 100-token bucket → exactly 100 allowed.
    ///
    /// Verifies that the Mutex correctly serializes concurrent access and
    /// no over-allowance occurs.
    #[tokio::test]
    async fn concurrent_no_over_allowance() {
        let limiter = Arc::new(RateLimiter::new_in_process());
        let key = "test:concurrent";
        let rate = 100u32;

        let handles: Vec<_> = (0..150)
            .map(|_| {
                let lim = Arc::clone(&limiter);
                tokio::spawn(async move { lim.check(key, rate).await })
            })
            .collect();

        let mut allowed = 0u32;
        for h in handles {
            if h.await.expect("task panicked").allowed {
                allowed += 1;
            }
        }

        assert_eq!(allowed, 100, "exactly 100 of 150 concurrent requests allowed");
    }

    /// `build_429` produces HTTP 429 with rate-limit headers.
    #[tokio::test]
    async fn build_429_has_correct_status_and_headers() {
        let result = RateLimitResult { allowed: false, remaining: 0, limit: 100, reset_secs: 5 };
        let now = 1_000_000u64;
        let resp = build_429(&result, now);
        assert_eq!(resp.status().as_u16(), 429);
        assert!(resp.headers().get("retry-after").is_some());
        assert!(resp.headers().get("x-ratelimit-limit").is_some());
        assert!(resp.headers().get("x-ratelimit-remaining").is_some());
        assert!(resp.headers().get("x-ratelimit-reset").is_some());
    }
}
