# Sidecar IPC Protocol

The gateway communicates with the credential injector sidecar over a Unix domain socket using a simple JSON-over-TCP framing protocol. The gateway never holds or inspects raw credential values; the sidecar injects them before forwarding the request upstream.

## Transport

- **Socket path**: configured via `INJECTOR_SOCKET_PATH` (default `/tmp/recast-injector.sock`)
- **Protocol**: JSON over Unix domain socket with 4-byte length-prefix framing
- **Connection model**: persistent connection pool (max 32 sockets); connections are reused across requests

## Framing

Every message (request and response) is framed as:

```
[u32 big-endian byte count (4 bytes)][UTF-8 JSON payload (N bytes)]
```

The receiver reads the 4-byte header, allocates a buffer of that size, then reads the payload. There is no terminator character.

## Request (gateway → sidecar)

```json
{
  "server_id": "550e8400-e29b-41d4-a716-446655440000",
  "request": {
    "method": "POST",
    "url": "https://api.stripe.com/v1/charges",
    "headers": {
      "Content-Type": "application/json",
      "User-Agent": "mcp-gateway/1.0"
    },
    "body": "<standard-base64-encoded-bytes>"
  }
}
```

| Field | Type | Description |
|---|---|---|
| `server_id` | UUID string | Identifies which server's credentials to inject |
| `request.method` | string | HTTP method in uppercase (`GET`, `POST`, etc.) |
| `request.url` | string | Full URL including query string (no auth query params) |
| `request.headers` | object | Request headers, excluding `Authorization` |
| `request.body` | string \| null | Standard Base64-encoded body bytes; `null` for bodyless requests |

**Security note**: The gateway never includes an `Authorization` header in the IPC request. The sidecar fetches credentials from the vault and injects them before the upstream call.

## Response (sidecar → gateway)

```json
{
  "status": 200,
  "headers": {
    "content-type": "application/json",
    "x-request-id": "req_abc123"
  },
  "body": "<standard-base64-encoded-bytes>",
  "latency_ms": 142
}
```

| Field | Type | Description |
|---|---|---|
| `status` | u16 | HTTP status code returned by the upstream |
| `headers` | object | Response headers from the upstream (lowercase names) |
| `body` | string | Standard Base64-encoded response body bytes |
| `latency_ms` | u64 | Total upstream latency measured by the sidecar (ms) |

## Status Code Handling (gateway side)

After receiving the IPC response, the gateway maps status codes to JSON-RPC errors:

| HTTP Status | JSON-RPC Error | Notes |
|---|---|---|
| 2xx | success | Body forwarded to transform engine |
| 4xx (non-429) | `-32003` `Upstream error` | Does **not** increment circuit breaker |
| 429 | `-32003` `Upstream error` | Increments circuit breaker; `Retry-After` forwarded |
| 5xx | `-32003` `Upstream error` | Increments circuit breaker |

## Timeout

The gateway applies a **35-second** safety-net timeout to the full IPC round-trip (send request frame + receive response frame). If the timeout fires, the connection is discarded from the pool and the gateway returns JSON-RPC error `-32002` with `message: "Upstream timeout"`.

The sidecar enforces its own upstream timeout (derived from `config_json.timeout_ms`, default 30 s) independently.

## Sidecar Unavailable

If the socket file does not exist, the connection is refused, or an I/O error occurs during the IPC exchange, the gateway returns JSON-RPC error:

```json
{
  "code": -32002,
  "message": "Upstream unavailable",
  "data": { "reason": "credential_service_unreachable" }
}
```

No hot-path retry is performed. The circuit breaker is incremented.

## auth_type = none

For servers configured with `auth_type: "none"`, the gateway bypasses the sidecar entirely and calls the upstream directly via an internal `reqwest::Client`. No Unix socket connection is opened.

## Circuit Breaker Integration

Before every IPC call, the gateway checks the per-server circuit breaker (`gateway::circuit_breaker`). If the circuit is open, a `-32004` error is returned immediately without contacting the sidecar. After the call, `on_success()` or `on_failure()` is invoked on the breaker based on the outcome.

## Connection Pool

| Property | Value |
|---|---|
| Max concurrent connections | 32 |
| Connection strategy | Lazy (created on first request) |
| Reuse | Healthy connections are returned to the idle pool after each call |
| Failure handling | Broken connections are discarded; a new one is opened on the next request |
| Backpressure | `tokio::sync::Semaphore(32)` — callers block when pool is at capacity |
