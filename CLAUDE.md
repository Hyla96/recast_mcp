# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Recast MCP is a hosted, no-code platform that exposes any REST API to AI agents (Claude, Cursor, ChatGPT) as a live MCP server. The full product spec lives in `docs/SUMMARY.md`.

**Status:** Active development — monorepo scaffolding complete, PostgreSQL schema migrations in place, shared Rust libraries implemented, Woodpecker CI pipelines and Docker multi-stage builds in place, OpenTelemetry telemetry foundation wired into all services.

## Planned Architecture

**Gateway model (Option B):** A single shared Rust proxy serves all user-created MCP servers via config-driven routing. No per-user containers.

Three main services:
- **Gateway** — Rust/axum multi-tenant MCP proxy. Handles JSON-RPC 2.0 over Streamable HTTP (primary) and SSE (fallback). Uses moka for in-memory config cache, PostgreSQL LISTEN/NOTIFY for hot reload.
- **Platform API** — Rust/axum control plane. CRUD for servers/credentials, Clerk auth, audit logging.
- **Credential Injector Sidecar** — Separate process that decrypts and injects credentials via Unix domain socket. Gateway never holds raw credentials.

Frontend: React 19 + TypeScript + Vite + Zustand.

## Planned Tech Stack

- **Backend:** Rust, axum, tokio, sqlx, serde, jsonpath-rust, aes-gcm, reqwest, tower
- **Frontend:** React 19, TypeScript, Vite, Zustand, @dnd-kit, @tanstack/virtual, jsonpath-plus, fast-xml-parser
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
- Dev seed data: `migrations/seed_dev.sql` — run with `just db-seed`. Idempotent.
- sqlx offline query cache: `.sqlx/` directory (tracked in git). Regenerate with `just db-prepare` when `sqlx::query!()` macros are added/changed.
- Set `SQLX_OFFLINE=true` in CI to compile without a live database.
- Five core tables: `users`, `mcp_servers`, `credentials`, `server_tokens`, `audit_log`.
- Hot-path indexes: `idx_mcp_servers_slug_active` (partial, status = 'active'), `idx_server_tokens_hash_active` (partial, is_active = true).
- `updated_at` columns on `users` and `mcp_servers` are maintained automatically by PostgreSQL triggers.

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
- **Metrics:** Prometheus format at `/metrics` on each service. Uses `metrics` + `metrics-exporter-prometheus` crates. Record DB query durations as `db_query_duration_seconds` histogram.
- **Local observability stack:** `docker compose up` starts OTEL Collector (ports 4317/4318) and Jaeger UI at `http://localhost:16686`.
- **HTTP tracing:** `tower_http::trace::TraceLayer` creates a tracing span per HTTP request. Combined with the OTEL subscriber layer, these become OTEL spans automatically.
- Crate versions: `opentelemetry 0.26`, `opentelemetry_sdk 0.26` (rt-tokio feature), `opentelemetry-otlp 0.26` (grpc-tonic feature), `tracing-opentelemetry 0.27`.
- `opentelemetry_otlp::new_pipeline().tracing()...install_batch(Tokio)` returns `TracerProvider` in 0.26 (not `Tracer`). Call `.tracer(name)` on the provider to get a `Tracer` for `tracing_opentelemetry::layer().with_tracer()`.
- Use `sdktrace::Config::default()` not `sdktrace::config()` (deprecated alias).

## Build Sequence (When Code Exists)

The planned 8-week build starts with:
1. Monorepo scaffolding (Cargo + pnpm workspaces)
2. PostgreSQL schema + migrations (sqlx)
3. Platform API + Gateway + Credential Injector skeletons
4. Gateway core (JSON-RPC, transports, config cache, hot reload)
5. Frontend builder UI
6. Deploy flow + playground
