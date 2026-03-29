/// Atomic Redis Lua token-bucket script.
///
/// KEYS[1]  — the bucket key (e.g. `ratelimit:server:<uuid>`)
/// ARGV[1]  — rate_per_min  (integer, tokens added per minute)
/// ARGV[2]  — now_ms        (integer, current Unix time in milliseconds)
///
/// Returns a Redis array: `[allowed, remaining, reset_secs]`
/// - `allowed`     — 1 if the request is permitted, 0 if rate-limited.
/// - `remaining`   — integer tokens left after this request.
/// - `reset_secs`  — seconds until at least 1 token is available (0 if allowed).
pub(super) const LUA_TOKEN_BUCKET: &str = r#"
local key         = KEYS[1]
local rate_pm     = tonumber(ARGV[1])
local now_ms      = tonumber(ARGV[2])
local capacity    = rate_pm * 1.5
local rate_ps     = rate_pm / 60.0

local data        = redis.call('HMGET', key, 'tokens', 'last_ms')
local tokens      = tonumber(data[1])
local last_ms     = tonumber(data[2])

if tokens == nil then
    tokens  = rate_pm
    last_ms = now_ms
end

local elapsed_ms  = math.max(0, now_ms - last_ms)
local refill      = (elapsed_ms / 1000.0) * rate_ps
tokens            = math.min(capacity, tokens + refill)

local allowed     = 0
if tokens >= 1.0 then
    tokens  = tokens - 1.0
    allowed = 1
end

local remaining   = math.max(0, math.floor(tokens))
local ttl         = math.ceil(capacity / rate_ps) + 10
redis.call('HMSET', key, 'tokens', tostring(tokens), 'last_ms', tostring(now_ms))
redis.call('EXPIRE', key, ttl)

local reset_secs = 0
if allowed == 0 and rate_ps > 0 then
    reset_secs = math.ceil((1.0 - tokens) / rate_ps)
end

return {allowed, remaining, reset_secs}
"#;
