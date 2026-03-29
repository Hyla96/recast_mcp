# EPIC-01: Foundation Microservices

**Epic ID:** EPIC-01
**Product:** Dynamic MCP Server Builder
**Architecture:** Gateway Pattern (Option B) — single multi-tenant Rust proxy, credential injector sidecar
**Status:** Ready for Engineering
**Date:** 2026-03-28
**Depends on:** EPIC-00 complete (S-000 through S-009, minimum P0 stories)

---

## Epic Summary

Implement the three foundational microservices (`services/api`, `services/gateway`, `services/credential-injector`) with their full middleware stacks, authentication integration, core data access, and security modules. No business-level features (MCP protocol, tool CRUD, visual mapper) are built in this epic. The output is three independently deployable, production-ready service skeletons that enforce auth, handle errors consistently, emit telemetry, rate-limit callers, and protect against SSRF — ready to receive feature work in EPIC-02 and beyond.

## Epic Acceptance Criteria

- All three services start from their individual Docker images without errors.
- A new user can sign up via Clerk, and their user record is synced to the local PostgreSQL `users` table within 2 seconds.
- An authenticated API call to `GET /v1/users/me` returns the caller's user record.
- An unauthenticated call to any `/v1/*` endpoint returns `401 Unauthorized` with a standard error body.
- A credential can be stored via the API (encrypted at rest), verified to not appear as plaintext in the database, and the credential injector can retrieve and inject it into an outgoing request.
- An SSRF-blocked URL passed to the URL validation module returns a structured error within 10ms.
- All three services pass their `/readyz` health checks in the Docker Compose environment.
- Rate limiting returns `429 Too Many Requests` with a `Retry-After` header after the configured limit is exceeded.

---

## Stories

---

### S-010: Platform API Service Scaffolding

**Story ID:** S-010
**Epic:** EPIC-01
**Priority:** P0
**Estimated Effort:** 5 points
**Dependencies:** S-000, S-003, S-004, S-005, S-006, S-008

#### Description

Scaffold the `services/api` axum application with the complete middleware stack, database connection pool, graceful shutdown, and all cross-cutting concerns wired up. This story produces no business-logic endpoints (those are in S-016 and S-017) but produces a running service where any new endpoint added by a subsequent story automatically inherits authentication, rate limiting, request logging, CORS, error formatting, and telemetry.

Think of this story as building the chassis — every subsequent API story drops an engine component into a pre-wired frame.

#### Required Middleware Stack

The middleware stack must be applied in this specific order (outermost to innermost):

```
Request
  |
  v
[1] CatchPanic         — catch handler panics, log, return 500 (never crash the process)
  |
  v
[2] RequestId          — generate/propagate X-Request-ID header
  |
  v
[3] TraceLayer         — OpenTelemetry span per request, propagate W3C traceparent
  |
  v
[4] StructuredLogger   — log request start/end with status, latency, request_id
  |
  v
[5] MetricsLayer       — increment http_requests_total, record http_request_duration_seconds
  |
  v
[6] CorsLayer          — enforce CORS policy from MCP_API_CORS_ORIGINS config
  |
  v
[7] CompressionLayer   — gzip/br response compression
  |
  v
[8] TimeoutLayer       — 30s hard timeout on all requests (returns 408 if exceeded)
  |
  v
[9] RateLimitLayer     — per-user rate limiting (S-018 implements this; placeholder for now)
  |
  v
[10] AuthLayer         — Clerk JWT validation (S-011 implements this; returns 401 if invalid)
  |
  v
Route handler
```

Layers 9 and 10 are stubs in this story (pass-through). S-011 fills in layer 10; S-018 fills in layer 9.

#### Connection Pool Configuration

```rust
let pool = sqlx::PgPoolOptions::new()
    .max_connections(20)           // Production: tune based on DB instance size
    .min_connections(2)            // Keep warm connections ready
    .acquire_timeout(Duration::from_secs(5))
    .idle_timeout(Duration::from_secs(300))
    .max_lifetime(Duration::from_secs(1800))
    .connect(&config.database_url)
    .await?;
```

The pool is injected into all route handlers via axum `State<Arc<AppState>>`. `AppState` is defined in this story and extended by subsequent stories.

#### Graceful Shutdown

The service must handle `SIGTERM` and `SIGINT`. On receiving either signal:

1. Stop accepting new connections (axum's `serve_with_incoming_shutdown`).
2. Wait up to 30 seconds for in-flight requests to complete.
3. Flush OpenTelemetry spans and metrics.
4. Close the database connection pool.
5. Exit with code 0.

If in-flight requests do not complete within 30 seconds, exit with code 1 and log a warning.

#### Acceptance Criteria

- `services/api` starts in the Docker Compose environment, passes `/readyz` (PostgreSQL connectivity check), and accepts HTTP requests.
- A `GET /` request returns `404 Not Found` with the standard error JSON body (not axum's default plain-text 404).
- Every request generates a `X-Request-ID` header in the response. If the client sends `X-Request-ID`, the same value is reflected in the response.
- Every request produces a structured JSON log line containing: `request_id`, `method`, `path`, `status`, `latency_ms`.
- A request to any `/v1/*` endpoint returns `501 Not Implemented` with error body `{"error": {"code": "NOT_IMPLEMENTED", "message": "This endpoint is not yet available"}}` (placeholder until S-016 and S-017 add real handlers).
- Sending `SIGTERM` to the process while a slow request is in-flight completes the in-flight request before shutting down.
- `AppState` struct is defined and documented. It holds: `PgPool`, `Arc<ApiConfig>`. It derives `Clone` (cheaply, because all fields are reference-counted).
- `cargo test -p platform-api` passes all unit tests for middleware logic.

#### Technical Notes

**CORS configuration:**

```rust
let cors = CorsLayer::new()
    .allow_origin(config.cors_origins.iter().map(|o| o.as_str().parse().unwrap()))
    .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE, Method::OPTIONS])
    .allow_headers([AUTHORIZATION, CONTENT_TYPE, ACCEPT, header::HeaderName::from_static("x-request-id")])
    .allow_credentials(true)
    .max_age(Duration::from_secs(3600));
```

**Panic catcher:**

Use `tower_http::catch_panic::CatchPanicLayer` with a custom response builder that returns a JSON error body. Never let a panic surface as a 500 with no body or as a plain-text response.

**Error response format (standard across all services):**

```json
{
  "error": {
    "code": "VALIDATION_ERROR",
    "message": "Human-readable description",
    "request_id": "uuid-v4",
    "details": {}
  }
}
```

This format is defined in `libs/mcp-common` as a `ApiErrorResponse` struct with a blanket `IntoResponse` implementation for `AppError`.

**Router structure:**

```rust
let app = Router::new()
    .route("/healthz", get(health::liveness))
    .route("/readyz", get(health::readiness))
    .route("/metrics", get(metrics::handler))
    .nest("/v1", v1_router())         // All business routes go here
    .fallback(handlers::not_found)    // JSON 404 for unknown routes
    .layer(middleware_stack);

fn v1_router() -> Router<AppState> {
    Router::new()
        .route("/users/me", get(users::me))
        // S-016: .nest("/servers", servers_router())
        // S-011: .route("/webhooks/clerk", post(webhooks::clerk))
}
```

---

### S-011: Clerk Authentication Integration

**Story ID:** S-011
**Epic:** EPIC-01
**Priority:** P0
**Estimated Effort:** 5 points
**Dependencies:** S-010, S-003

#### Description

Integrate Clerk as the platform authentication provider. This story has two components:

1. **JWT validation middleware** — every request to `/v1/*` (except `/v1/webhooks/clerk`) must carry a valid Clerk session JWT in the `Authorization: Bearer <token>` header. The middleware validates the JWT cryptographically (RS256, using Clerk's JWKS endpoint), extracts the `sub` (Clerk user ID) and `email` claims, and injects an `AuthenticatedUser` struct into the request extensions.

2. **User sync** — the first time a Clerk user authenticates against the API, their record must be created in the local `users` table. Clerk webhooks handle async user lifecycle events (creation, update, deletion).

#### JWT Validation Middleware

The middleware must:

1. Extract the `Authorization` header. Return `401` if absent or not in `Bearer <token>` format.
2. Fetch Clerk's JWKS from `https://api.clerk.dev/v1/.well-known/jwks.json`. Cache the JWKS in memory for 5 minutes (refresh on cache miss for unknown `kid`).
3. Validate the JWT: signature (RS256), `exp` (not expired), `iss` (matches Clerk frontend API URL), `nbf` (not before, if present).
4. On successful validation, extract `sub` (Clerk user ID) and `email` claims.
5. Look up the local `users` table by `clerk_id = sub`. If not found, create the user record (upsert). Return `AuthenticatedUser { id: Uuid, clerk_id: String, email: String }`.
6. Insert `AuthenticatedUser` into axum request extensions. Route handlers access it via `Extension<AuthenticatedUser>`.
7. On any failure (missing token, invalid JWT, expired JWT, DB error): return `401 Unauthorized` with standard error body. Log the failure reason at `warn` level with `request_id`. Do not log the token value.

#### Clerk Webhook Handler

Endpoint: `POST /v1/webhooks/clerk`

Clerk sends webhook events when users are created, updated, or deleted on the Clerk side. This endpoint must:

1. Verify the webhook signature using Svix (Clerk uses Svix for webhook delivery). The `CLERK_WEBHOOK_SECRET` env var is the signing secret. Return `400 Bad Request` if the signature is invalid.
2. Parse the event type from the JSON body.
3. Handle the following event types:
   - `user.created`: Upsert the user record in the local `users` table.
   - `user.updated`: Update `email` in the local `users` table.
   - `user.deleted`: Soft-delete or hard-delete the user. For MVP: hard delete with `ON DELETE CASCADE` to clean up all associated servers and credentials.
4. Return `200 OK` within 5 seconds (Clerk retries webhooks for non-2xx responses).
5. This endpoint is excluded from the JWT authentication middleware (it authenticates via Svix signature instead).

#### User Sync on First Login

When the JWT middleware creates a new user (step 5 above), use an `INSERT INTO users ... ON CONFLICT (clerk_id) DO UPDATE SET email = EXCLUDED.email, updated_at = NOW()` upsert. This handles race conditions where two concurrent requests from the same new user both attempt to create the user record.

#### Acceptance Criteria

- `GET /v1/users/me` with a valid Clerk JWT returns `200 OK` with body `{"id": "uuid", "email": "user@example.com", "created_at": "..."}`.
- `GET /v1/users/me` with no `Authorization` header returns `401 Unauthorized` with code `UNAUTHORIZED`.
- `GET /v1/users/me` with an expired JWT returns `401 Unauthorized` with code `TOKEN_EXPIRED`.
- `GET /v1/users/me` with a tampered JWT (modified payload, valid structure) returns `401 Unauthorized`.
- A JWKS cache hit does not make a network call to Clerk. A cache miss fetches and caches the JWKS.
- `POST /v1/webhooks/clerk` with a valid `user.created` event upserts the user in the local database.
- `POST /v1/webhooks/clerk` with an invalid Svix signature returns `400 Bad Request`.
- `POST /v1/webhooks/clerk` with a `user.deleted` event removes the user and all their associated servers and credentials from the database (cascading delete).
- Integration tests cover all acceptance criteria above using a real `TestDatabase` and a mock Clerk JWKS server (mock the JWKS endpoint via `MockUpstream`, serve a test RSA key pair, sign test JWTs with the private key).
- The JWT validation middleware adds `clerk_id` and `user_id` as fields to the active tracing span.

#### Technical Notes

**JWKS caching:**

```rust
pub struct JwksCache {
    jwks: Arc<RwLock<Option<CachedJwks>>>,
    clerk_api_url: String,
}

struct CachedJwks {
    keys: Vec<Jwk>,
    fetched_at: Instant,
}

impl JwksCache {
    pub async fn get_key(&self, kid: &str) -> Result<DecodingKey, AuthError> {
        // Try cache first. If cache is stale (>5 min) or key not found, fetch fresh.
    }
}
```

Use the `jsonwebtoken` crate for JWT decoding and the `reqwest` client (shared with other outgoing calls, not a new client per request) for JWKS fetching.

**Svix webhook verification:**

Use the `svix` Rust crate. The verification requires the raw request body (before JSON parsing) and the Svix-specific headers (`svix-id`, `svix-timestamp`, `svix-signature`). In axum, extract the body as `Bytes` before consuming it as JSON.

**User upsert query (sqlx):**

```sql
INSERT INTO users (clerk_id, email)
VALUES ($1, $2)
ON CONFLICT (clerk_id)
DO UPDATE SET email = EXCLUDED.email, updated_at = NOW()
RETURNING id, clerk_id, email, created_at, updated_at
```

---

### S-012: Credential Encryption Service

**Story ID:** S-012
**Epic:** EPIC-01
**Priority:** P0
**Estimated Effort:** 3 points
**Dependencies:** S-004 (libs/mcp-crypto), S-003

#### Description

Implement the application-layer service that wraps `libs/mcp-crypto` and provides the `services/api` and `services/credential-injector` with clean interfaces for storing and retrieving encrypted credentials. This story is about the database access layer, not the encryption primitives themselves (those are in S-004).

This service enforces one invariant above all others: **a credential value that enters the system in plaintext must never leave the system in plaintext through any code path except the credential injector's injection step.** The API never returns credential values. The audit log never contains credential values. Error messages never contain credential values.

#### Credential Lifecycle

```
User provides plaintext credential via API
  |
  v
[services/api] CredentialService::store(server_id, cred_type, plaintext)
  |
  v
[libs/mcp-crypto] encrypt(key, plaintext) -> EncryptedValue { ciphertext, iv }
  |
  v
INSERT INTO credentials (server_id, credential_type, encrypted_value, iv)
  |
  v
Plaintext is dropped from memory (zeroize on drop for the input buffer)

--- Later, during an MCP tool call ---

[services/credential-injector] InjectorService::inject(server_id, request_skeleton)
  |
  v
SELECT encrypted_value, iv FROM credentials WHERE server_id = $1
  |
  v
[libs/mcp-crypto] decrypt(key, EncryptedValue) -> plaintext
  |
  v
Inject plaintext into upstream HTTP request Authorization header
  |
  v
Plaintext is dropped from memory after the HTTP request is sent
```

#### CredentialService API (in `services/api`)

```rust
pub struct CredentialService {
    pool: sqlx::PgPool,
    crypto_key: mcp_crypto::CryptoKey,
}

impl CredentialService {
    /// Store a new credential. Returns the Credential record (without the plaintext value).
    /// The plaintext Vec<u8> is zeroized after encryption.
    pub async fn store(
        &self,
        server_id: Uuid,
        credential_type: CredentialType,
        key_name: Option<&str>,
        plaintext: &mut Vec<u8>,   // mut to allow zeroization
    ) -> Result<Credential, AppError>;

    /// Rotate a credential (replace ciphertext in-place with new plaintext).
    /// Atomically updates encrypted_value, iv, and rotated_at in a single UPDATE.
    pub async fn rotate(
        &self,
        credential_id: Uuid,
        server_id: Uuid,  // for ownership check
        new_plaintext: &mut Vec<u8>,
    ) -> Result<Credential, AppError>;

    /// Delete a credential. Called when a server is deleted or user explicitly removes it.
    pub async fn delete(&self, credential_id: Uuid, server_id: Uuid) -> Result<(), AppError>;

    /// List credential metadata for a server (never returns plaintext or ciphertext).
    /// Returns: id, credential_type, key_name, created_at, rotated_at.
    pub async fn list_for_server(&self, server_id: Uuid) -> Result<Vec<CredentialMeta>, AppError>;
}
```

`CredentialMeta` is a separate struct that explicitly omits `encrypted_value` and `iv`. It cannot be constructed from the `Credential` row directly without an explicit field extraction step — this makes it impossible to accidentally include ciphertext in an API response.

#### Key Rotation Support

The encryption key is loaded from `MCP_ENCRYPTION_KEY` at startup. When the key needs to be rotated (e.g., after a security incident or scheduled rotation):

1. A new key is generated and set as `MCP_ENCRYPTION_KEY_NEW`.
2. A migration job (a separate Rust binary `credential-rotator`) reads all credentials, decrypts with the old key, re-encrypts with the new key, and writes back in a transaction.
3. After the migration job completes, `MCP_ENCRYPTION_KEY` is updated to the new value and `MCP_ENCRYPTION_KEY_NEW` is removed.
4. Services are restarted to pick up the new key.

The `CredentialService` supports key rotation by accepting an optional `rotation_key: Option<&CryptoKey>`. When present, credentials successfully decrypted with the primary key are ignored; the service re-encrypts them with the primary key after decrypting with the rotation key. This enables a live rotation without downtime for reads (injector still decrypts with old key while rotation runs).

#### Acceptance Criteria

- `CredentialService::store` encrypts the plaintext and inserts a row with non-null `encrypted_value` and `iv`.
- After `store`, `SELECT encrypted_value FROM credentials WHERE id = $1` returns a non-empty byte sequence that is not equal to the original plaintext (verified in integration test by raw DB query).
- `CredentialService::list_for_server` does not return any field named `encrypted_value`, `iv`, or `value`. Verified by attempting to access `.encrypted_value` on the return type (compile-time check).
- `CredentialService::rotate` updates `rotated_at` and changes the `encrypted_value` and `iv` columns. The new ciphertext decrypts to the new plaintext.
- Calling `CredentialService::rotate` with a `server_id` that does not own the `credential_id` returns `Err(AppError::Forbidden)`.
- The `CredentialService` writes an audit log entry (`action = "credential_create"` or `"credential_rotate"`) for every store and rotate operation.
- `cargo test -p platform-api` includes tests for all four `CredentialService` methods using a `TestDatabase`.

#### Technical Notes

- The `mcp_crypto::CryptoKey` is loaded once at service startup and shared via `Arc<CryptoKey>` in `AppState`. It is not reloaded at runtime.
- Use `zeroize::Zeroizing<Vec<u8>>` for the `plaintext` parameter type rather than `&mut Vec<u8>` — this provides automatic zeroing on drop at the type level.
- The rotation job binary is out of scope for MVP but the `CredentialService` API must be designed to support it (the `rotation_key` parameter placeholder is sufficient for now).
- Audit log writes in `CredentialService` use a fire-and-forget pattern: log write failures are logged at `error` level but do not fail the primary operation. The audit log is best-effort for MVP; make it transactional in a later story.

---

### S-013: Credential Injector Sidecar

**Story ID:** S-013
**Epic:** EPIC-01
**Priority:** P0
**Estimated Effort:** 8 points
**Dependencies:** S-010, S-012, S-003, S-006

#### Description

Implement the `services/credential-injector` as a fully independent Rust binary and service. The credential injector is the most security-critical component in the system. Its sole purpose is to receive "request skeletons" from the gateway (which describe an HTTP request without auth), decrypt the appropriate credential from the database, inject the credential into the request, execute the HTTP request against the upstream API, and return the response.

**The gateway must NEVER hold raw credentials in memory.** This design means a memory disclosure vulnerability in the gateway exposes only the request skeleton (URL, method, body), not the credential. The credential injector has a much smaller attack surface (no MCP protocol handling, no multi-tenant routing, no business logic) and is the only process that holds decrypted credentials — and only for the duration of a single request.

#### Request Skeleton Protocol

The gateway sends the following JSON body to the injector over HTTP (internal network only):

```json
{
  "request_id": "uuid-v4",
  "server_id": "uuid-v4",
  "upstream_url": "https://api.stripe.com/v1/customers/cus_123",
  "method": "GET",
  "headers": {
    "Accept": "application/json",
    "Content-Type": "application/json"
  },
  "body": null,
  "timeout_seconds": 30
}
```

The injector:
1. Validates that `upstream_url` passes SSRF validation (post-DNS-resolution check — see S-014). The gateway also checks, but the injector validates independently as defense-in-depth.
2. Looks up the credential for `server_id` from the database (cached in the injector's in-process LRU cache for up to 60 seconds).
3. Decrypts the credential using `mcp-crypto`.
4. Injects the credential into the request (header, query parameter, or basic auth, based on the stored `credential_type`).
5. Executes the upstream HTTP request using `reqwest`.
6. Returns the upstream response (status code, headers, body) to the gateway.
7. Zeroizes the decrypted credential from memory immediately after the HTTP request is sent.

#### Injector HTTP API

The injector exposes a single endpoint:

```
POST /inject
Content-Type: application/json
X-Request-ID: <uuid>

Body: RequestSkeleton (see above)

Response: 200 OK
{
  "status": 200,
  "headers": {"Content-Type": "application/json"},
  "body": "{...upstream response body...}"
}

Or on error:
{
  "error": {
    "code": "CREDENTIAL_NOT_FOUND",
    "message": "No credential configured for this server",
    "request_id": "uuid"
  }
}
```

HTTP status codes returned by the injector to the gateway:
- `200` — upstream request completed (regardless of upstream status code; the upstream status is in the response body)
- `400` — invalid request skeleton (malformed JSON, missing fields)
- `403` — caller IP is not in `MCP_INJECTOR_ALLOWED_CALLER_IPS` allowlist
- `404` — no credential found for `server_id`
- `422` — SSRF blocked (URL resolves to private IP)
- `500` — internal error (decrypt failure, DB error, upstream connection refused)
- `504` — upstream timeout

#### Caller Authentication

The injector only accepts requests from known gateway IP addresses (`MCP_INJECTOR_ALLOWED_CALLER_IPS`). In Docker Compose and production, this is the gateway container's IP. The check is performed before any credential database access.

In addition, a shared secret (`MCP_INJECTOR_SHARED_SECRET`) is required as a Bearer token in the `Authorization` header of every request from the gateway to the injector. This provides a second layer of authentication in case the IP allowlist is misconfigured (e.g., in cloud environments where IP addresses change).

#### In-Process Credential Cache

The injector caches decrypted... no. The injector caches **encrypted** credentials in an LRU cache (keyed by `server_id`). Decryption happens on every request from the cached ciphertext. This means:

- Cache hit: DB query avoided. Decryption cost: ~1ms (AES-256-GCM is fast).
- Cache miss: DB query + decryption.
- A credential rotation invalidates the cache entry for that `server_id` (see S-012 for rotation flow).

Never cache decrypted credentials. Decrypted credentials in memory for more than the duration of a single request is a security vulnerability.

Cache invalidation: the injector subscribes to PostgreSQL LISTEN/NOTIFY on channel `credential_updated`. When `services/api` updates a credential, it sends `NOTIFY credential_updated, '<server_id>'`. The injector receives the notification and evicts the cache entry for that `server_id`.

#### Acceptance Criteria

- The injector starts and passes `/healthz` and `/readyz` (PostgreSQL connectivity).
- `POST /inject` with a valid request skeleton for a server with a stored Bearer token credential returns a response body containing the upstream API's response.
- `POST /inject` from a caller IP not in `MCP_INJECTOR_ALLOWED_CALLER_IPS` returns `403 Forbidden`.
- `POST /inject` with an invalid `Authorization: Bearer` shared secret returns `403 Forbidden`.
- `POST /inject` for a `server_id` with no stored credential returns `404 Not Found`.
- `POST /inject` with an `upstream_url` that resolves to `192.168.1.1` returns `422` with error code `SSRF_BLOCKED`.
- After a credential is rotated (S-012), the next `POST /inject` for that server uses the new credential (cache is invalidated via NOTIFY).
- The injector emits an audit log entry for every inject call: `action = "credential_access"`, `server_id`, `success` (true/false), `credential_type`. The audit log row is written after the upstream HTTP call completes (not before).
- The decrypted credential value does not appear in: structured logs, tracing spans, error messages, or the response body under any code path. Verified by code review and by grepping log output in integration tests.
- Integration test: gateway sends a `RequestSkeleton`, injector injects a Bearer token credential, `MockUpstream` asserts it received the `Authorization: Bearer <expected_token>` header.
- `cargo test -p credential-injector` passes all unit and integration tests.

#### Technical Notes

**LRU cache for encrypted credentials:**

Use the `lru` crate. Cache size: 10,000 entries (each entry is ~200 bytes of ciphertext + IV; 10k entries = ~2MB). Set `max_capacity` to match expected peak server count per injector instance.

```rust
pub struct CredentialCache {
    inner: Arc<Mutex<LruCache<Uuid, EncryptedCredential>>>,
}
```

Do not use `tokio::sync::Mutex` for the LRU cache — the cache operations are synchronous and short. Use `std::sync::Mutex` and avoid holding the lock across `.await` points.

**Upstream HTTP client:**

The `reqwest::Client` is shared across all requests (one client per injector process). Configure it with:
- `connection_pool_max_idle_per_host: 10`
- `timeout: Duration::from_secs(config.max_upstream_timeout_secs)`
- `danger_accept_invalid_certs: false` (never disable TLS verification)
- Custom `User-Agent: mcp-gateway/0.1.0`
- Redirect policy: `redirect::Policy::limited(5)` (follow up to 5 redirects)

**Credential injection by type:**

```rust
match credential.credential_type {
    CredentialType::BearerToken => {
        headers.insert(AUTHORIZATION, format!("Bearer {}", plaintext).parse()?);
    }
    CredentialType::ApiKeyHeader => {
        let header_name = credential.key_name.as_deref().unwrap_or("X-API-Key");
        headers.insert(HeaderName::from_str(header_name)?, plaintext.parse()?);
    }
    CredentialType::ApiKeyQuery => {
        let key_name = credential.key_name.as_deref().unwrap_or("api_key");
        url.query_pairs_mut().append_pair(key_name, &plaintext);
    }
    CredentialType::BasicAuth => {
        // plaintext is "username:password"
        let encoded = BASE64.encode(plaintext.as_bytes());
        headers.insert(AUTHORIZATION, format!("Basic {}", encoded).parse()?);
    }
}
// Zeroize plaintext immediately after all injection is complete
plaintext.zeroize();
```

---

### S-014: SSRF Protection Module

**Story ID:** S-014
**Epic:** EPIC-01
**Priority:** P0
**Estimated Effort:** 5 points
**Dependencies:** S-004 (mcp-common error types)

#### Description

Implement a standalone SSRF (Server-Side Request Forgery) protection module in `libs/common/src/ssrf.rs`. This module is the security gatekeeper for every outgoing HTTP request in the system. Both the gateway (when the user configures an upstream URL) and the credential injector (before executing the upstream request) must call this module. The module must be correct, not convenient.

SSRF is the highest-severity vulnerability class for a platform that proxies HTTP requests on behalf of users. A successful SSRF attack allows an attacker to:
- Access cloud provider metadata endpoints (AWS: `169.254.169.254`, GCP: `metadata.google.internal`, Azure: `169.254.169.254`) and steal IAM credentials.
- Access internal services (PostgreSQL, Redis, internal APIs) on the same network as the gateway.
- Enumerate internal network topology.

This module prevents all known SSRF vectors.

#### SSRF Validation Rules

The module implements two validation phases:

**Phase 1: URL-level validation (before DNS resolution)**

Reject if:
- URL scheme is not `https` or `http`. Reject `file://`, `ftp://`, `gopher://`, `dict://`, etc.
- Hostname is a bare IP address in a blocked range (see Phase 2 for ranges). This handles `http://192.168.1.1/` without DNS.
- Hostname is `localhost`, `127.0.0.1`, `::1`, `0.0.0.0`, or any variation.
- Hostname is a known cloud metadata hostname: `metadata.google.internal`, `instance-data.ec2.internal`, `link-local.metadata.internal`, `metadata.azure.com`.
- Port is non-standard and in a suspicious range: 22 (SSH), 23 (Telnet), 25 (SMTP), 3306 (MySQL), 5432 (PostgreSQL), 6379 (Redis). Standard ports (80, 443, 8080, 8443) are allowed.

**Phase 2: Post-DNS-resolution validation (the critical defense against DNS rebinding)**

DNS rebinding attack: attacker registers `evil.attacker.com` which initially resolves to a public IP (passes Phase 1), but immediately before the request is made, the DNS TTL expires and the attacker changes the DNS record to `192.168.1.1`. The request then hits the internal service.

Defense: resolve the hostname to IP addresses, then validate EVERY resolved IP against the blocklist. If any resolved IP is blocked, reject the request.

Blocked IP ranges:
- `0.0.0.0/8` — "This" network
- `10.0.0.0/8` — RFC 1918 private
- `100.64.0.0/10` — Carrier-grade NAT (CGNAT)
- `127.0.0.0/8` — Loopback
- `169.254.0.0/16` — Link-local (AWS/GCP metadata endpoint lives here)
- `172.16.0.0/12` — RFC 1918 private
- `192.0.0.0/24` — IETF Protocol Assignments
- `192.168.0.0/16` — RFC 1918 private
- `198.18.0.0/15` — Network Interconnect Device Benchmark Testing
- `198.51.100.0/24` — TEST-NET-2
- `203.0.113.0/24` — TEST-NET-3
- `224.0.0.0/4` — Multicast
- `240.0.0.0/4` — Reserved
- `255.255.255.255/32` — Broadcast
- `::1/128` — IPv6 loopback
- `fc00::/7` — IPv6 unique-local (RFC 1918 equivalent)
- `fe80::/10` — IPv6 link-local

#### Module API

```rust
/// Validate a URL before making an outgoing HTTP request.
/// Phase 1 (URL-level) only. Call this when the user first configures a URL.
/// Fast (no network I/O). Returns Ok(()) or Err(AppError::SsrfBlocked).
pub fn validate_url(url: &Url) -> Result<(), AppError>;

/// Validate a URL including post-DNS-resolution IP check.
/// Phase 1 + Phase 2. Call this immediately before executing the HTTP request.
/// Requires DNS resolution (async, ~1-10ms).
/// Returns Ok(()) or Err(AppError::SsrfBlocked).
pub async fn validate_url_with_dns(url: &Url) -> Result<(), AppError>;

/// Check if an IP address is in any blocked range.
/// Exposed publicly for testing and for the credential injector's independent check.
pub fn is_blocked_ip(ip: IpAddr) -> bool;

/// Configurable allowlist for testing environments.
/// WARNING: Only use in tests. Never enable in production.
pub struct SsrfAllowlist(Vec<IpAddr>);

impl SsrfAllowlist {
    pub fn allow(&self, ip: &IpAddr) -> bool;
}
```

#### Acceptance Criteria

- `validate_url("http://192.168.1.1/api")` returns `Err(AppError::SsrfBlocked)`.
- `validate_url("http://10.0.0.1/api")` returns `Err(AppError::SsrfBlocked)`.
- `validate_url("http://169.254.169.254/latest/meta-data/")` returns `Err(AppError::SsrfBlocked)`.
- `validate_url("http://metadata.google.internal/computeMetadata/v1/")` returns `Err(AppError::SsrfBlocked)` at Phase 1 (hostname match).
- `validate_url("https://api.stripe.com/v1/customers")` returns `Ok(())`.
- `validate_url("file:///etc/passwd")` returns `Err(AppError::SsrfBlocked)`.
- `validate_url_with_dns` for a hostname that resolves to a private IP returns `Err(AppError::SsrfBlocked)` even if Phase 1 passed.
- `is_blocked_ip(IpAddr::from([192, 168, 1, 1]))` returns `true`.
- `is_blocked_ip(IpAddr::from([8, 8, 8, 8]))` returns `false`.
- Unit tests cover every blocked CIDR range with at least one test address per range.
- Unit tests include DNS rebinding simulation: a mock DNS resolver that returns a private IP for a public hostname.
- `validate_url` executes in under 1ms (no I/O). `validate_url_with_dns` completes in under 50ms for a hostname with a standard TTL.
- The module is imported and called in: `services/api` (on URL save), `services/gateway` (on URL config load), `services/credential-injector` (before executing upstream request). All three call sites are verified in integration tests.
- `SsrfBlocked` errors are logged at `warn` level with the blocked URL and the resolved IP (if Phase 2). The blocked URL in the log must not include query parameters that could contain credentials (use `RedactedUrl` from S-005).

#### Technical Notes

- Use `tokio::net::lookup_host` for DNS resolution. It respects the system resolver configuration and is cancellable.
- The DNS resolution step must have a timeout (default: 5 seconds, configurable via `MCP_SSRF_DNS_TIMEOUT_SECS`). A DNS resolution that hangs is a DoS vector.
- After DNS resolution, connect to the resolved IP directly (bypass DNS for the actual HTTP connection). This is done by using `reqwest`'s `resolve_to_addrs` feature or by building a custom `Connect` implementation that uses the pre-resolved IPs. Without this, a second DNS resolution could return a different IP (DNS rebinding).
- The `ipnetwork` crate provides CIDR matching. Use `IpNetwork::contains(ip)` for each blocked range.

---

### S-015: Audit Logging Service

**Story ID:** S-015
**Epic:** EPIC-01
**Priority:** P0
**Estimated Effort:** 3 points
**Dependencies:** S-003 (audit_log table), S-004 (mcp-common)

#### Description

Implement an async audit log writer that all services use to write structured security events to the `audit_log` table. The audit log is a security control, not an observability tool — it records who did what to which resource, and every security-sensitive operation must emit an audit event. The writer is async and non-blocking: audit log writes must not slow down or fail primary operations.

This story defines the audit event taxonomy for MVP, implements the async writer, and integrates it into all three services.

#### Audit Event Types (MVP)

```rust
pub enum AuditAction {
    // Authentication
    AuthSuccess,           // User authenticated (JWT validated)
    AuthFailure,           // JWT validation failed
    WebhookAuthFailure,    // Clerk webhook signature invalid

    // Credential operations (highest security sensitivity)
    CredentialCreate,      // Credential stored for a server
    CredentialRotate,      // Credential replaced
    CredentialDelete,      // Credential removed
    CredentialAccess,      // Credential decrypted for injection (by credential-injector)
    CredentialAccessFailure, // Decryption failed (wrong key, corruption)

    // Security blocks
    SsrfBlock,             // Outgoing URL blocked by SSRF module
    RateLimitExceeded,     // Rate limit triggered for a server or user

    // Server lifecycle (admin actions)
    ServerCreate,
    ServerUpdate,
    ServerDelete,
    ServerTokenGenerate,
    ServerTokenRevoke,
}
```

#### AuditLogger API

```rust
pub struct AuditLogger {
    pool: sqlx::PgPool,
    sender: tokio::sync::mpsc::Sender<AuditEvent>,
}

pub struct AuditEvent {
    pub action: AuditAction,
    pub user_id: Option<Uuid>,
    pub server_id: Option<Uuid>,
    pub success: bool,
    pub error_msg: Option<String>,       // Sanitized — must not contain credentials
    pub metadata: Option<serde_json::Value>,
    pub correlation_id: Option<String>,  // X-Request-ID from the request context
}

impl AuditLogger {
    /// Initialize the logger. Spawns a background task that drains the channel
    /// and writes audit events to the database in batches.
    pub fn new(pool: sqlx::PgPool) -> Self;

    /// Enqueue an audit event. Non-blocking. Returns immediately.
    /// If the channel is full (back-pressure), logs a warning and drops the event.
    /// Dropping an audit event is preferable to blocking the primary operation.
    pub fn log(&self, event: AuditEvent);
}
```

The background task batches events: it waits up to 100ms or until 50 events accumulate, then inserts them in a single `INSERT INTO audit_log ... VALUES ($1, $2, ...), ($3, $4, ...)` batch statement. Batching reduces DB write pressure under high load.

#### Acceptance Criteria

- `AuditLogger::log` returns immediately (does not block the caller). Verified by measuring call latency: under 1ms for 99th percentile.
- Events enqueued via `AuditLogger::log` appear in the `audit_log` table within 200ms under normal load.
- `AuditLogger::log` for a `CredentialAccess` event must not include the credential value in `error_msg` or `metadata`. Verified by code review and by asserting in integration tests that the `metadata` JSON for such events contains only `server_id`, `credential_type`, and `success`.
- Under load test (1000 events/second for 10 seconds), fewer than 0.1% of events are dropped (channel overflow).
- When the service shuts down gracefully (S-010 graceful shutdown), the `AuditLogger` is given 5 seconds to drain remaining events before the process exits.
- Integration test: `CredentialAccess` events are written by the credential injector on every successful injection.
- Integration test: `SsrfBlock` events are written by the SSRF module when a URL is blocked.
- Integration test: `AuthFailure` events are written by the JWT middleware on failed authentication.
- The `audit_log` table is never modified by `UPDATE` or `DELETE` operations. Enforced by a PostgreSQL trigger that raises an exception on any `UPDATE` or `DELETE` on the `audit_log` table. The trigger definition is included in the migration for S-003.

#### Technical Notes

**Background writer task:**

```rust
tokio::spawn(async move {
    let mut batch = Vec::with_capacity(50);
    let mut interval = tokio::time::interval(Duration::from_millis(100));
    loop {
        tokio::select! {
            event = receiver.recv() => {
                match event {
                    Some(e) => {
                        batch.push(e);
                        if batch.len() >= 50 { flush(&pool, &mut batch).await; }
                    }
                    None => {
                        // Channel closed (shutdown). Flush remaining.
                        if !batch.is_empty() { flush(&pool, &mut batch).await; }
                        break;
                    }
                }
            }
            _ = interval.tick() => {
                if !batch.is_empty() { flush(&pool, &mut batch).await; }
            }
        }
    }
});
```

**Credential sanitization enforcement:**

Create a `SanitizedErrorMsg` newtype that implements `From<AppError>` and strips credential-related patterns. Require this type for `AuditEvent::error_msg`. This makes it a compile-time error to pass a raw `AppError` string as the error message.

**External sink (optional, post-MVP):**

The `AuditLogger` is designed for extensibility: the background task can fan out to multiple sinks (DB, S3, CloudWatch) by implementing an `AuditSink` trait. For MVP, only the DB sink is implemented. The trait is defined but not wired up.

---

### S-016: Server CRUD API

**Story ID:** S-016
**Epic:** EPIC-01
**Priority:** P0
**Estimated Effort:** 5 points
**Dependencies:** S-010, S-011, S-003, S-014, S-015

#### Description

Implement the REST API endpoints for managing MCP server configurations. These endpoints are the primary interface between the React frontend and the backend. They enforce ownership (users can only CRUD their own servers), validate inputs, generate server slugs, and trigger audit log events.

#### API Endpoints

```
POST   /v1/servers           — Create a new MCP server
GET    /v1/servers           — List caller's MCP servers (paginated)
GET    /v1/servers/{id}      — Get a single MCP server by ID
PUT    /v1/servers/{id}      — Update an MCP server (full update, not patch)
DELETE /v1/servers/{id}      — Delete an MCP server (and all credentials, tokens)
POST   /v1/servers/{id}/validate-url  — Validate an upstream URL (SSRF check + DNS)
```

#### Request/Response Schemas

**`POST /v1/servers` — Create:**

```json
Request:
{
  "display_name": "Stripe Customer API",
  "description": "Exposes Stripe customer lookup as MCP tools",
  "config": {
    "tools": [],
    "upstream_base_url": "https://api.stripe.com"
  }
}

Response 201:
{
  "id": "uuid",
  "slug": "stripe-customer-api-a1b2",
  "display_name": "Stripe Customer API",
  "description": "...",
  "config": { "tools": [], "upstream_base_url": "https://api.stripe.com" },
  "is_active": true,
  "created_at": "2026-03-28T12:00:00Z",
  "updated_at": "2026-03-28T12:00:00Z",
  "mcp_url": "https://gateway.example.com/mcp/stripe-customer-api-a1b2"
}
```

**`GET /v1/servers` — List (paginated):**

```json
Response 200:
{
  "data": [...],
  "pagination": {
    "total": 42,
    "page": 1,
    "per_page": 20,
    "has_next": true
  }
}
```

Pagination: cursor-based using `before` / `after` query parameters (opaque cursors encoding `created_at` + `id`). Do not use offset-based pagination — it is unreliable under concurrent inserts.

**`POST /v1/servers/{id}/validate-url` — SSRF validation:**

```json
Request: { "url": "https://api.stripe.com/v1/customers" }
Response 200: { "valid": true }
Response 422: {
  "valid": false,
  "error": { "code": "SSRF_BLOCKED", "message": "URL resolves to a private IP address" }
}
```

#### Slug Generation

Server slugs are the URL path segment for the MCP endpoint (`/mcp/{slug}`). Rules:
- Derived from `display_name`: lowercase, spaces to hyphens, strip non-alphanumeric except hyphens.
- Maximum 50 characters.
- Suffix with 4 random alphanumeric characters to ensure uniqueness (e.g., `stripe-customer-api-a1b2`).
- If the generated slug is already taken (unique constraint violation), regenerate with a new suffix. Retry up to 5 times before returning a 500.

#### Ownership Enforcement

Every endpoint that operates on a specific server (`GET`, `PUT`, `DELETE /v1/servers/{id}`) must verify that the server's `user_id` matches the authenticated user's `id`. Return `404 Not Found` (not `403 Forbidden`) when the server does not belong to the caller — this prevents enumeration attacks (callers cannot distinguish "does not exist" from "exists but not yours").

#### Acceptance Criteria

- `POST /v1/servers` creates a server and returns `201 Created` with the server JSON including a generated `slug` and `mcp_url`.
- `GET /v1/servers` returns only servers owned by the authenticated caller. Calling with User A's token does not return User B's servers.
- `GET /v1/servers/{id}` for a server owned by another user returns `404 Not Found` (not `403`).
- `DELETE /v1/servers/{id}` deletes the server, all its credentials, and all its tokens (cascade enforced by DB, but the API confirms deletion by verifying the row is gone).
- `POST /v1/servers` with `display_name` exceeding 100 characters returns `400 Bad Request` with error code `VALIDATION_ERROR`.
- `POST /v1/servers` with an `upstream_base_url` that fails SSRF Phase 1 validation returns `422 Unprocessable Entity` with error code `SSRF_BLOCKED`.
- `GET /v1/servers` supports cursor pagination. Requesting page 2 returns the next 20 servers. The `has_next` field correctly indicates whether more results exist.
- Every `POST`, `PUT`, and `DELETE` operation writes an audit log event (`ServerCreate`, `ServerUpdate`, `ServerDelete`).
- Integration tests cover all endpoints with ownership enforcement scenarios (User A cannot access User B's servers).

#### Technical Notes

**Config JSONB validation:**

The `config` field is stored as JSONB but validated in Rust via a `ServerConfig` struct before storage. Unknown fields in `config` return `400 Bad Request` with `VALIDATION_ERROR` in tests (use `serde(deny_unknown_fields)` in the input type). In production deserialization (reading from DB), use `serde(default)` to handle forward-compatible schema evolution.

**`mcp_url` generation:**

```rust
fn mcp_url(gateway_base_url: &str, slug: &str) -> String {
    format!("{}/mcp/{}", gateway_base_url, slug)
}
```

`gateway_base_url` comes from `ApiConfig.gateway_base_url` env var.

---

### S-017: Credential CRUD API

**Story ID:** S-017
**Epic:** EPIC-01
**Priority:** P0
**Estimated Effort:** 5 points
**Dependencies:** S-016, S-012, S-015

#### Description

Implement the REST API endpoints for managing credentials attached to MCP servers. Credentials are write-once (create, rotate, delete). They are NEVER returned in plaintext after creation. The "create" response includes a `hint` field (last 4 characters of the credential value) so users can verify which credential is stored, but not reconstruct it.

#### API Endpoints

```
POST   /v1/servers/{server_id}/credentials          — Store a new credential
GET    /v1/servers/{server_id}/credentials          — List credentials (metadata only, no values)
PUT    /v1/servers/{server_id}/credentials/{id}     — Rotate (replace) a credential
DELETE /v1/servers/{server_id}/credentials/{id}     — Delete a credential

POST   /v1/servers/{server_id}/tokens               — Generate a new server token
GET    /v1/servers/{server_id}/tokens               — List tokens (hint only, not raw token)
DELETE /v1/servers/{server_id}/tokens/{id}          — Revoke a token
```

#### Request/Response Schemas

**`POST /v1/servers/{server_id}/credentials` — Create:**

```json
Request:
{
  "credential_type": "bearer_token",
  "value": "sk_live_abc123xyz...",
  "key_name": null
}

Response 201:
{
  "id": "uuid",
  "server_id": "uuid",
  "credential_type": "bearer_token",
  "key_name": null,
  "hint": "...xyz",
  "created_at": "2026-03-28T12:00:00Z",
  "rotated_at": null
}
```

Note: `value` is present ONLY in the request. It is NEVER included in any response.

**`POST /v1/servers/{server_id}/tokens` — Generate token:**

```json
Response 201:
{
  "id": "uuid",
  "token": "mcp_live_abc123...xyz",   // ONLY returned on creation, never again
  "hint": "...xyz",
  "created_at": "2026-03-28T12:00:00Z"
}
```

The raw `token` value is returned only in the `201 Created` response. Subsequent `GET` requests return only the `hint`. If the user loses the token, they must revoke and regenerate.

#### Token Generation

Server tokens are the credentials MCP clients use to authenticate to the gateway. Format: `mcp_live_` prefix + 32 cryptographically random bytes (base62-encoded) = `mcp_live_` + ~43 chars. Total length: ~52 chars.

Storage:
1. Generate raw token using `rand::rngs::OsRng`.
2. Hash with SHA-256 (hex-encoded) and store in `server_tokens.token_hash`.
3. Store last 4 characters in `server_tokens.hint`.
4. Return the raw token in the `201` response only.

Verification (in the gateway's auth layer): take the `Authorization: Bearer mcp_live_...` value, SHA-256 hash it, and look up `server_tokens.token_hash WHERE revoked_at IS NULL`.

#### Ownership and Access Control

- Every credential and token endpoint must first verify that `server_id` belongs to the authenticated user (same ownership check as S-016).
- A `DELETE /v1/servers/{server_id}/credentials/{id}` where `id` does not belong to `server_id` returns `404`.

#### Acceptance Criteria

- `POST /v1/servers/{server_id}/credentials` stores an encrypted credential and returns `201` with the `hint` but not the `value`.
- `GET /v1/servers/{server_id}/credentials` returns a list of credentials with `hint` and metadata, but no `value`, `encrypted_value`, or `iv` fields. Verified by asserting the response JSON has no such keys.
- `PUT /v1/servers/{server_id}/credentials/{id}` updates the stored credential. After rotation, the credential injector uses the new value (cache invalidation via NOTIFY is verified in integration test).
- `DELETE /v1/servers/{server_id}/credentials/{id}` removes the row from the database.
- `POST /v1/servers/{server_id}/tokens` generates a token, returns it in the `201` response, and stores only the hash.
- `GET /v1/servers/{server_id}/tokens` returns tokens with `hint` but no raw `token` field.
- `DELETE /v1/servers/{server_id}/tokens/{id}` sets `revoked_at` and the token is no longer usable in the gateway.
- Credential `value` never appears in: response bodies, structured logs, error messages, or OpenTelemetry spans. Verified by asserting log output in integration tests.
- Every create, rotate, and delete operation writes an audit log event.
- Integration test: full end-to-end — create server, store credential, generate token, call the gateway with the token, verify the credential injector uses the stored credential (via `MockUpstream` header assertion).

#### Technical Notes

**Preventing double-exposure of `value`:**

Define a `CreateCredentialRequest` struct with `value: Zeroizing<String>`. After extracting the value for encryption, the `Zeroizing` wrapper ensures it is cleared from memory. Never create a response type that has a `value` field; the compiler will catch any accidental inclusion.

**Server token prefix:**

The `mcp_live_` prefix (inspired by Stripe's `sk_live_` pattern) helps users identify the token type and helps secret scanners detect accidentally-committed tokens. Add a GitHub secret scanning pattern for `mcp_live_[a-zA-Z0-9]{40,}` as a custom pattern in the repository settings.

---

### S-018: Rate Limiting Middleware — Token Bucket with Redis Backend

**Story ID:** S-018
**Epic:** EPIC-01
**Priority:** P1
**Estimated Effort:** 5 points
**Dependencies:** S-010, S-011, S-006

#### Description

Implement request rate limiting for the `services/gateway` (MCP tool calls) and `services/api` (management API calls). Rate limiting protects against:
- Abuse by a single user consuming all gateway capacity.
- Accidental misconfiguration causing infinite loops in AI agents.
- Upstream API hammering (users who connect a rate-limited upstream API and run an agent in a loop).

Two independent rate limits apply simultaneously:
- **Per-server:** 100 requests/minute per MCP server. Protects individual upstream APIs.
- **Per-user:** 1,000 requests/minute across all servers owned by a user. Protects overall platform capacity.

If either limit is exceeded, the request is rejected with `429`. Both limits must be enforced independently.

#### Token Bucket Algorithm

The token bucket algorithm allows burst traffic up to 150% of the per-minute limit (burst capacity) while enforcing an average rate. This prevents spiky-but-legitimate traffic (a user running an agent that makes 5 requests in 1 second) from hitting the limit, while still catching sustained high-rate callers.

Token bucket parameters:
- `capacity` = `rate_limit * 1.5` (e.g., 150 for per-server 100/min limit)
- `refill_rate` = `rate_limit / 60` tokens per second (e.g., 1.67/s for 100/min)
- `initial_tokens` = `capacity` (full bucket on first request)

#### Redis Backend for Multi-Instance Support

For a single gateway instance, an in-process token bucket (using `std::sync::Mutex<HashMap<key, BucketState>>`) suffices. But horizontal scaling requires shared state.

This story implements Redis-backed rate limiting using the `EVAL` command with a Lua script (atomic token bucket update):

```lua
local key = KEYS[1]
local capacity = tonumber(ARGV[1])
local refill_rate = tonumber(ARGV[2])   -- tokens per second
local now = tonumber(ARGV[3])           -- current unix timestamp (milliseconds)
local requested = tonumber(ARGV[4])     -- tokens requested (always 1)

local bucket = redis.call('HMGET', key, 'tokens', 'last_refill')
local tokens = tonumber(bucket[1]) or capacity
local last_refill = tonumber(bucket[2]) or now

local elapsed = (now - last_refill) / 1000.0
local new_tokens = math.min(capacity, tokens + elapsed * refill_rate)

if new_tokens >= requested then
    new_tokens = new_tokens - requested
    redis.call('HMSET', key, 'tokens', new_tokens, 'last_refill', now)
    redis.call('PEXPIRE', key, 120000)  -- 2 minute TTL
    return {1, math.floor(new_tokens)}  -- allowed, remaining tokens
else
    redis.call('HMSET', key, 'tokens', new_tokens, 'last_refill', now)
    redis.call('PEXPIRE', key, 120000)
    return {0, 0}                       -- rejected
end
```

Redis key naming:
- Per-server: `ratelimit:server:{server_id}`
- Per-user: `ratelimit:user:{user_id}`

#### Rate Limiting Middleware

The middleware runs in the gateway's middleware stack as a Tower layer. It extracts:
- `server_id` from the route path (for per-server limiting)
- `user_id` from the JWT / server token lookup result (for per-user limiting)

If `FEATURE_RATE_LIMIT_ENABLED = false`, the middleware is a no-op pass-through. Default: `true`.

If Redis is unavailable, the middleware falls back to in-process token buckets with a warning log. Do not fail requests when Redis is down — rate limiting is a best-effort protection, not a hard dependency.

#### 429 Response

```json
HTTP/1.1 429 Too Many Requests
Retry-After: 12
X-RateLimit-Limit: 100
X-RateLimit-Remaining: 0
X-RateLimit-Reset: 1711624800

{
  "error": {
    "code": "RATE_LIMITED",
    "message": "Rate limit exceeded. This server allows 100 requests per minute.",
    "retry_after_seconds": 12
  }
}
```

The `Retry-After` header value is the number of seconds until the bucket refills sufficiently for one more request.

#### Acceptance Criteria

- Sending 101 requests in under 60 seconds to a single MCP server returns `429` on the 101st request (burst capacity allows 150 before hard rejection — test with 151 requests in 1 second to trigger the limit).
- The `Retry-After` header in the `429` response is a positive integer representing seconds until the next request can succeed.
- `X-RateLimit-Limit`, `X-RateLimit-Remaining`, and `X-RateLimit-Reset` headers are present on every response (not only 429s).
- When Redis is unavailable (simulate by stopping the Redis container), rate limiting falls back to in-process buckets and requests continue to succeed. A `warn`-level log is emitted once per minute while Redis is unreachable (not once per request — rate-limit the warning itself).
- `FEATURE_RATE_LIMIT_ENABLED=false` disables rate limiting entirely. All requests pass without the `X-RateLimit-*` headers being set.
- Per-user rate limiting (1,000/min) is enforced across multiple servers. Sending 500 requests to server A and 501 requests to server B (by the same user) triggers the per-user limit on the 1,001st request.
- Integration test: verify rate limiting with a real Redis instance (Redis included in Docker Compose for testing).
- `RateLimitExceeded` audit log events are written when a limit is triggered.
- The Redis Lua script is atomic (no race conditions with concurrent requests from multiple gateway replicas). Verified by running concurrent load test (100 goroutines, but use Rust's `tokio::spawn`) and asserting no over-allowance.

#### Technical Notes

**Redis client:**

Use `redis` crate (async feature). The connection is a `deadpool_redis::Pool` for connection pooling. Configure pool size: 10 connections per gateway instance.

**Lua script caching:**

Redis caches Lua scripts by SHA1 hash. Use `redis::Script::new(LUA_SCRIPT)` which computes and stores the SHA1. Use `EVALSHA` for subsequent calls and fall back to `EVAL` on `NOSCRIPT` error.

**In-process fallback:**

```rust
struct InProcessBucket {
    tokens: f64,
    last_refill: Instant,
    capacity: f64,
    refill_rate: f64,  // tokens per second
}
```

Store in `Arc<Mutex<HashMap<String, InProcessBucket>>>`. The Mutex is contended but lock hold time is microseconds (simple arithmetic). Acceptable for the fallback case.

---

### S-019: Error Handling and Error Codes

**Story ID:** S-019
**Epic:** EPIC-01
**Priority:** P0
**Estimated Effort:** 3 points
**Dependencies:** S-010, S-004 (mcp-common AppError)

#### Description

Define and implement the complete error handling framework for all services. Error handling is a user experience concern, a debuggability concern, and a security concern simultaneously. A well-designed error system:

1. **Helps users understand what went wrong** — actionable human-readable messages.
2. **Helps engineers debug issues** — correlation IDs, structured fields, non-generic messages.
3. **Does not leak internal state to attackers** — no stack traces, no internal error messages, no database error strings in API responses.

This story implements the error catalog, the `IntoResponse` conversion for `AppError`, and the MCP-compatible JSON-RPC error mapping for gateway errors.

#### Error Catalog

| Code | HTTP Status | JSON-RPC Code | Description |
|------|-------------|---------------|-------------|
| `UNAUTHORIZED` | 401 | -32003 | Missing or invalid authentication token |
| `TOKEN_EXPIRED` | 401 | -32003 | Authentication token has expired |
| `FORBIDDEN` | 403 | -32003 | Authenticated but not authorized for this resource |
| `NOT_FOUND` | 404 | -32601 | Resource does not exist or caller does not own it |
| `VALIDATION_ERROR` | 400 | -32602 | Request payload failed validation |
| `SSRF_BLOCKED` | 422 | -32002 | URL blocked by SSRF protection |
| `RATE_LIMITED` | 429 | -32004 | Rate limit exceeded |
| `UPSTREAM_ERROR` | 502 | -32001 | Upstream API returned an error |
| `UPSTREAM_TIMEOUT` | 504 | -32001 | Upstream API did not respond within the timeout |
| `UPSTREAM_UNREACHABLE` | 502 | -32001 | Upstream API connection refused or DNS failure |
| `CREDENTIAL_NOT_FOUND` | 404 | -32000 | No credential configured for this server |
| `TOOL_NOT_FOUND` | 404 | -32000 | Requested MCP tool does not exist on this server |
| `NOT_IMPLEMENTED` | 501 | -32601 | Feature not available (feature flag disabled) |
| `INTERNAL_ERROR` | 500 | -32603 | Unexpected internal error (details logged, not exposed) |

#### REST API Error Response Format

```json
{
  "error": {
    "code": "VALIDATION_ERROR",
    "message": "display_name must be between 1 and 100 characters",
    "request_id": "550e8400-e29b-41d4-a716-446655440000",
    "details": {
      "field": "display_name",
      "constraint": "max_length",
      "max": 100,
      "actual": 142
    }
  }
}
```

Rules:
- `code` is always a string from the catalog above. No ad-hoc error codes.
- `message` is always human-readable English. No internal error messages (e.g., no "pq: duplicate key value violates unique constraint").
- `request_id` is always present (from the `X-Request-ID` middleware).
- `details` is optional. Present only for `VALIDATION_ERROR` to provide field-level context.
- For `INTERNAL_ERROR`: the message is always the generic string `"An unexpected error occurred. Please try again or contact support with the request ID."` The actual error is logged server-side with the `request_id` for correlation.

#### MCP JSON-RPC Error Response Format

For gateway errors (which must be JSON-RPC 2.0 compliant):

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "error": {
    "code": -32001,
    "message": "Upstream API returned an error",
    "data": {
      "upstream_status": 401,
      "upstream_body": "Invalid API key",
      "request_id": "550e8400..."
    }
  }
}
```

The `data` field provides context without exposing internal details. `upstream_body` is truncated to 500 characters.

#### `AppError` → HTTP Response Mapping

```rust
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code, message, details) = match &self {
            AppError::Unauthorized => (StatusCode::UNAUTHORIZED, "UNAUTHORIZED",
                "Authentication required.", None),
            AppError::Forbidden => (StatusCode::FORBIDDEN, "FORBIDDEN",
                "You do not have permission to access this resource.", None),
            AppError::NotFound { resource } => (StatusCode::NOT_FOUND, "NOT_FOUND",
                format!("{} not found.", resource), None),
            AppError::Validation { field, message } => (StatusCode::BAD_REQUEST, "VALIDATION_ERROR",
                message.clone(), Some(json!({"field": field}))),
            AppError::SsrfBlocked { .. } => (StatusCode::UNPROCESSABLE_ENTITY, "SSRF_BLOCKED",
                "The provided URL is not allowed.", None),
            AppError::RateLimited => (StatusCode::TOO_MANY_REQUESTS, "RATE_LIMITED",
                "Rate limit exceeded. Please slow down your requests.", None),
            AppError::UpstreamError { status, body } => (StatusCode::BAD_GATEWAY, "UPSTREAM_ERROR",
                format!("Upstream API error (HTTP {}).", status),
                Some(json!({"upstream_status": status, "upstream_body": &body[..body.len().min(500)]}))),
            AppError::Internal(_) | AppError::Database(_) => {
                tracing::error!(error = ?self, "Internal error");  // Log full error
                (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR",
                "An unexpected error occurred. Please try again or contact support with the request ID.",
                None)
            }
            // ... other variants
        };

        let request_id = /* extract from response extensions */;
        let body = json!({
            "error": {
                "code": code,
                "message": message,
                "request_id": request_id,
                "details": details
            }
        });

        (status, Json(body)).into_response()
    }
}
```

#### Acceptance Criteria

- Every error response from `services/api` conforms to the REST error format above. No endpoint returns a plain-text error or an axum default error body.
- Every error response from `services/gateway` for MCP requests conforms to JSON-RPC 2.0 error format.
- `INTERNAL_ERROR` responses never include the original error message, stack trace, or database error string. Verified by triggering DB connection failures and asserting the response body contains only the generic message.
- `VALIDATION_ERROR` responses always include the `details.field` for field-level errors.
- Error codes map to the correct HTTP status codes as defined in the catalog.
- Error responses always include `request_id`.
- An integration test covers all error types by triggering the conditions that produce them and asserting response structure.
- `cargo test -p mcp-common` includes tests for `AppError::IntoResponse` for every variant.
- A Clippy lint is added (or enforced via code review convention) that `AppError::Internal` is never used for expected error conditions. Expected errors (not found, auth failure, validation) use their specific variants.

#### Technical Notes

**Preventing internal error leakage:**

The `AppError::Database` variant wraps `sqlx::Error`. The `IntoResponse` for `Database` must extract NO information from the sqlx error except whether it is a connection error vs. a query error. Log the full `sqlx::Error` at `error` level server-side, but return `INTERNAL_ERROR` to the client.

**MCP error mapping:**

Implement a separate `McpError::from(AppError)` conversion (not `IntoResponse`) in `libs/mcp-protocol`. This conversion translates `AppError` variants to JSON-RPC error codes as per the catalog table. The gateway uses this to wrap errors in proper JSON-RPC responses.

**`?` operator ergonomics:**

All route handlers return `Result<impl IntoResponse, AppError>`. The `?` operator converts `sqlx::Error` to `AppError::Database` automatically via the `From` impl. `CredentialService` methods return `AppError`, so `?` works transitively. Engineers should never write `.map_err(|e| AppError::Internal(e.into()))` for expected error cases.

---

## Epic Summary Table

| Story | Title | Points | Priority | Dependencies |
|-------|-------|--------|----------|--------------|
| S-010 | Platform API Service Scaffolding | 5 | P0 | S-000, S-003, S-004, S-005, S-006, S-008 |
| S-011 | Clerk Authentication Integration | 5 | P0 | S-010, S-003 |
| S-012 | Credential Encryption Service | 3 | P0 | S-004, S-003 |
| S-013 | Credential Injector Sidecar | 8 | P0 | S-010, S-012, S-003, S-006 |
| S-014 | SSRF Protection Module | 5 | P0 | S-004 |
| S-015 | Audit Logging Service | 3 | P0 | S-003, S-004 |
| S-016 | Server CRUD API | 5 | P0 | S-010, S-011, S-003, S-014, S-015 |
| S-017 | Credential CRUD API | 5 | P0 | S-016, S-012, S-015 |
| S-018 | Rate Limiting Middleware | 5 | P1 | S-010, S-011, S-006 |
| S-019 | Error Handling & Error Codes | 3 | P0 | S-010, S-004 |
| **Total** | | **47** | | |

## Critical Path

The critical path for EPIC-01 is:

```
S-010 (API scaffolding)
  |
  +-- S-011 (Auth) --> S-016 (Server CRUD) --> S-017 (Credential CRUD)
  |
  +-- S-019 (Errors)   [completes in parallel with S-011]

S-012 (Crypto service) --> S-013 (Injector) [in parallel with S-010/S-011]

S-014 (SSRF) [in parallel, no blocking dependency on S-010]

S-015 (Audit log) [in parallel, depends only on S-003 and S-004]

S-018 (Rate limiting) [after S-010, S-011; can start in parallel with S-016]
```

All P0 stories in this epic must complete before EPIC-02 (Gateway Core and MCP Protocol) begins. S-018 (P1) may be completed in the first sprint of EPIC-02 if time pressure demands it, as the gateway can operate without rate limiting during internal testing.

## Security Review Gates

Before any story in this epic is merged to `main`, the following security checks must pass:

1. **S-012 and S-013:** Code review by a second engineer specifically focused on: does any code path expose `plaintext` after encryption? Does the `ZeroizeOnDrop` trait cover all credential variables?
2. **S-013:** Verify `MCP_INJECTOR_ALLOWED_CALLER_IPS` is set and tested. An injector accessible from the public internet without IP filtering is a critical vulnerability.
3. **S-014:** All SSRF test cases pass including the DNS rebinding simulation.
4. **S-015:** Verify audit log entries for `CredentialAccess` never include the credential value in any field.
5. **S-019:** Verify `INTERNAL_ERROR` responses are exercised in tests and confirmed to not leak `sqlx::Error` messages.
