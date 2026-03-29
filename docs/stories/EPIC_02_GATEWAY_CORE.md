# EPIC 02: Gateway Core

**Product:** Dynamic MCP Server Builder
**Epic ID:** E-02
**Status:** Ready for Sprint Planning
**Date:** 2026-03-28
**Author:** Engineering (multi-agent synthesis)

---

## Epic Summary

Implement the runtime gateway: the axum/tokio process that accepts MCP client connections, routes by server slug, proxies upstream HTTP requests through the credential injector sidecar, applies declarative response transforms, and returns MCP-compliant responses. This epic covers the full request path from TCP accept to MCP response, including the in-memory config cache, hot reload, circuit breaking, backpressure, graceful shutdown, and multi-instance coordination.

## Scope Boundaries

**In scope:** JSON-RPC parsing, both MCP transports, config-driven routing, in-memory cache, hot reload, upstream HTTP client, credential injection protocol, transform engine, tool schema generation, per-server auth, structured logging, circuit breaker, connection limits, graceful shutdown, multi-instance design.

**Out of scope (future epics):** OAuth2 flows, rate limiting by plan tier, billing metering, admin API, web UI, OpenAPI import, multi-step tool chains.

## Dependencies on Other Epics

- E-01 (Data Layer): PostgreSQL schema for `mcp_servers`, `credentials`, `audit_log` tables and the NOTIFY triggers must be complete before S-024 and S-025 can be closed.
- Credential injector sidecar (separate service): Unix domain socket protocol must be agreed before S-027.

---

## Stories

---

### S-020: MCP Protocol — JSON-RPC 2.0 Message Parser

**Priority:** P0
**Estimate:** 3 points

#### Description

Parse and validate all incoming JSON-RPC 2.0 messages before they reach any handler. This is the gateway's first line of defence. Every message that arrives on either transport (Streamable HTTP or SSE) passes through this layer. The parser must be allocation-efficient because it runs on every request at potentially millions of RPS.

#### Acceptance Criteria

1. Parse single JSON-RPC 2.0 request objects with fields `jsonrpc` (must equal `"2.0"`), `id` (string, number, or null), `method` (non-empty string), and `params` (optional object or array).
2. Parse JSON-RPC 2.0 batch requests (top-level JSON array of request objects). Return a batch response array preserving the original ordering.
3. Parse JSON-RPC 2.0 notifications (objects where `id` is absent or null). Notifications must never produce a response message.
4. Return `{"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error"}}` for any input that is not valid JSON.
5. Return `{"jsonrpc":"2.0","id":<id>,"error":{"code":-32600,"message":"Invalid Request"}}` for JSON that is valid but violates JSON-RPC 2.0 structure (missing `jsonrpc` field, wrong version string, missing `method`).
6. Return `{"jsonrpc":"2.0","id":<id>,"error":{"code":-32601,"message":"Method not found"}}` for unrecognised methods. Recognised methods: `initialize`, `initialized`, `tools/list`, `tools/call`, `ping`.
7. Enforce a maximum input size of 512 KB. Return `-32600 Invalid Request` with an `extensions` field `{"reason":"payload_too_large"}` for oversized bodies.
8. The `params` field is preserved as a raw `serde_json::Value` and passed to the relevant handler without re-serialisation.
9. All parsing logic is covered by unit tests using table-driven test vectors including: valid request, valid notification, valid batch (mixed requests and notifications), batch with one invalid item (returns partial error), parse error, invalid request, method not found, oversized payload.
10. Parsing a 1 KB single-request body completes in under 10 µs on the CI runner (verified with `criterion` benchmark).

#### Technical Notes

- Use `serde_json` for deserialisation into a `JsonRpcMessage` enum: `Request`, `Notification`, `BatchRequest`.
- Define a shared `JsonRpcError` struct with `code: i64`, `message: String`, `data: Option<Value>`.
- The parser module is `gateway::protocol::jsonrpc`. It is transport-agnostic; both axum handlers call into it.
- For batch requests, spawn a `FuturesUnordered` of handler futures and collect results in ID order. Do not process batch items sequentially.
- Do not allocate for the `jsonrpc` version string check — compare bytes directly.

#### Dependencies

None. This is a leaf module.

---

### S-021: MCP Protocol — Streamable HTTP Transport

**Priority:** P0
**Estimate:** 5 points

#### Description

Implement the primary MCP transport: a single HTTP endpoint that accepts JSON-RPC POST bodies and returns JSON-RPC responses. When the client sends an `Accept: text/event-stream` header, the endpoint upgrades to an SSE stream for server-initiated notifications. This is the spec-mandated primary transport for MCP 2025-03-26 and later.

#### Acceptance Criteria

1. Mount the endpoint at `POST /mcp/{slug}`. The `{slug}` path parameter identifies the MCP server being accessed.
2. Accept `Content-Type: application/json` request bodies. Return HTTP 415 for other content types.
3. For non-streaming clients (no `Accept: text/event-stream`): parse the JSON-RPC body using S-020, dispatch to handler, return `Content-Type: application/json` response with the JSON-RPC result or error. HTTP status is always 200 for valid JSON-RPC (errors are in the JSON-RPC envelope, not HTTP).
4. For streaming clients (`Accept: text/event-stream`): open an SSE stream. Send the immediate JSON-RPC response as the first SSE event (`data: <json>\n\n`). Keep the stream alive for server-initiated notifications (progress, log messages) for up to 60 seconds. Close the stream when the response is complete and no notifications are pending.
5. Support the `initialize` handshake. Respond with `{"jsonrpc":"2.0","id":<id>,"result":{"protocolVersion":"2025-03-26","capabilities":{"tools":{}},"serverInfo":{"name":"<server_name>","version":"1.0.0"}}}`.
6. Respond to `ping` method with `{"jsonrpc":"2.0","id":<id>,"result":{}}`.
7. Set `Access-Control-Allow-Origin: *` and handle CORS preflight (`OPTIONS`) requests with appropriate `Allow` headers.
8. Set `Cache-Control: no-store` on all responses.
9. Return HTTP 404 (wrapped in a JSON-RPC error envelope) when the `slug` does not match any active server in the config cache.
10. Return HTTP 401 (wrapped in a JSON-RPC error envelope) when the Bearer token is missing or invalid (delegates to S-030).
11. Integration test: a complete `initialize` → `tools/list` → `tools/call` flow using a test server config and a mock upstream. Verify all three responses are spec-compliant JSON-RPC.
12. Integration test: streaming client receives SSE events. Verify Content-Type is `text/event-stream; charset=utf-8` and each event is correctly framed.

#### Technical Notes

- Use `axum::response::sse::Sse` for the streaming path. Wrap the SSE channel in a `tokio::sync::mpsc` so the handler can push notifications while the response is in flight.
- The non-streaming path is a standard axum `Json` extractor and response — keep it on the fast path with no channel overhead.
- `Accept` header matching should check for `text/event-stream` as a substring to handle quality values (`text/event-stream;q=0.9`).
- Server-sent events must use the `data:` field only. Do not use SSE `event:` or `id:` fields in MVP; this simplifies client compatibility.
- Graceful close: when the axum handler returns, the SSE body is dropped, which closes the HTTP/1.1 chunked transfer or HTTP/2 DATA stream. Verify with `curl --no-buffer` in CI smoke test.

#### Dependencies

- S-020 (JSON-RPC parser)
- S-023 (gateway router, needed for slug dispatch)
- S-030 (Bearer token auth, needed for 401 path)

---

### S-022: MCP Protocol — SSE Transport (Fallback)

**Priority:** P1
**Estimate:** 5 points

#### Description

Implement the legacy MCP SSE transport for backwards compatibility with Claude Desktop versions prior to Streamable HTTP support and any other clients that have not yet migrated. The SSE transport uses two separate endpoints: a long-lived GET endpoint for server-to-client messages and a separate POST endpoint for client-to-server messages.

#### Acceptance Criteria

1. Mount `GET /sse/{slug}` as the long-lived SSE endpoint. The server sends a `endpoint` event on connect: `event: endpoint\ndata: /messages/{slug}?session_id=<uuid>\n\n`. This tells the client where to POST messages.
2. Mount `POST /messages/{slug}` as the client-to-server message endpoint. Requires `session_id` query parameter matching an active SSE session. Returns HTTP 202 Accepted immediately (the response is delivered over the SSE stream).
3. Session lifecycle: a session is created when the GET `/sse/{slug}` connection opens. The session stores a `tokio::sync::mpsc::Sender` used to push JSON-RPC responses back over the SSE stream. The session is destroyed when the SSE connection closes (client disconnect or server timeout).
4. Session timeout: if no message is received on a session within 120 seconds, close the SSE connection and destroy the session. Send a `{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"reason":"session_timeout"}}` event before closing.
5. Maximum concurrent SSE sessions per gateway instance: 10,000. Return HTTP 503 with `Retry-After: 5` when at capacity.
6. Session ID is a UUIDv4 generated server-side. The client cannot choose its session ID.
7. POST `/messages/{slug}` returns HTTP 400 for unknown session IDs, HTTP 415 for non-JSON content types, HTTP 202 on success.
8. All JSON-RPC responses and server-initiated notifications delivered over the SSE stream are framed as `data: <json>\n\n`. No SSE `event:` field is used (matches Claude Desktop expectations).
9. Integration test: simulate Claude Desktop flow — connect SSE, capture `endpoint` event, POST `initialize`, verify response arrives on SSE stream, POST `tools/list`, verify response, disconnect.
10. Load test: 1,000 concurrent SSE sessions, each sending 1 message per second for 60 seconds. p99 latency from POST to SSE delivery must be under 50 ms.

#### Technical Notes

- Sessions are stored in a `DashMap<Uuid, SessionHandle>` in shared application state. `DashMap` avoids a single `RwLock` bottleneck for the session registry.
- `SessionHandle` contains: `sender: mpsc::Sender<String>`, `slug: String`, `created_at: Instant`, `last_activity: Instant` (updated atomically via `AtomicU64` Unix timestamp).
- A background `tokio::task` sweeps the session map every 30 seconds to expire idle sessions.
- The SSE GET handler uses `axum::response::sse::Sse` with a `tokio_stream::wrappers::ReceiverStream` wrapping the mpsc receiver.
- Keep-alive: send SSE comment lines (`: keepalive\n\n`) every 15 seconds to prevent proxy timeout. axum's `Sse::keep_alive` API handles this.
- This transport is explicitly labelled "legacy" in code comments. A deprecation header (`Deprecation: true`, `Sunset: <date>`) may be added in a future story once Streamable HTTP adoption is confirmed.

#### Dependencies

- S-020 (JSON-RPC parser)
- S-023 (gateway router)
- S-030 (Bearer token auth)
- S-033 (connection limits — SSE sessions count against per-server connection budget)

---

### S-023: Gateway Router — Config-Driven Request Dispatch

**Priority:** P0
**Estimate:** 3 points

#### Description

The router is the central dispatch layer. It receives a parsed JSON-RPC request, a server slug, and an authenticated identity, then routes the request to the correct internal handler based on MCP method. For `tools/list` it returns tool schemas from the config cache. For `tools/call` it dispatches to the upstream proxy pipeline. All other methods (`initialize`, `ping`, `initialized`) are handled locally.

#### Acceptance Criteria

1. Implement `Router::dispatch(slug: &str, request: JsonRpcRequest) -> JsonRpcResponse` as the single entry point for both transports.
2. For slug not found in the config cache: return JSON-RPC error `{"code":-32001,"message":"Server not found"}`.
3. For `initialize`: respond with server capabilities without touching the upstream. Include the server's `name` and `description` from its config row in `serverInfo`.
4. For `initialized`: no-op, return `{"jsonrpc":"2.0","id":null}` (notification acknowledgement).
5. For `ping`: return `{"jsonrpc":"2.0","id":<id>,"result":{}}`.
6. For `tools/list`: call the tool schema generator (S-029) with the server's config and return the schema array. Result is cached per server config version; invalidate when config changes.
7. For `tools/call`: validate that the requested tool name exists in the server's tool list. If not, return `{"code":-32602,"message":"Unknown tool: <name>"}`. Otherwise, pass to the upstream proxy pipeline (S-026 → S-027 → S-028).
8. For any other method: return `{"code":-32601,"message":"Method not found"}`.
9. The dispatch path for `tools/list` (cache hit) must complete in under 100 µs measured in unit test with `criterion`.
10. Unit tests: one test per method variant including unknown slug, unknown tool, and an `initialize` response that correctly reflects the server name from config.

#### Technical Notes

- The router holds an `Arc<ConfigCache>` (S-024) and an `Arc<UpstreamPipeline>` (composed of S-026, S-027, S-028).
- Dispatch is a `match` on `request.method.as_str()`. No dynamic dispatch overhead.
- The `tools/list` cache is a separate `moka::sync::Cache<ServerId, Arc<Vec<McpTool>>>` keyed on `(server_id, config_version)`. When the config cache emits an invalidation for a server, this cache entry is also dropped.
- Pass the full `JsonRpcRequest` (not just params) to the upstream pipeline so that the `id` field is available for constructing the response envelope.
- This module does not perform auth — auth is checked by the transport layer (S-021, S-022) before dispatch is called.

#### Dependencies

- S-020 (JSON-RPC types)
- S-024 (config cache)
- S-026, S-027, S-028 (upstream pipeline)
- S-029 (tool schema generator)

---

### S-024: In-Memory Config Cache

**Priority:** P0
**Estimate:** 5 points

#### Description

All active server configs must be in memory for sub-microsecond lookup on every request. Loading from PostgreSQL on each request is not acceptable at scale. This story implements the config cache: warm population on startup, LRU eviction for inactive configs, and an invalidation interface consumed by the hot reload system (S-025).

#### Acceptance Criteria

1. On gateway startup, load all rows from `mcp_servers` where `status = 'active'` into the cache. Startup must complete (cache warm) within 5 seconds for up to 100,000 server rows.
2. Cache is backed by `moka::sync::Cache` with a maximum capacity of 500,000 entries and a time-to-idle eviction of 1 hour for entries not accessed. LRU eviction applies when capacity is reached.
3. Cache keys are `server_id: Uuid`. Cache values are `Arc<ServerConfig>` (immutable, clone-free reads).
4. `ConfigCache::get(server_id: Uuid) -> Option<Arc<ServerConfig>>` completes in under 1 µs on a warmed cache (verified with `criterion` benchmark against 100,000 entries).
5. `ConfigCache::upsert(config: ServerConfig)` atomically replaces the existing entry. Concurrent reads during an upsert see either the old or the new value, never a partial write.
6. `ConfigCache::remove(server_id: Uuid)` evicts the entry. Subsequent reads return `None`.
7. `ConfigCache::slug_to_id(slug: &str) -> Option<Uuid>` provides O(1) reverse lookup. A secondary `DashMap<String, Uuid>` maintains the slug index. Updates to this index are atomic with cache upserts.
8. On cache miss (entry not found in memory, not expired): attempt a single synchronous PostgreSQL query to load the config. If found and `status = 'active'`, insert into cache and return. If not found, return `None`. This handles the case where a config was added by a NOTIFY that the gateway missed.
9. Expose `ConfigCache::stats() -> CacheStats` with fields: `total_entries`, `hit_count`, `miss_count`, `eviction_count`. Publish stats as Prometheus gauges and counters via the `/metrics` endpoint.
10. Unit test: populate 1,000 entries, read all 1,000, verify hit_count == 1,000. Upsert 10 entries with new values, verify old values are gone. Remove 5 entries, verify miss on those keys. Slug lookup returns correct ID.

#### Technical Notes

- `moka` is the preferred caching library (https://github.com/moka-rs/moka). It is lock-striped and performs well under high concurrency. Use `moka::sync::Cache` (not async) to avoid unnecessary `await` on reads.
- `ServerConfig` is the deserialized form of the `mcp_servers` row plus its joined `credentials` config (but never raw credential values). It includes: `id`, `slug`, `name`, `description`, `base_url`, `auth_type`, `tool_definitions: Vec<ToolDefinition>`, `transform_pipeline: Option<TransformPipeline>`, `timeout_ms`, `max_connections`, `config_version`.
- The fallback PostgreSQL read on cache miss uses a connection from the read-replica pool if one is configured, otherwise the primary pool. Use `sqlx::query_as!` for type-safety.
- Startup load uses `sqlx::query_as!(...).fetch_all()` in a single query. For very large deployments (>100k configs), paginate with `LIMIT 10000 OFFSET n` in a loop and insert in batches of 10k.
- The `config_version` field (a `i64` sequence from PostgreSQL) is stored in the cache entry. The hot reload handler (S-025) compares versions to avoid applying stale NOTIFY messages.

#### Dependencies

- E-01 (Data Layer): `mcp_servers` table schema must be defined.
- S-025 (hot reload) will call `upsert` and `remove` on this cache — interface must be stable before S-025 starts.

---

### S-025: Hot Reload — PostgreSQL LISTEN/NOTIFY

**Priority:** P0
**Estimate:** 5 points

#### Description

Config changes (create, update, delete) made via the API must propagate to all gateway instances without a restart. PostgreSQL LISTEN/NOTIFY is the mechanism: a trigger on `mcp_servers` publishes a notification containing the `server_id` and the change type. The gateway maintains a long-lived `LISTEN` connection and updates its in-memory cache within 2 seconds of the change being committed.

#### Acceptance Criteria

1. Gateway subscribes to the `mcp_server_changes` PostgreSQL channel on startup using a dedicated connection (separate from the request-serving connection pool).
2. The `mcp_servers` table has an `AFTER INSERT OR UPDATE OR DELETE` trigger that calls `pg_notify('mcp_server_changes', payload)` where `payload` is a JSON string `{"server_id":"<uuid>","op":"<created|updated|deleted>","config_version":<n>}`.
3. On receiving a `created` or `updated` notification: query PostgreSQL for the full server config row (including joined credential config), upsert into the config cache. Operation completes within 2 seconds of the original transaction commit (measured end-to-end in integration test).
4. On receiving a `deleted` notification: call `ConfigCache::remove(server_id)`. The server is immediately unavailable to new requests.
5. If the LISTEN connection drops (PostgreSQL restart, network interruption): reconnect with exponential backoff starting at 1 second, capped at 30 seconds. Log each reconnect attempt as a WARN-level structured log. After reconnecting, replay any missed changes by comparing the current cache contents against PostgreSQL (reload all rows modified since `last_sync_at` stored in a local variable).
6. Message ordering: apply notifications in the order they arrive. If two notifications arrive for the same `server_id`, apply only the one with the higher `config_version`. Discard lower-version notifications that arrive out of order.
7. Batch updates: if more than 20 notifications arrive within a 100 ms window, coalesce them by `server_id` (keeping the highest version), then apply the deduplicated set.
8. The LISTEN goroutine (tokio task) is supervised. If it panics, the gateway runtime restarts it. A panic counter is exposed as a Prometheus counter `gateway_config_sync_panics_total`.
9. Integration test: insert a server config, wait for cache population, verify `ConfigCache::get` returns it. Update the config, wait ≤2s, verify the cache reflects the new value. Delete the config, verify `ConfigCache::get` returns `None`.
10. Integration test: simulate a LISTEN connection drop by closing the underlying PostgreSQL connection. Verify the gateway reconnects and re-syncs within 35 seconds (reconnect cap + one sync cycle).

#### Technical Notes

- Use `sqlx::postgres::PgListener` which wraps PostgreSQL's `LISTEN/UNLISTEN` and delivers notifications as an async stream. This is the correct sqlx API for this use case.
- The dedicated LISTEN connection should not be part of the sqlx connection pool — it is a single always-alive connection.
- The trigger SQL (to be created in a migration in E-01's scope):
  ```sql
  CREATE OR REPLACE FUNCTION notify_server_change() RETURNS trigger AS $$
  BEGIN
    IF TG_OP = 'DELETE' THEN
      PERFORM pg_notify('mcp_server_changes',
        json_build_object('server_id', OLD.id, 'op', 'deleted', 'config_version', OLD.config_version)::text);
    ELSE
      PERFORM pg_notify('mcp_server_changes',
        json_build_object('server_id', NEW.id, 'op', TG_OP::text, 'config_version', NEW.config_version)::text);
    END IF;
    RETURN NULL;
  END;
  $$ LANGUAGE plpgsql;

  CREATE TRIGGER mcp_server_change_trigger
  AFTER INSERT OR UPDATE OR DELETE ON mcp_servers
  FOR EACH ROW EXECUTE FUNCTION notify_server_change();
  ```
- The missed-changes replay on reconnect: `SELECT id, config_version FROM mcp_servers WHERE updated_at > $1`. Compare against what is in the cache; fetch and upsert anything that differs.
- The `last_sync_at` timestamp is stored as an `Instant` in the task's local state; it is set to `now()` after each successful batch of upserts.

#### Dependencies

- S-024 (config cache — `upsert` and `remove` APIs)
- E-01 (Data Layer): `mcp_servers` table must exist with `config_version` and `updated_at` columns; trigger migration must be present.

---

### S-026: Upstream HTTP Client

**Priority:** P0
**Estimate:** 5 points

#### Description

Build the HTTP request to the upstream REST API from the server config and the `tools/call` arguments. This includes URL template interpolation, query parameter injection, and request body construction. The client must respect per-server timeout configuration and produce a `reqwest::Request` ready for handoff to the credential injector (S-027).

#### Acceptance Criteria

1. Implement `UpstreamRequestBuilder::build(config: &ServerConfig, tool_call: &ToolCallParams) -> Result<UpstreamRequest, BuildError>`.
2. URL interpolation: replace `{param_name}` placeholders in `config.base_url` and the tool's `path_template` with values from `tool_call.arguments`. Percent-encode substituted values (RFC 3986 unreserved characters pass through). Return `BuildError::MissingPathParam` if a required placeholder has no matching argument.
3. Query parameters: append all key-value pairs from `tool_definition.query_params` to the URL. If a query param key maps to a `tool_call.arguments` key, use the argument value; otherwise use the static default from the tool definition.
4. Request method: use `tool_definition.http_method` (GET, POST, PUT, PATCH, DELETE). Default GET.
5. Request body: for POST/PUT/PATCH, construct a JSON body from `tool_definition.body_template`. The template is a JSON object where string values matching `{param_name}` patterns are replaced with the corresponding argument. Non-matching values are passed through unchanged. Return `BuildError::InvalidBodyTemplate` if the template is not valid JSON after substitution.
6. Set `Content-Type: application/json` for requests with a body.
7. Set `User-Agent: mcp-gateway/1.0` on all upstream requests.
8. Apply a per-request timeout equal to `config.timeout_ms` (default 30,000 ms). Use `reqwest::ClientBuilder::timeout`.
9. Return `UpstreamRequest { method, url, headers, body, timeout }` — a plain struct with no async operations. The credential injector (S-027) receives this struct and makes the actual network call.
10. Unit tests: GET with path param substitution, POST with body template, missing required path param returns error, query param with argument override, URL with multiple path params.
11. Reject `config.base_url` values that are not HTTPS (return `BuildError::InsecureUrl`) unless the gateway is running in dev mode (`GATEWAY_ALLOW_HTTP=true`).

#### Technical Notes

- URL interpolation: use a simple regex `\{([a-zA-Z_][a-zA-Z0-9_]*)\}` to find placeholders. Do not pull in a full template engine; the regex is sufficient and avoids a dependency.
- Body template substitution: deserialize the template to `serde_json::Value`, walk the value tree recursively, replace string leaves that match the placeholder pattern. This approach handles nested objects.
- `reqwest` is already a dependency for the credential injector sidecar HTTP call. Reuse the same `reqwest::Client` instance (connection pooled, keep-alive enabled).
- `UpstreamRequest` deliberately does not include auth headers. Auth is the sidecar's domain.
- URL length limit: 8,192 characters. Return `BuildError::UrlTooLong` if exceeded.

#### Dependencies

- S-024 (config cache provides `ServerConfig` and `ToolDefinition`)
- S-020 (provides `ToolCallParams` struct from parsed JSON-RPC params)

---

### S-027: Credential Injection Flow

**Priority:** P0
**Estimate:** 8 points

#### Description

The gateway must never hold raw credentials. All auth injection is delegated to a co-located credential injector sidecar process. The gateway sends a sanitised HTTP request skeleton (URL, headers without auth, body) to the sidecar over a Unix domain socket. The sidecar decrypts the credential, adds the auth header (or query param or body field), makes the upstream call, and returns the response body and status. The gateway receives results without ever seeing the plaintext credential.

#### Acceptance Criteria

1. Define the sidecar IPC protocol as a JSON-over-Unix-socket request/response. Request schema: `{"server_id":"<uuid>","request":{"method":"POST","url":"https://...","headers":{"X-Custom":"value"},"body":"<base64-encoded or null>"}}`. Response schema: `{"status":200,"headers":{},"body":"<base64-encoded>","latency_ms":42}`.
2. The gateway opens a connection to the sidecar over the Unix domain socket at the path configured by `SIDECAR_SOCKET_PATH` (default: `/run/mcp-gateway/sidecar.sock`). Use a connection pool of up to 32 sockets.
3. Each `tools/call` that requires upstream auth sends exactly one IPC request to the sidecar. The gateway does not construct or inspect the `Authorization` header; the sidecar adds it.
4. If the sidecar is unavailable (socket not found, connection refused, timeout): return JSON-RPC error `{"code":-32002,"message":"Upstream unavailable","data":{"reason":"credential_service_unreachable"}}`. Do not retry in the hot path (circuit breaker in S-032 handles retries).
5. IPC request timeout: 35 seconds (5 seconds above the 30s upstream default). Return JSON-RPC error `{"code":-32002,"message":"Upstream timeout"}` if exceeded.
6. The sidecar response's `status` field is used to determine success. HTTP 2xx → success. HTTP 4xx → surface as `{"code":-32003,"message":"Upstream error","data":{"status":<n>}}`. HTTP 5xx → treat as upstream failure (increment circuit breaker counter).
7. The gateway passes `server_id` to the sidecar but does not pass credential values. The sidecar independently looks up and decrypts the credential by `server_id`. This is the trust boundary.
8. For servers with `auth_type = 'none'`: the gateway makes the upstream HTTP call directly (no sidecar hop). Use the `reqwest::Client` directly.
9. The IPC protocol is documented in `docs/sidecar-ipc-protocol.md`. This file is created as part of this story.
10. Integration test with a mock sidecar (a local TCP server on a temp socket): send a `tools/call`, verify the gateway sends the correct IPC request structure, verify the gateway returns the mock response body to the MCP client.
11. Test: sidecar unavailable — verify JSON-RPC error `-32002` with `credential_service_unreachable` reason.
12. Test: `auth_type = 'none'` server — verify no socket connection is made (mock sidecar receives zero connections).

#### Technical Notes

- Use `tokio::net::UnixStream` for the IPC connection. Wrap in a `bb8` connection pool (`bb8-tokio-unixstream` or implement a custom `ManageConnection`).
- The IPC request/response framing: length-prefixed (4-byte big-endian `u32` message length followed by JSON bytes). This allows efficient stream parsing without HTTP overhead on the socket.
- The credential injector sidecar is a separate Rust binary in the same repository under `crates/credential-injector/`. It is out of scope for this story except for agreeing on the IPC protocol.
- Sensitive data audit: the gateway must NEVER log or trace the IPC request body (it may contain injected auth tokens by the time it enters the sidecar). Log only: `server_id`, `method`, `url` (path only, no query string if query contains auth tokens), `status`, `latency_ms`.
- The sidecar runs as a separate OS process with different permissions (access to the credential encryption key). The Unix socket file is owned by the sidecar with mode `0600`, preventing other processes from connecting.

#### Dependencies

- S-026 (produces `UpstreamRequest` consumed by this story)
- S-024 (provides `auth_type` from `ServerConfig`)
- E-01 (Data Layer): `credentials` table schema required by sidecar (out of scope here, but must be agreed)
- S-032 (circuit breaker tracks sidecar failures)

---

### S-028: Response Transformation Engine

**Priority:** P0
**Estimate:** 8 points

#### Description

Transform the upstream API response into an MCP-compliant tool result. The transform pipeline is declarative — defined in the server config as an ordered list of named operations. The engine processes the pipeline in under 5 ms for typical responses. It must handle all upstream response types (JSON only in MVP) and produce a clean, schema-validated MCP result.

#### Acceptance Criteria

1. Accept an upstream response body (JSON string, max 100 KB) and a `TransformPipeline` config. Return a `serde_json::Value` representing the MCP tool result content.
2. Return `TransformError::ResponseTooLarge` if the response exceeds 100 KB. Return `TransformError::InvalidJson` if the response is not valid JSON.
3. Implement the following transform operations, applied in declared order:
   - `jsonpath_extract`: extract a value at a JSONPath expression (e.g., `$.results[*].name`). Powered by `jsonpath-rust` crate. Store the extracted value under a named key in the working document.
   - `field_rename`: rename a key in the working document to a new key. `{"op":"field_rename","from":"userId","to":"user_id"}`.
   - `type_coerce`: coerce a field's value. Supported coercions: `string_to_number`, `number_to_string`, `string_to_bool`, `bool_to_string`, `date_format` (input ISO 8601 string, output format string using `chrono` format specifiers).
   - `arithmetic`: apply a basic arithmetic expression to a numeric field. Supported operations: `add`, `subtract`, `multiply`, `divide` with a literal second operand. E.g., `{"op":"arithmetic","field":"price_cents","operation":"divide","operand":100,"output":"price_dollars"}`.
   - `string_op`: apply a string operation to a field. Supported: `concat` (with a literal suffix/prefix), `trim`, `uppercase`, `lowercase`, `truncate` (max length with `...` suffix).
   - `array_flatten`: flatten a nested array one level. E.g., `[[1,2],[3,4]]` → `[1,2,3,4]`.
   - `select_fields`: keep only the listed top-level keys in the working document. Equivalent to SQL `SELECT`.
   - `drop_fields`: remove listed top-level keys from the working document.
4. Each operation that fails (e.g., JSONPath expression that matches nothing, type coercion of a non-numeric string) produces a `TransformWarning` (not an error). The pipeline continues. Warnings are included in the structured log for the request.
5. The final working document is wrapped in an MCP `content` array: `[{"type":"text","text":"<json_serialized_result>"}]`. This is the value returned from `tools/call`.
6. A `TransformPipeline` with zero operations returns the upstream response body verbatim (still JSON-serialized, wrapped in MCP content).
7. Benchmark: a pipeline with 10 operations on a 10 KB response body completes in under 5 ms (measured with `criterion`).
8. Unit tests: one test per operation type with a valid input and expected output. One test per operation type with an invalid input verifying a warning is produced. One test for an empty pipeline. One test for a response exceeding 100 KB.
9. All transform operations are defined as variants of a `TransformOp` enum with `serde` derive. The pipeline config is fully serialisable to/from JSON.

#### Technical Notes

- The working document is a `serde_json::Map<String, Value>` throughout the pipeline. Operations mutate this map in place using `get_mut` / `insert` / `remove`. Avoid cloning the full document between steps.
- JSONPath extraction with `jsonpath-rust`: the crate returns a `Vec<&Value>` of matches. For single-value extractions, take `[0]`. For array extractions (`[*]`), collect into a `Value::Array`. Pre-compile JSONPath expressions when loading the `TransformPipeline` config (not per-request).
- Arithmetic overflow: use `checked_add`, `checked_sub`, `checked_mul`. Division by zero produces a warning and leaves the field unchanged.
- Date formatting: parse the input with `chrono::DateTime::parse_from_rfc3339`, reformat with `chrono::format::strftime`. Invalid date strings produce a warning.
- The transform engine is a pure function: `fn apply(pipeline: &TransformPipeline, input: &str) -> (Value, Vec<TransformWarning>)`. No I/O, no async. This makes it trivially testable and benchmarkable.

#### Dependencies

- S-026, S-027 (provide the upstream response body)
- S-024 (provides `TransformPipeline` from `ServerConfig`)

---

### S-029: MCP Tool Schema Generation

**Priority:** P0
**Estimate:** 3 points

#### Description

Generate the MCP-compliant `tools/list` response from a server's config. Each configured endpoint becomes a tool with a name, description, and JSON Schema input parameter definition. The generated schema must pass MCP spec validation so that any compliant MCP client can discover and call the tools without additional configuration.

#### Acceptance Criteria

1. Implement `generate_tool_schemas(config: &ServerConfig) -> Vec<McpTool>`.
2. Each `ToolDefinition` in the server config produces one `McpTool`. `McpTool` has: `name: String`, `description: String`, `inputSchema: JsonSchema`.
3. `inputSchema` is a JSON Schema object (`"type":"object"`) with a `properties` map. Each parameter source contributes properties:
   - Path parameters (extracted from `{param}` placeholders in the URL template) → required, type `string`.
   - Query parameters marked `required: true` in the tool definition → required, type as declared.
   - Query parameters marked `required: false` → optional, type as declared.
   - Body parameters from the body template → type as declared, required if marked.
4. The `required` array in the JSON Schema lists only parameters that are `required: true`.
5. Tool names must match the regex `^[a-zA-Z][a-zA-Z0-9_-]{0,63}$`. If a tool definition's name fails this check, log an error and omit the tool from the schema list (do not panic).
6. Description field maximum length: 1,024 characters. Truncate with `...` if exceeded.
7. The returned `Vec<McpTool>` is serialised to `{"tools":[...]}` in the `tools/list` response result field.
8. Validate the generated schema array against the MCP 2025-03-26 spec JSON Schema (embed as a static file in `tests/fixtures/mcp-tools-list-schema.json`). The integration test sends `tools/list` and validates the response.
9. Unit tests: server with one GET endpoint (path param only), one POST endpoint (body params), one endpoint with all param types mixed, one endpoint with an invalid tool name (verify it is omitted), empty tool list (valid `{"tools":[]}`).

#### Technical Notes

- Path parameter extraction reuses the same regex from S-026 (`\{([a-zA-Z_][a-zA-Z0-9_]*)\}`). Factor this regex into a shared utility in `gateway::util::template`.
- JSON Schema generation uses `serde_json::json!` macro to construct the schema value. Do not pull in `schemars` for this simple use case.
- Parameter types in tool definitions: `string`, `number`, `integer`, `boolean`, `array`. Map these to JSON Schema `"type"` values directly. Unknown types default to `"string"` with a log warning.
- The generated `Vec<McpTool>` is cached per `(server_id, config_version)` in the same `moka` cache used by S-023. Cache miss calls `generate_tool_schemas` and stores the result.

#### Dependencies

- S-024 (provides `ServerConfig` and `ToolDefinition`)
- S-023 (consumes the generated schema for `tools/list` responses)

---

### S-030: Per-Server Bearer Token Authentication

**Priority:** P0
**Estimate:** 5 points

#### Description

Every MCP server has its own Bearer token. MCP clients must present this token in the `Authorization: Bearer <token>` header. The gateway validates the token before dispatching the request to any handler. Tokens are cryptographically random, stored hashed in PostgreSQL, and can be revoked or regenerated independently per server.

#### Acceptance Criteria

1. Token generation: `generate_token() -> (raw: String, hashed: String)`. Raw token is 32 random bytes encoded as URL-safe base64 (43 characters, no padding). Hash is `Argon2id` with parameters `m=65536, t=1, p=1` (memory-hard, suitable for tokens). The raw token is returned once at creation and never stored.
2. Token storage: the `mcp_servers` table has columns `token_hash: TEXT NOT NULL` and `token_prefix: CHAR(8)` (first 8 chars of the raw token for display/identification purposes only).
3. Token validation in the request path:
   - Extract the `Authorization` header. If absent, return HTTP 401 with `WWW-Authenticate: Bearer realm="mcp-gateway"`.
   - Extract the Bearer token string. If malformed (not `Bearer <token>` format), return HTTP 401.
   - Look up the server config by slug (config cache hit). Extract `token_hash`.
   - Verify the token with `argon2::verify_raw`. If invalid, return HTTP 401.
   - If valid, allow the request to proceed to dispatch.
4. Return HTTP 403 (not 401) when the token is valid but the server `status` is `'suspended'`.
5. Token validation adds at most 2 ms to request latency (Argon2 at the specified parameters runs in ~1 ms on modern hardware; verify in benchmark).
6. Token revocation: the API (E-03) can rotate a token by generating a new one and updating `token_hash`. The old token becomes invalid immediately (no grace period).
7. Timing-safe comparison: use `argon2::verify_raw` which is inherently timing-safe. Do not compare hashes with `==`.
8. The token prefix (first 8 chars) is logged in the request log to help users identify which token made a request, without logging the full token.
9. Unit tests: valid token accepted, invalid token rejected (401), missing header rejected (401), suspended server returns 403, token prefix correctly extracted.
10. Integration test: create a server, generate a token, make an authenticated `tools/list` request, verify success. Make the same request with a wrong token, verify 401.

#### Technical Notes

- Use the `argon2` crate (Rust implementation). Configure with `Argon2::new(Algorithm::Argon2id, Version::V0x13, Params::new(65536, 1, 1, None).unwrap())`.
- Token validation is synchronous (Argon2 runs on the calling thread). For a gateway handling thousands of concurrent requests, this could become a CPU bottleneck. Add a semaphore limiting concurrent Argon2 validations to `num_cpus * 2`. Requests that cannot acquire the semaphore immediately are queued (this is acceptable because the 2 ms per validation amortizes well under the 100 ms p95 target).
- Token cache: after a successful validation, cache `(slug, token_raw) -> valid` in a short-lived `moka` cache with TTL of 30 seconds. This avoids re-hashing for the same client making repeated requests. Cache invalidation on token rotation: the hot reload path (S-025) must also evict this cache entry.
- The `Authorization` header is never logged. The `token_prefix` (8 chars) is safe to log.

#### Dependencies

- S-024 (provides `token_hash` from `ServerConfig`)
- S-025 (must evict token validation cache on config update)
- E-01 (Data Layer): `token_hash` and `token_prefix` columns in `mcp_servers`

---

### S-031: Request/Response Logging

**Priority:** P1
**Estimate:** 3 points

#### Description

Every MCP request must produce a structured log entry for observability, debugging, and audit purposes. The logger writes asynchronously to avoid adding latency to the request path. It never logs credentials, auth headers, or full response bodies.

#### Acceptance Criteria

1. Log one structured record per `tools/call` and `tools/list` request with fields: `server_id` (UUID), `server_slug` (string), `method` (string), `tool_name` (string or null for `tools/list`), `upstream_url` (URL without query string if query contains auth tokens), `upstream_status` (integer or null for `tools/list`), `latency_ms` (total gateway time), `upstream_latency_ms` (time from sidecar send to sidecar response), `response_size_bytes` (size of the MCP response JSON), `transform_warnings` (array of warning strings), `token_prefix` (8-char prefix from S-030), `instance_id` (gateway instance UUID, set at startup), `trace_id` (UUIDv4, generated per request).
2. Log records are written to stdout as newline-delimited JSON (NDJSON). One JSON object per line. This is the format expected by log aggregators (Datadog, CloudWatch, Loki).
3. The logger uses a `tokio::sync::mpsc` channel (capacity 4,096) as an async buffer. The request handler sends a log record to the channel and continues without blocking. A dedicated background task drains the channel and writes to stdout.
4. If the channel is full (backpressure), the log record is dropped and a counter `gateway_log_drops_total` is incremented. The request is NOT slowed down by logging.
5. NEVER log: `Authorization` header values, full request bodies, full response bodies, raw credential values, the `token_hash` field, query parameters named `api_key`, `token`, `secret`, `password`, `key`.
6. Log level is configurable via `LOG_LEVEL` environment variable (`trace`, `debug`, `info`, `warn`, `error`). Default `info`. At `debug` level, log the full upstream URL including query string (with sensitive params redacted per rule 5).
7. `initialize` and `ping` requests are logged at `debug` level only (not `info`) to avoid log noise.
8. The `trace_id` is propagated to the sidecar IPC request (added to the IPC JSON envelope) so that sidecar logs can be correlated.
9. Unit test: send 10 log records, verify all 10 appear in stdout with correct fields. Send a record with a sensitive query param in the URL, verify the URL is sanitised in the log output. Fill the channel to capacity (mock), verify the drop counter increments and the thread does not block.
10. Log format includes a `timestamp` field in RFC 3339 format with millisecond precision.

#### Technical Notes

- Use `tracing` + `tracing-subscriber` with a custom JSON formatter (or `tracing-subscriber::fmt::format::Json`). Configure the subscriber to write to stdout.
- The request context (server_id, slug, trace_id) must be propagated through the async call stack. Use `tracing::Span` with custom fields, entered at the transport layer and inherited by all downstream calls.
- URL sanitisation: after building the upstream URL (S-026), pass it through a `sanitise_url(url: &Url) -> String` function that strips or masks query params matching the sensitive param name list. This function is pure and unit-testable.
- Structured log fields use snake_case keys. Do not use the `tracing` default of dot-notation (`span.field`).
- The `instance_id` is a UUIDv4 generated once at gateway startup and stored in the global app state. It distinguishes log lines from different gateway instances in a multi-instance deployment.

#### Dependencies

- S-020 (provides `method` and `tool_name`)
- S-026, S-027 (provide `upstream_url`, `upstream_status`, `upstream_latency_ms`)
- S-028 (provides `transform_warnings`)
- S-030 (provides `token_prefix`)

---

### S-032: Circuit Breaker Per Upstream API

**Priority:** P1
**Estimate:** 5 points

#### Description

Upstream APIs fail. When they do, the gateway must stop hammering them and return a clean error to the MCP client. A per-server circuit breaker tracks upstream health and opens the circuit after repeated failures, giving the upstream time to recover.

#### Acceptance Criteria

1. Each server has an independent circuit breaker tracked in shared state (keyed by `server_id`). Circuit breakers are created on first use and persist until the server is deleted.
2. Three states: `Closed` (normal operation), `Open` (failing fast, no upstream calls), `HalfOpen` (probing, limited calls allowed).
3. State transitions:
   - `Closed → Open`: after 5 consecutive upstream failures. "Failure" is defined as: HTTP 5xx from upstream, upstream timeout, sidecar unreachable.
   - `Open → HalfOpen`: automatically after 30 seconds.
   - `HalfOpen → Closed`: after 3 consecutive successes in `HalfOpen` state.
   - `HalfOpen → Open`: on any failure in `HalfOpen` state. Reset the 30-second timer.
4. When the circuit is `Open`: return JSON-RPC error `{"code":-32004,"message":"Upstream temporarily unavailable","data":{"retry_after_ms":30000}}` without making any upstream call.
5. When the circuit is `HalfOpen`: allow at most 1 concurrent upstream call. If another request arrives while one probe is in flight, return the `Open` error without waiting.
6. HTTP 4xx responses from the upstream (client errors) do NOT count as failures for the circuit breaker. The upstream is functioning correctly; the request was bad.
7. HTTP 429 (rate limit) from the upstream: count as a failure AND set `retry_after_ms` from the upstream's `Retry-After` header if present.
8. Circuit breaker state changes are logged at `WARN` level: `circuit_breaker_opened`, `circuit_breaker_half_open`, `circuit_breaker_closed`.
9. Circuit breaker state and failure counts are exposed as Prometheus gauges: `gateway_circuit_breaker_state{server_id="<uuid>"}` (0=Closed, 1=Open, 2=HalfOpen) and `gateway_circuit_breaker_consecutive_failures{server_id="<uuid>"}`.
10. Unit tests: 5 failures → open, 30s elapsed → half-open, 3 successes → closed. 4xx does not open. HalfOpen → Open on failure. Concurrent probe in HalfOpen state returns Open error.

#### Technical Notes

- Implement `CircuitBreaker` as a struct with `state: AtomicU8`, `consecutive_failures: AtomicU32`, `opened_at: AtomicU64` (Unix millis). Atomic operations avoid locking on the hot path.
- State encoding: 0 = Closed, 1 = Open, 2 = HalfOpen. Use compare-and-swap for state transitions.
- `HalfOpen` probe concurrency: use an `AtomicBool` flag (`probe_in_flight`). Set to `true` with CAS when entering HalfOpen and sending a probe. Reset to `false` after the probe completes regardless of outcome.
- The `CircuitBreakerRegistry` is an `Arc<DashMap<Uuid, Arc<CircuitBreaker>>>` stored in app state. Creating a new entry races are harmless (insert-if-absent with `DashMap::entry`).
- 30-second `Open → HalfOpen` transition is checked on each incoming request (lazy transition), not via a background timer. This avoids timer complexity.
- The `failkit` or `failsafe-rs` crates were evaluated but are too opinionated for this use case. Implement the state machine directly (~150 lines).

#### Dependencies

- S-027 (circuit breaker is checked before and notified after each sidecar/upstream call)
- S-031 (circuit breaker state changes are logged)

---

### S-033: Connection Management and Backpressure

**Priority:** P1
**Estimate:** 5 points

#### Description

Without connection limits, a single misconfigured or abusive MCP client could exhaust gateway resources and degrade service for all other users. This story enforces per-server and global connection limits, applies backpressure via HTTP 503, and enables graceful shutdown connection draining.

#### Acceptance Criteria

1. Per-server connection limit: configurable via `config.max_connections` (default 50). Counts all in-flight requests for a given `server_id` (both transports combined). When the limit is reached, return HTTP 503 with `Retry-After: 1` and a JSON-RPC error `{"code":-32005,"message":"Server at capacity"}`.
2. Global connection limit per gateway instance: configurable via `GATEWAY_MAX_CONNECTIONS` environment variable (default 10,000). When reached, return HTTP 503 with `Retry-After: 1`.
3. Per-server connection counting: use a `DashMap<Uuid, AtomicI32>` in app state. Increment on request entry (after auth, before dispatch), decrement on response completion (including error paths). Use `scopeguard::defer!` to guarantee decrement even on panic.
4. A Prometheus gauge `gateway_active_connections{server_id="<uuid>"}` tracks per-server connection count. A gauge `gateway_active_connections_total` tracks the global count.
5. SSE connections (S-022) count as 1 connection for the duration of the SSE session.
6. For graceful shutdown (S-034): expose `ConnectionTracker::drain()` which returns a `Future` that resolves when all active connection counts reach zero. This is used by the shutdown handler.
7. Response headers on 503: include `Content-Type: application/json` and the JSON-RPC error body (not a plain HTML error).
8. Log a `WARN` when a server first hits its connection limit. Do not log for every subsequent rejection (rate-limit the log: at most once per 10 seconds per server).
9. Unit tests: counter increments on entry and decrements on exit, 503 returned when limit exceeded, global limit respected, decrement happens even if handler panics (via scopeguard).
10. The connection limit check happens after authentication (S-030) to prevent unauthenticated clients from consuming connection budget.

#### Technical Notes

- `AtomicI32` is used (signed) so that decrement-before-increment races do not underflow to `u32::MAX`. In practice this should not occur with `scopeguard`, but signed arithmetic is safer.
- The per-server `DashMap` is the same `DashMap<Uuid, ConnectionCounter>` — wrap `AtomicI32` in a newtype `ConnectionCounter` that also holds the configured limit. `ConnectionCounter::try_acquire() -> Result<ConnectionGuard, CapacityError>` returns a guard that decrements on drop.
- The `GATEWAY_MAX_CONNECTIONS` global limit is enforced by a `tokio::sync::Semaphore` with `permits = GATEWAY_MAX_CONNECTIONS`. Acquire the semaphore before incrementing the per-server counter. The `SemaphorePermit` is held for the duration of the request and released on drop.
- Connection limit configuration: per-server limit can be updated via hot reload (S-025). When the new limit is lower than current connections, existing connections are not dropped — the limit applies only to new incoming requests.

#### Dependencies

- S-024 (provides `max_connections` from `ServerConfig`)
- S-025 (hot reload can update the limit)
- S-030 (auth must succeed before connection is counted)
- S-034 (consumes `drain()` API)

---

### S-034: Gateway Graceful Shutdown

**Priority:** P1
**Estimate:** 3 points

#### Description

During rolling deploys and deliberate shutdowns, the gateway must stop accepting new connections, drain in-flight requests, flush all async writers, and exit cleanly. Zero requests should be dropped during a rolling deploy.

#### Acceptance Criteria

1. Trap `SIGTERM` and `SIGINT`. On receipt, begin the shutdown sequence immediately.
2. Shutdown sequence (in order):
   a. Set a global `AtomicBool` flag `is_shutting_down = true`. New incoming connections receive HTTP 503 with `Connection: close` immediately after this flag is set.
   b. Signal the Kubernetes/load balancer readiness probe: stop responding to `GET /healthz/ready` with 200. The probe endpoint returns 503 during shutdown. Allow 5 seconds for the load balancer to drain traffic.
   c. Wait for all in-flight connections to complete using `ConnectionTracker::drain()` (from S-033). Timeout: 30 seconds.
   d. If drain times out: log a WARN with the count of connections that were forcibly dropped. Close remaining connections with a `connection: close` header on HTTP/1.1, or send a `GOAWAY` frame on HTTP/2.
   e. Flush the log writer channel (from S-031): send a sentinel value and wait for the background writer task to process all queued records.
   f. Close the PostgreSQL LISTEN connection (from S-025).
   g. Close the sidecar Unix socket pool (from S-027).
   h. Exit with code 0.
3. The total shutdown time must not exceed 40 seconds (5s LB drain + 30s connection drain + 5s log flush buffer).
4. During graceful drain, already-accepted HTTP/1.1 connections receive `Connection: close` on their next response so that the client does not attempt to reuse the connection after it closes.
5. During graceful drain, HTTP/2 connections receive a `GOAWAY` frame after the current stream completes.
6. Liveness probe `GET /healthz/live` continues to return 200 during shutdown (the process is alive; only readiness changes).
7. Log the full shutdown sequence at `INFO` level with timestamps: `shutdown_initiated`, `lb_drain_complete`, `connections_drained`, `logs_flushed`, `exiting`.
8. Integration test: start the gateway, open 5 concurrent long-running SSE connections, send `SIGTERM`, verify all 5 connections complete their current response (not mid-message), verify the gateway exits within 40 seconds, verify exit code 0.

#### Technical Notes

- Use `tokio::signal::unix::signal(SignalKind::terminate())` for SIGTERM and `tokio::signal::ctrl_c()` for SIGINT. Race them with `tokio::select!`.
- The `is_shutting_down` flag is checked in the axum middleware layer before routing. New requests get 503 immediately.
- `ConnectionTracker::drain()` returns a `tokio::sync::watch::Receiver<usize>` that yields the current connection count. Poll until the count reaches zero or timeout.
- Use `axum_server::Handle` (from `axum-server` crate) for graceful shutdown of the axum HTTP server. `Handle::graceful_shutdown(Some(Duration::from_secs(30)))` initiates drain.
- The log flush uses a `oneshot::channel`. Send the oneshot sender as a sentinel through the mpsc log channel. When the background writer receives it, it sends a signal back. Await the signal.
- Ordered shutdown of async resources is implemented as a sequence of `await` calls in the main task — no complex dependency graph is needed.

#### Dependencies

- S-033 (provides `ConnectionTracker::drain()`)
- S-031 (log channel flush)
- S-025 (close LISTEN connection)
- S-027 (close sidecar socket pool)

---

### S-035: Multi-Instance Gateway Support

**Priority:** P1
**Estimate:** 3 points

#### Description

The gateway must be horizontally scalable. Any instance must be able to serve any server without session affinity. This story validates the stateless design, documents the shared-state contract, implements health check endpoints, and verifies that LISTEN/NOTIFY config sync works correctly across instances.

#### Acceptance Criteria

1. All per-request state is in memory within the lifetime of a single request. No persistent in-process state is required between requests (the config cache is a warm read-through cache, not the source of truth).
2. Implement `GET /healthz/live` — returns HTTP 200 `{"status":"ok","instance_id":"<uuid>"}` as long as the process is running. Used by Kubernetes liveness probe.
3. Implement `GET /healthz/ready` — returns HTTP 200 `{"status":"ready","instance_id":"<uuid>","cache_entries":<n>}` when the gateway is ready to serve traffic (config cache warm, sidecar socket connected). Returns HTTP 503 `{"status":"not_ready","reason":"<string>"}` otherwise. Used by Kubernetes readiness probe and load balancer health check.
4. Ready conditions: (a) the initial config cache load (S-024) has completed, (b) the PostgreSQL LISTEN connection (S-025) is established, (c) the sidecar Unix socket pool (S-027) has at least one healthy connection.
5. Implement `GET /metrics` — returns Prometheus text-format metrics. Secured by a static token in `METRICS_TOKEN` environment variable (avoid exposing metrics publicly).
6. Session affinity is NOT required. The load balancer must be configured in round-robin mode. Document this in `docs/deployment.md`.
7. The SSE transport (S-022) does require connection affinity for the duration of a single SSE session (the session state is in-process). Document this requirement: use a load balancer that supports sticky sessions for SSE endpoints only, OR use a Redis-backed session store in a future story. For MVP, document the limitation and recommend sticky sessions for SSE.
8. Integration test: start two gateway instances (same PostgreSQL database), send `tools/call` to instance A, update the server config via direct SQL, verify instance B's cache is updated within 2 seconds (LISTEN/NOTIFY propagates to both). Send `tools/call` to instance B with the updated config.
9. Instance ID: a UUIDv4 generated at startup, logged on every structured log line (from S-031), and included in health check responses. Enables per-instance filtering in log aggregators.
10. Document in `docs/deployment.md`: stateless design rationale, load balancer configuration (round-robin for Streamable HTTP, sticky sessions for SSE), health check endpoint contract, scaling out procedure, config cache warm-up time estimate at scale.

#### Technical Notes

- The `instance_id` is stored in the global `AppState` struct alongside the config cache, circuit breaker registry, and connection tracker. It is generated once with `Uuid::new_v4()` and never changes.
- The readiness check is a synchronous query of in-process state (no PostgreSQL query on the probe path). Check a `AtomicBool` flag `cache_loaded` (set by S-024 after startup load completes) and `AtomicBool` flags for LISTEN connection and sidecar socket health.
- `/healthz/live` and `/healthz/ready` are excluded from request logging (S-031) to prevent probe noise in production logs.
- `/metrics` uses the `prometheus` crate's default registry. All counters and gauges defined in S-024, S-031, S-032, S-033 register themselves at module init time using `lazy_static!` or `once_cell`.
- SSE affinity note for docs: cloud load balancers (AWS ALB, GCP GLB, nginx) all support cookie-based sticky sessions. For SSE, configure stickiness on the `/sse/{slug}` path prefix. Streamable HTTP (`/mcp/{slug}`) needs no stickiness.

#### Dependencies

- S-024 (cache warm flag)
- S-025 (LISTEN connection health flag)
- S-027 (sidecar socket health)
- S-031 (instance_id in logs)
- S-033, S-034 (graceful shutdown interacts with readiness probe)

---

## Story Summary Table

| ID    | Title                                      | Priority | Points | Depends On              |
|-------|--------------------------------------------|----------|--------|-------------------------|
| S-020 | JSON-RPC 2.0 message parser                | P0       | 3      | —                       |
| S-021 | Streamable HTTP transport                  | P0       | 5      | S-020, S-023, S-030     |
| S-022 | SSE transport (fallback)                   | P1       | 5      | S-020, S-023, S-030, S-033 |
| S-023 | Config-driven request dispatch             | P0       | 3      | S-020, S-024, S-026–S-029 |
| S-024 | In-memory config cache                     | P0       | 5      | E-01                    |
| S-025 | Hot reload via LISTEN/NOTIFY               | P0       | 5      | S-024, E-01             |
| S-026 | Upstream HTTP client                       | P0       | 5      | S-024, S-020            |
| S-027 | Credential injection flow                  | P0       | 8      | S-026, S-024, S-032     |
| S-028 | Response transformation engine             | P0       | 8      | S-026, S-027, S-024     |
| S-029 | MCP tool schema generation                 | P0       | 3      | S-024, S-023            |
| S-030 | Per-server Bearer token authentication     | P0       | 5      | S-024, S-025            |
| S-031 | Request/response logging                   | P1       | 3      | S-020, S-026–S-028, S-030 |
| S-032 | Circuit breaker per upstream API           | P1       | 5      | S-027, S-031            |
| S-033 | Connection management and backpressure     | P1       | 5      | S-024, S-025, S-030     |
| S-034 | Gateway graceful shutdown                  | P1       | 3      | S-033, S-031, S-025, S-027 |
| S-035 | Multi-instance gateway support             | P1       | 3      | S-024–S-025, S-027, S-031, S-033–S-034 |
| **Total** |                                       |          | **80** |                         |

## Sprint Allocation Guidance

**Sprint 1 (foundation, ~21 points):** S-020, S-024, S-025, S-029 — establish parsing, cache, hot reload, and tool schema. No HTTP server yet. Focus on unit and integration tests for these pure modules.

**Sprint 2 (request path, ~26 points):** S-026, S-027, S-030, S-023 — build the upstream pipeline and auth. The gateway can now serve a hardcoded Streamable HTTP endpoint for local testing.

**Sprint 3 (transports + transform, ~21 points):** S-021, S-028, S-031 — transports go live, transform engine active, logs flowing. Full end-to-end `tools/call` working in staging.

**Sprint 4 (resilience + SSE, ~22 points):** S-022, S-032, S-033, S-034, S-035 — SSE fallback, circuit breaker, connection limits, graceful shutdown, multi-instance verified. Production-ready.
