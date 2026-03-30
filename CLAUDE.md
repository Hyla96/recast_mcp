# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Recast MCP is a hosted, no-code platform that exposes any REST API to AI agents (Claude, Cursor, ChatGPT) as a live MCP server. The full product spec lives in `docs/SUMMARY.md`.

**Status:** Active development — EPIC-00 (project setup) and EPIC-01 (foundation services) substantially complete. Monorepo scaffolding, PostgreSQL schema migrations, shared Rust libraries, Woodpecker CI pipelines, Docker multi-stage builds, OpenTelemetry telemetry, health check endpoints, Clerk JWT auth, credential encryption (AES-256-GCM), CRUD APIs (servers, credentials, tokens), webhooks, SSRF protection, rate limiting (Redis + in-process fallback), credential injector sidecar, and audit logging are all implemented.

## Planned Architecture

**Gateway model (Option B):** A single shared Rust proxy serves all user-created MCP servers via config-driven routing. No per-user containers.

Three main services:
- **Gateway** — Rust/axum multi-tenant MCP proxy. Handles JSON-RPC 2.0 over Streamable HTTP (primary) and SSE (fallback). Uses moka for in-memory config cache, PostgreSQL LISTEN/NOTIFY for hot reload.
- **Platform API** — Rust/axum control plane. CRUD for servers/credentials, Clerk auth, audit logging.
- **Credential Injector Sidecar** — Separate process that decrypts and injects credentials via Unix domain socket. Gateway never holds raw credentials.

Frontend: React 19 + TypeScript + Vite + Zustand + Tanstack Query + React Router. Design system: `docs/ui/SYSTEM_DESIGN.md` (v2.1).

## Planned Tech Stack

- **Backend:** Rust, axum, tokio, sqlx, serde, jsonpath-rust, aes-gcm, reqwest, tower
- **Frontend:** React 19, TypeScript, Vite, Zustand (+ immer middleware), Tanstack Query, React Router v6+, Vitest, React Testing Library, TailwindCSS
- **Database:** PostgreSQL (JSONB configs, pgcrypto, LISTEN/NOTIFY)
- **Auth:** Clerk (React + Rust SDKs)
- **Monorepo:** Cargo workspaces (Rust) + pnpm workspaces (frontend)
- **Task Runner:** [just](https://github.com/casey/just) — all project commands live in `justfile`
- **CI/CD:** Woodpecker CI + Docker

## Key Design Decisions

- MCP protocol scope (MVP): `tools/list`, `tools/call`, `initialize`, `initialized` only
- Auth types (MVP): Bearer Token, API Key (header/query), Basic Auth — no OAuth
- Transforms are declarative only (JSONPath, field rename, safe arithmetic, array flattening) — no Turing-complete scripting
- Credential encryption: AES-256-GCM, per-row IV
- SSRF protection: blocklist + post-DNS IP validation (RFC 1918, link-local, cloud metadata)
- Response constraints: JSON only, 100KB max, 30s timeout
- Rate limits: 100 calls/min per server, 1,000 calls/min per user (token bucket via Tower)

## Common Commands

Run `just` to see all available recipes. Key ones:

| Command | Description |
|---------|-------------|
| `just check` | Run fmt-check + lint + test |
| `just build` | Build all Rust crates |
| `just test` | Run all Rust tests |
| `just fe-dev` | Start frontend dev server |
| `just fe-build` | Build frontend for production |
| `just run-gateway` | Run the gateway service |
| `just run-api` | Run the platform API |
| `just db-migrate` | Run sqlx migrations |
| `just db-seed` | Seed DB with dev data (idempotent) |
| `just db-prepare` | Generate .sqlx/ offline cache (needs DATABASE_URL) |
| `just clean` | Remove build artifacts |

### Docker Compose (local dev environment)

| Command | Description |
|---------|-------------|
| `make dev` | Start all services with hot reload (cargo-watch + Vite HMR) |
| `make down` | Stop all containers (volumes preserved) |
| `make logs` | Tail logs from all services |
| `make db-migrate` | Run sqlx migrations against localhost:5432 |
| `make db-reset` | Drop/recreate db, migrate, seed |
| `docker compose down -v` | Stop and remove all containers and volumes |

The `make dev` command merges `docker-compose.yml` (base production topology) and
`docker/docker-compose.override.yml` (dev hot-reload overrides with cargo-watch and Vite).

Copy `.env.example` → `.env` before running `make dev`. The `.env` file is git-ignored.

Port map (host):
- PostgreSQL: `localhost:5432`
- Gateway: `localhost:3000`
- Platform API: `localhost:3001`
- Credential Injector: `localhost:3002`
- React dev server: `localhost:5173`

## Database

- Migrations live in `migrations/` using timestamp-prefixed filenames (`YYYYMMDDHHMMSS_name.sql`).
- Run `just db-migrate` after cloning or adding new migrations.
- Dev seed data: `migrations/seeds/seed_dev.sql` — run with `just db-seed`. Idempotent. The seeds directory is a subdirectory so `sqlx::migrate!()` skips it.
- sqlx offline query cache: `.sqlx/` directory (tracked in git). Regenerate with `just db-prepare` when `sqlx::query!()` macros are added/changed.
- Set `SQLX_OFFLINE=true` in CI to compile without a live database.
- Five core tables: `users`, `mcp_servers`, `credentials`, `server_tokens`, `audit_log`.
- Hot-path indexes: `idx_mcp_servers_slug_active` (partial, status = 'active'), `idx_server_tokens_hash_active` (partial, is_active = true).
- `updated_at` columns on `users` and `mcp_servers` are maintained automatically by PostgreSQL triggers.
- `sqlx::Error::Database` exposes `.constraint()` which returns the violated constraint name. Use `db_err.constraint().is_some_and(|c| c.contains("constraint_name"))` to distinguish specific conflicts (e.g., slug collisions) from other DB errors.
- `use sqlx::Row` must be in scope for `.try_get()` to compile. In test functions, place the import at the top of the function body — module-level imports don't always suffice when the test module is in a separate file.

## CI/CD Pipelines (Woodpecker CI)

Pipeline files live in `.woodpecker/`. Each file has a pipeline-level `when.paths` filter so only the relevant pipeline triggers on a given PR.

| Pipeline file | Triggers on |
|---|---|
| `rust-shared.yml` | `libs/**`, `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml` |
| `gateway.yml` | `services/gateway/**` |
| `api.yml` | `services/api/**` |
| `credential-injector.yml` | `services/credential-injector/**` |
| `frontend.yml` | `apps/web/**`, `pnpm-workspace.yaml`, `pnpm-lock.yaml` |

All Rust CI steps set `SQLX_OFFLINE=true` so no live database is needed.

**Docker images** (built on push to `main` only):
- Dockerfiles live in `docker/{service}/Dockerfile`.
- All use a cargo-chef multi-stage pattern: `chef → planner → builder → runtime (debian:bookworm-slim)`.
- Tagged `{registry}/{org}/mcp-{service}:{short-sha}` and `{registry}/{org}/mcp-{service}:latest`.
- Secrets required in Woodpecker server: `docker_registry`, `docker_org`, `docker_username`, `docker_password`.
- Platforms: `linux/amd64,linux/arm64`.

**Lib-dep guard** (in `rust-shared.yml`): `.woodpecker/scripts/check-lib-deps.sh` verifies no `libs/*` crate depends on any `services/*` crate.

**Local execution** (no Woodpecker server needed):
```
woodpecker-cli exec .woodpecker/rust-shared.yml
```

## Telemetry

All three Rust services initialize telemetry via `mcp_common::init_telemetry(service_name, version)` in `main()`. The guard returned must be held for the process lifetime.

- **Traces:** OTLP gRPC to `OTEL_EXPORTER_OTLP_ENDPOINT` (default `http://localhost:4317`). Set `OTEL_SDK_DISABLED=true` to disable.
- **Logs:** Structured JSON on stdout with fields: `timestamp`, `level`, `service`, `version`, `message`, `trace_id` (inside spans), `span_id` (inside spans).
- **Metrics:** Prometheus format at `/metrics` on each service. Uses `metrics` + `metrics-exporter-prometheus` crates. Record DB query durations as `db_query_duration_seconds` histogram. The shared `mcp_common::track_metrics` middleware records `http_requests_total` and `http_request_duration_seconds` using `axum::extract::MatchedPath` for the `path` label (route template like `/v1/servers/:id`, not the raw URI) to prevent unbounded Prometheus cardinality.
- **Local observability stack:** `docker compose up` starts OTEL Collector (ports 4317/4318) and Jaeger UI at `http://localhost:16686`. Topology: services → `otel-collector:4317` (OTLP gRPC) → collector exports to `jaeger:14250` (OTLP gRPC). The collector (contrib image) mediates all telemetry — services never connect directly to Jaeger.
- **HTTP tracing:** `tower_http::trace::TraceLayer` creates a tracing span per HTTP request. Combined with the OTEL subscriber layer, these become OTEL spans automatically.
- Crate versions: `opentelemetry 0.26`, `opentelemetry_sdk 0.26` (rt-tokio feature), `opentelemetry-otlp 0.26` (grpc-tonic feature), `tracing-opentelemetry 0.27`.
- `opentelemetry_otlp::new_pipeline().tracing()...install_batch(Tokio)` returns `TracerProvider` in 0.26 (not `Tracer`). Call `.tracer(name)` on the provider to get a `Tracer` for `tracing_opentelemetry::layer().with_tracer()`.
- Use `sdktrace::Config::default()` not `sdktrace::config()` (deprecated alias).

## API Error Contract

All 4xx/5xx responses from the Platform API use a single JSON shape. Full spec in `docs/api/README.md`, JSON Schema in `docs/api/error-schema.json`.

```json
{"error": {"code": "not_found", "message": "...", "request_id": "01HWRF..."}}
```

- `AppError` (in `mcp-common`) implements `axum::response::IntoResponse`. When converted to a response it generates a fresh ULID, puts it in BOTH the JSON body (`error.request_id`) and the `X-Request-ID` response header so they always match.
- `mcp_common::request_id_middleware` (axum `from_fn` middleware) adds `X-Request-ID` to responses that don't already carry the header (i.e., successful responses). Wire it with `.layer(axum::middleware::from_fn(request_id_middleware))`.
- `mcp_common::RequestId` extension is stored on the request by the middleware; handlers can extract it via `Extension<RequestId>` if needed.
- Error messages never include stack traces, SQL error strings, or internal file paths.
- `#[serde(deny_unknown_fields)]` on structs used as axum `Json<T>` extractors automatically returns 422 Unprocessable Entity for unknown fields in the request body — no custom validation needed. Use this on input DTOs (e.g., `ServerConfigInput`) but not on DB-read DTOs where forward compatibility matters.

## Health Checks

All three services expose identical health endpoints via shared handlers in `mcp_common::health`:

- `GET /health/live` — HTTP 200 immediately after process start. Body: `{"status":"ok","service":"...","version":"..."}`. No dependency checks.
- `GET /health/ready` — HTTP 200 when PostgreSQL is reachable; HTTP 503 otherwise. Body: `{"status":"ok"|"degraded","checks":{"database":{"status":"ok"|"error","message":"..."}}}`. Individual checks time out after 500 ms.

Health routes are mounted on a separate `Router` without `TraceLayer`, so they produce **no OTEL spans**. They are also outside the metrics middleware so they don't skew `http_requests_total`.

DB connectivity uses `sqlx::PgPool::connect_lazy` (non-blocking at startup). The ready handler calls `pool.acquire()` to verify connectivity.

`DbCheckerFn = Arc<dyn Fn() -> DbCheckFuture + Send + Sync>` — an injectable type alias. Use `mcp_common::health::pg_pool_checker(pool)` in production; inject a mock closure in tests. This makes service-level tests fully DB-independent.

docker-compose health checks use `/health/ready` (not `/health/live`) so `condition: service_healthy` verifies actual DB connectivity.

## Auth (Platform API)

JWT validation uses `jsonwebtoken = "9"` with RS256. Clerk JWKS is cached in `JwksCache` (5-min TTL, `Arc<RwLock<...>>`). Configuration:

- `CLERK_JWKS_URL` (required) — full URL to Clerk's `/.well-known/jwks.json`
- `CLERK_ISSUER` (optional) — expected `iss` claim; empty string skips iss validation
- `CLERK_WEBHOOK_SECRET` (required) — Svix signing secret from Clerk dashboard. Format: `whsec_<base64>`. Used by `POST /v1/webhooks/clerk` to verify signatures before processing user lifecycle events.

Auth middleware is applied via `route_layer` on `/v1/*` routes, not globally. This allows `/v1/webhooks/clerk` (TASK-017) to bypass auth by being in a separate `Router::new()` that is merged before the `route_layer` call.

`JwksCache::new(url)` accepts any URL — point it at a `MockUpstream` in tests. The cache fetches on first call and on unknown `kid` (key rotation). Write lock is taken only on cache miss.

Email must come from the JWT claims (Clerk session claims template must include `email`). The `sub` claim is the Clerk user ID.

**Lib/bin split**: `services/api` has both `src/lib.rs` (for integration tests) and `src/main.rs` (entry point). Integration tests in `tests/` import from `mcp_api` (the lib target). The binary in `main.rs` also uses `use mcp_api::...` to avoid duplicated module trees.

**Service-layer pattern:** Business logic and SQL queries live in service structs, not in handlers. `CredentialService` (`credentials.rs`) and `ServerService` (`servers.rs`) encapsulate database access, encryption, and audit logging behind clean interfaces. Handlers are thin HTTP-layer wrappers: extract request → call service → map to response. New resources should follow this pattern.

## Integration Tests

Integration tests live in `services/{service}/tests/integration_tests.rs` (and `tests/auth_tests.rs` for auth, `tests/webhook_tests.rs` for webhooks). They require a live PostgreSQL instance.

```
TEST_DATABASE_URL=postgres://recast:recast@localhost:5432/postgres \
  cargo test --workspace --test integration_tests
```

- Set `TEST_DATABASE_URL` (or `DATABASE_URL`) — must point to any database on the server; the DB name is replaced per-test.
- `mcp_common::testing` module (feature `testing`) provides: `TestDatabase` (isolated DB per test, auto-dropped), `MockUpstream` (in-process HTTP stub with request recording), `TestMcpClient` (JSON-RPC client helper).
- Services activate the testing utilities via: `mcp-common = { ..., features = ["testing"] }` in `[dev-dependencies]`. The feature is never active in release builds.
- Integration tests are **not** compiled by `cargo build` or `cargo clippy` without `--tests`. They have `#![allow(clippy::expect_used, ...)]` at the top since panicking on test-setup failure is intentional.
- Seed file is at `migrations/seeds/seed_dev.sql` (subdirectory keeps it out of `sqlx::migrate!()` scans).
- **Shared test helpers** live in `services/api/tests/helpers/mod.rs`. This module provides `TestRsaKey`, `make_jwt`, `make_jwt_with_offset`, `make_state_with_jwks`, and `TEST_ISSUER` — imported by auth, server, and credential endpoint tests via `mod helpers;`. New integration test files should use these shared fixtures instead of defining their own.
- `#[tokio::test]` requires `tokio = { version = "1", features = ["rt", "macros"] }` in dev-dependencies. The `"rt"` feature alone is insufficient — the `macros` feature provides the `#[tokio::test]` proc macro.

## Module Structure

### `libs/common/` (mcp-common)

Shared library for all services. Key modules:
- `error.rs` — `AppError` enum, `McpError`, `IntoResponse` impl, sanitized error messages
- `audit.rs` — `AuditLogger` with batched async background writer
- `rate_limit/mod.rs` — Token bucket rate limiter (Redis + in-process fallback), `rate_limit/lua.rs` contains the Redis Lua script
- `ssrf.rs` — URL/IP validation against SSRF (blocklists, DNS resolution, CIDR checks)
- `middleware.rs` — `track_metrics`, `metrics_handler`, `fallback_handler`, `request_id_middleware` (shared across all services)
- `health.rs` — `/health/live` and `/health/ready` endpoint handlers
- `config.rs` — Environment variable loading helpers, `FromEnv` trait
- `telemetry.rs` — OpenTelemetry + tracing initialization
- `testing/` — feature-gated (`testing`) test utilities: `TestDatabase`, `MockUpstream`, `TestMcpClient`

### `services/api/` (mcp-api)

Platform API service. Module layout:
- `main.rs` — Startup lifecycle only: config, telemetry, pool, state assembly, serve loop, shutdown
- `router.rs` — `build_router`, `build_cors`, `build_router_with_timeout`, route wiring
- `shutdown.rs` — `shutdown_signal` (SIGTERM/SIGINT)
- `app_state.rs` — `AppState` struct
- `auth.rs` — Clerk JWT validation, `JwksCache` with 5-min TTL
- `credentials.rs` — `CredentialService` (SQL + encryption + audit)
- `servers.rs` — `ServerService` (SQL + slug generation + audit)
- `handlers/` — Thin HTTP handlers organized by resource:
  - `servers/mod.rs` — CRUD handlers + validation helpers
  - `servers/types.rs` — Request/response DTOs, `ServerConfig`, pagination types
  - `credentials.rs`, `tokens.rs`, `users.rs`, `webhooks.rs`
- `middleware.rs` — API-specific middleware (auth layer wiring)
- `config.rs` — `ApiConfig` (environment-validated)
- `tests/helpers/mod.rs` — Shared test fixtures (RSA keys, JWT builders, state constructors)

## Build Sequence (When Code Exists)

The planned 8-week build starts with:
1. Monorepo scaffolding (Cargo + pnpm workspaces)
2. PostgreSQL schema + migrations (sqlx)
3. Platform API + Gateway + Credential Injector skeletons
4. Gateway core (JSON-RPC, transports, config cache, hot reload)
5. Frontend builder UI
6. Deploy flow + playground
