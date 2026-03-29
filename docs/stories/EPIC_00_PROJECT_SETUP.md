# EPIC-00: Project Setup & Monorepo Foundation

**Epic ID:** EPIC-00
**Product:** Recast MCP — Dynamic MCP Server Builder
**Architecture:** Gateway Pattern (Option B) — single shared Rust proxy, config-driven routing, no per-user containers
**Status:** Ready for Engineering
**Date:** 2026-03-28
**Source of truth:** `docs/SUMMARY.md` §§ 5, 6, 7, 8, 21

---

## Epic Summary

Establish the complete monorepo skeleton, CI/CD pipelines, local development environment, PostgreSQL schema, shared libraries, telemetry, and health-check infrastructure that every subsequent epic depends on. **No feature work in any other epic begins until every P0 story in this epic is done.** Every architectural and tooling decision made here locks in the development workflow for the life of the product; changes after EPIC-01 begins require a written ADR.

## Epic Acceptance Criteria

- `docker compose up` brings the full system to a healthy state within 60 seconds on a developer machine with a warm image cache (single command, no pre-steps).
- `cargo build --workspace` compiles all Rust crates with zero errors and zero Clippy warnings.
- `cargo test --workspace` passes on a clean clone against a locally managed PostgreSQL container.
- The PostgreSQL schema is version-controlled via sqlx forward-only migrations; `sqlx migrate run` is idempotent.
- All shared library crates export stable public APIs documented with `rustdoc`; breaking changes require bumping the crate version and updating all consumers.
- Telemetry (traces, structured logs, metrics) is emitting from all services to a local OpenTelemetry collector before any EPIC-01 service work begins.
- Health-check endpoints (`/health/live` and `/health/ready`) respond correctly on all services before any EPIC-01 story begins.
- A developer unfamiliar with the codebase can reach a working local environment within 15 minutes by following only the root `README.md`.

---

## Stories

---

### S-000: Monorepo Scaffolding — Cargo Workspace + pnpm Workspace

**Story ID:** S-000
**Epic:** EPIC-00
**Priority:** P0
**Estimated Effort:** 3 points
**Dependencies:** None

#### Description

Initialize the repository as a dual-workspace monorepo: a Cargo workspace for all Rust services and libraries, and a pnpm workspace for the React frontend. The directory layout is fixed from this point forward; changes after EPIC-01 begins require a documented ADR, because all CI pipelines, Docker build contexts, and inter-crate import paths reference it.

The directory layout:

```
/
  Cargo.toml                  # Workspace root — [workspace] members list every Rust crate
  Cargo.lock                  # Committed. Single lock file for all Rust crates.
  rust-toolchain.toml         # Pins exact Rust channel + version for all CI and local builds
  pnpm-workspace.yaml         # pnpm workspace root: packages: ["apps/*"]
  package.json                # Root package.json: workspace scripts only, no runtime deps
  pnpm-lock.yaml              # Committed.
  .cargo/
    config.toml               # Workspace-level Cargo config (linker, incremental flags)
  .github/
    workflows/                # CI workflow files (populated by S-001)
    dependabot.yml
  services/
    gateway/                  # MCP data-plane proxy (crate: mcp-gateway)
      Cargo.toml
      src/main.rs
      Dockerfile
    api/                      # Platform control-plane API (crate: platform-api)
      Cargo.toml
      src/main.rs
      Dockerfile
    credential-injector/      # Auth sidecar (crate: credential-injector)
      Cargo.toml
      src/main.rs
      Dockerfile
  libs/
    mcp-protocol/             # MCP JSON-RPC 2.0 types (crate: mcp-protocol)
      Cargo.toml
      src/lib.rs
    crypto/                   # AES-256-GCM encryption primitives (crate: mcp-crypto)
      Cargo.toml
      src/lib.rs
    common/                   # Shared error types, config loader, telemetry (crate: mcp-common)
      Cargo.toml
      src/lib.rs
  apps/
    web/                      # React 19 + TypeScript frontend (pnpm package: @mcp/web)
      package.json
      vite.config.ts
      tsconfig.json
      src/
  migrations/                 # sqlx migration files, numbered sequentially
    001_initial_schema.sql
  docker/
    docker-compose.yml
    docker-compose.override.yml
  docs/
    adr/                      # Architecture Decision Records
  .editorconfig
  .nvmrc                      # Node.js version pin
  Makefile                    # Developer convenience targets
  README.md
```

#### Acceptance Criteria

- `cargo build --workspace` compiles all Rust crates from the workspace root with zero errors and zero warnings on the toolchain version pinned in `rust-toolchain.toml`.
- `cargo test --workspace` runs all unit tests and passes on a clean clone.
- `pnpm install` installs all frontend dependencies from the repository root.
- `pnpm --filter @mcp/web build` produces a production bundle without TypeScript errors.
- Each `services/*` crate only declares `libs/*` path dependencies in its `Cargo.toml`. No service crate imports another service crate.
- `libs/*` crates declare zero dependencies on any `services/*` crate (enforced by CI; see S-001).
- `rust-toolchain.toml` pins `channel = "stable"` with a `date` field. CI and local builds use this version identically.
- `.cargo/config.toml` sets `[build] incremental = true` for local builds and documents why `incremental = false` is set in CI (reproducibility).
- `[workspace.lints.rust]` and `[workspace.lints.clippy]` in the root `Cargo.toml` enforce at minimum: `deny(warnings)`, `deny(clippy::unwrap_used)`, `deny(clippy::expect_used)`. Exceptions in test code are scoped with `#[cfg(test)]` or `#[allow(clippy::...)]` with a comment.
- `[workspace.package]` in root `Cargo.toml` provides `version`, `edition`, `license`, `rust-version`. Individual crates inherit and do not redeclare these.
- `Cargo.lock` is committed (binary target, not a library).
- `pnpm-lock.yaml` is committed.
- Root `README.md` documents the directory layout and the single-command local setup path.

#### Technical Notes

- Use Cargo workspace dependency inheritance (`[workspace.dependencies]`) for shared third-party crates (axum, tokio, serde, sqlx, etc.) so version bumps happen in one place.
- Pin Node.js version in `.nvmrc` and `engines` field of root `package.json`. Match the version used in all CI runners (see S-001).
- Add `.editorconfig` covering indent style, charset, and line endings for all file types (Rust, TypeScript, SQL, YAML, TOML).
- The `migrations/` directory lives at the workspace root so `sqlx-cli` can be run from the root without service-specific config.

#### Out of Scope

- Database setup (S-003).
- Docker Compose configuration (S-002).
- CI/CD pipeline implementation (S-001).
- Any service logic.

---

### S-001: CI/CD Pipeline — Woodpecker CI with Per-Service Path Filtering

**Story ID:** S-001
**Epic:** EPIC-00
**Priority:** P0
**Estimated Effort:** 5 points
**Dependencies:** S-000

#### Description

Implement Woodpecker CI pipelines that enforce quality gates on every pull request and produce deployable Docker images on every merge to `main`. The pipeline must support independent builds per service — a change to `services/gateway` must not trigger a rebuild of `services/api` or `apps/web`. Parallelism is mandatory; sequential builds do not scale to a multi-service monorepo.

Woodpecker CI runs entirely in Docker containers and can be executed locally via `woodpecker-cli exec`, making it possible to validate pipeline changes on a developer machine before pushing.

Three operational modes:

1. **Pull request:** Lint, format check, test, and build checks for changed services only (path-filtered via `when: path:`). Target: total PR check time under 5 minutes.
2. **Merge to `main`:** Full build, test, Docker image build and push for changed services. Images tagged with short Git SHA and `latest`.
3. **Release tag (`v*`):** Full build for all services regardless of diff. Images tagged with semantic version.

#### Acceptance Criteria

- A PR changing only `services/gateway/**` triggers gateway CI jobs only. `services/api`, `services/credential-injector`, and `apps/web` jobs do not run.
- A PR changing `libs/**`, `Cargo.toml`, or `Cargo.lock` triggers lint and test jobs for **all** Rust services and libraries (transitive dependency change), but not Docker build jobs.
- `cargo clippy --workspace -- -D warnings` passes with zero warnings. A single Clippy warning fails the build.
- `cargo fmt --check` passes. Unformatted code fails the build.
- `cargo test --workspace` passes. A single test failure fails the build.
- `pnpm --filter @mcp/web type-check` passes. TypeScript type errors fail the build.
- `pnpm --filter @mcp/web lint` (ESLint) passes. Lint errors fail the build.
- Docker images for each Rust service use multi-stage builds: `builder` stage compiles the binary; `runtime` stage is `debian:bookworm-slim` with only the binary and runtime libraries. No build toolchain in the final image.
- Docker images are pushed to a container registry on merge to `main`, tagged `{registry}/org/mcp-{service}:{short-sha}` and `{registry}/org/mcp-{service}:latest`.
- Renovate is configured for weekly Cargo and npm dependency updates. Security-advisory PRs for high/critical CVEs are auto-merged after CI passes.
- CI secrets (registry push token, signing keys) are stored in Woodpecker server secrets; never in pipeline files or committed code. Secrets are referenced via `from_secret` in pipeline steps.
- A CI step validates pipeline YAML syntax on every PR using `woodpecker-ci/lint`.
- A CI job verifies that no `libs/*` crate declares a dependency on any `services/*` crate (cross-layer import check).
- Any developer can run the full pipeline locally with `woodpecker-cli exec .woodpecker/<pipeline>.yml` without a running Woodpecker server.

#### Technical Notes

**Pipeline file layout** — one pipeline file per service under `.woodpecker/`:

```
.woodpecker/
  gateway.yml
  api.yml
  credential-injector.yml
  web.yml
  cross-layer-check.yml
```

**Path-filtered job triggering** — Woodpecker's native `when: path:` condition:

```yaml
# .woodpecker/gateway.yml
steps:
  - name: lint-and-test
    image: rust:1.XX-slim-bookworm
    commands:
      - cargo clippy --workspace -- -D warnings
      - cargo fmt --check
      - cargo test -p mcp-gateway
    when:
      - path:
          include:
            - services/gateway/**
            - libs/**
            - Cargo.toml
            - Cargo.lock
        event: [push, pull_request]

  - name: docker-build-push
    image: woodpeckerci/plugin-docker-buildx
    settings:
      repo: "${REGISTRY}/org/mcp-gateway"
      tags:
        - "${CI_COMMIT_SHA:0:8}"
        - latest
      username:
        from_secret: registry_username
      password:
        from_secret: registry_password
    when:
      - path:
          include:
            - services/gateway/**
            - libs/**
            - Cargo.toml
            - Cargo.lock
        event: push
        branch: main
```

A `libs/**` / `Cargo.toml` / `Cargo.lock` change is included in every Rust service's path filter so that shared-library changes trigger all downstream service pipelines.

**Multi-stage Dockerfile for Rust services:**

```dockerfile
# Stage 1: builder
FROM rust:1.XX-slim-bookworm AS builder
WORKDIR /build
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY libs/ libs/
COPY services/gateway/ services/gateway/
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo build --release -p mcp-gateway

# Stage 2: runtime
FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates libssl3 && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/mcp-gateway /usr/local/bin/
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/mcp-gateway"]
```

BuildKit cache mounts are mandatory. Enable via `DOCKER_BUILDKIT=1` in all Docker build steps. Without cache mounts, each CI run re-downloads the full Cargo registry.

**Cargo dependency caching** — use the `meltwater/drone-cache` plugin (Woodpecker-compatible) keyed on the `Cargo.lock` hash for `~/.cargo/registry` and `~/.cargo/git`. Do not share `target/` cache across services; include the service name in the cache key:

```yaml
  - name: restore-cache
    image: meltwater/drone-cache
    settings:
      backend: filesystem
      restore: true
      cache_key: "cargo-{{ checksum \"Cargo.lock\" }}-gateway"
      mount:
        - ~/.cargo/registry
        - ~/.cargo/git

  # ... build steps ...

  - name: rebuild-cache
    image: meltwater/drone-cache
    settings:
      backend: filesystem
      rebuild: true
      cache_key: "cargo-{{ checksum \"Cargo.lock\" }}-gateway"
      mount:
        - ~/.cargo/registry
        - ~/.cargo/git
```

**Cross-layer import check** — a shell script that reads every `libs/*/Cargo.toml` and fails if any contains a `path = "../../services/..."` dependency. Run as a dedicated pipeline step on every PR:

```yaml
# .woodpecker/cross-layer-check.yml
steps:
  - name: cross-layer-import-check
    image: bash:5
    commands:
      - scripts/check-cross-layer-imports.sh
    when:
      event: [push, pull_request]
```

**Running pipelines locally** — install `woodpecker-cli` and execute any pipeline without a running server:

```bash
# Install
go install go.woodpecker-ci.org/woodpecker/v2/cmd/woodpecker-cli@latest
# Or via package manager: brew install woodpecker-cli

# Execute a pipeline locally (mounts the current working directory)
woodpecker-cli exec .woodpecker/gateway.yml

# Pass secrets for local runs
woodpecker-cli exec .woodpecker/gateway.yml \
  --secret registry_username=myuser \
  --secret registry_password=mypass
```

**Woodpecker server local setup** — for full server + agent functionality during development, add a `woodpecker` compose profile to `docker-compose.override.yml`:

```yaml
services:
  woodpecker-server:
    image: woodpeckerci/woodpecker-server:latest
    profiles: [ci]
    ports:
      - "8000:8000"
    environment:
      WOODPECKER_OPEN: "true"
      WOODPECKER_GITEA: "true"
      WOODPECKER_GITEA_URL: http://gitea:3000
      WOODPECKER_AGENT_SECRET: local-dev-secret
    volumes:
      - woodpecker-server-data:/var/lib/woodpecker

  woodpecker-agent:
    image: woodpeckerci/woodpecker-agent:latest
    profiles: [ci]
    depends_on: [woodpecker-server]
    environment:
      WOODPECKER_SERVER: woodpecker-server:9000
      WOODPECKER_AGENT_SECRET: local-dev-secret
      WOODPECKER_MAX_WORKFLOWS: 4
    volumes:
      - /var/run/docker.sock:/var/run/docker.sock
```

Start with `docker compose --profile ci up woodpecker-server woodpecker-agent`.

#### Out of Scope

- Deployment to Railway/Fly.io (separate DevOps runbook; not a story in EPIC-00).
- Container vulnerability scanning (add `aquasec/trivy` as a pipeline step before GA; not MVP-blocking).
- Release notes generation.

---

### S-002: Docker Compose Local Development Environment

**Story ID:** S-002
**Epic:** EPIC-00
**Priority:** P0
**Estimated Effort:** 3 points
**Dependencies:** S-000, S-001

#### Description

Create a Docker Compose configuration that runs the complete system locally with a single command (`docker compose up`). Local development must support hot reload for Rust services (via `cargo-watch`) and the React frontend (via Vite HMR) without rebuilding container images. Target: a `.rs` file change is reflected in a running local gateway within 10 seconds; a `.tsx` change appears in the browser within 1 second.

#### Acceptance Criteria

- `docker compose up` starts all services: PostgreSQL, `gateway`, `api`, `credential-injector`, and the React dev server.
- All services reach healthy status within 60 seconds on a developer machine with a warm Docker image cache.
- A code change in `services/gateway/src/` causes `cargo-watch` to recompile and restart the gateway within 10 seconds on a modern laptop (Apple M2/M3 or equivalent). The developer does not restart Compose or rebuild images.
- A code change in `apps/web/src/` is reflected in the browser via Vite HMR within 1 second without a full page reload.
- PostgreSQL is accessible from the host at `localhost:5432` for developer inspection. Credentials are in `.env.example`.
- Service host ports:
  - Gateway: `localhost:3000`
  - Platform API: `localhost:3001`
  - Credential Injector: `localhost:3002` (published for local debugging; internal-only in production)
  - React dev server: `localhost:5173`
- `docker/docker-compose.override.yml` provides developer-specific volume mounts and port mappings. The base `docker-compose.yml` describes the production-like service topology. Docker Compose merges the override automatically; developers do not pass `-f` flags.
- `.env.example` documents every required environment variable with example values and explanatory comments. The `.env` file is git-ignored and never committed.
- `docker compose down -v` cleanly removes all containers and volumes; `docker compose up` after this starts from a clean state.
- The `Makefile` at the repository root provides: `make dev`, `make down`, `make logs`, `make db-migrate`, `make db-reset`.
- Docker images work on both `linux/amd64` and `linux/arm64` (Apple Silicon). CI builds and pushes both platforms.

#### Technical Notes

**Source code volume mounts for hot reload (`docker-compose.override.yml`):**

```yaml
services:
  gateway:
    volumes:
      - ./services/gateway:/app/services/gateway:cached
      - ./libs:/app/libs:cached
      - cargo-cache:/usr/local/cargo/registry
      - target-cache:/app/target
    command: >
      cargo watch
        --watch services/gateway/src
        --watch libs
        -x "run -p mcp-gateway"

volumes:
  cargo-cache:
  target-cache:
```

The named `cargo-cache` and `target-cache` volumes persist between container restarts, reducing incremental recompile from ~30s to ~3–5s.

**Service startup ordering:**

```yaml
# docker-compose.yml
services:
  api:
    depends_on:
      postgres:
        condition: service_healthy
  credential-injector:
    depends_on:
      postgres:
        condition: service_healthy
  gateway:
    depends_on:
      api:
        condition: service_healthy
      credential-injector:
        condition: service_healthy
```

The gateway must not start until the credential injector is healthy; the gateway sends all credential injection requests to the sidecar over a Unix domain socket.

**PostgreSQL health check:**

```yaml
postgres:
  image: postgres:15-alpine
  healthcheck:
    test: ["CMD-SHELL", "pg_isready -U $POSTGRES_USER -d $POSTGRES_DB"]
    interval: 5s
    timeout: 5s
    retries: 10
    start_period: 10s
```

**Network topology:** All services share a single internal Docker network (`mcp-net`). Only the ports listed above are published to the host. Inter-service communication uses Docker service names as hostnames (e.g., the gateway connects to `credential-injector:3002`).

#### Out of Scope

- Production deployment configuration (Bootstrap tier uses Railway/Fly.io; documented in separate DevOps runbook).
- Database migrations (triggered by `make db-migrate` which calls `sqlx migrate run`; schema defined in S-003).
- TLS/SSL in local development (plain HTTP is acceptable locally).

---

### S-003: PostgreSQL Database Schema v1 with sqlx Migrations

**Story ID:** S-003
**Epic:** EPIC-00
**Priority:** P0
**Estimated Effort:** 5 points
**Dependencies:** S-000

#### Description

Define and implement the complete PostgreSQL schema for the MVP. All schema changes are managed by `sqlx-cli` with forward-only migrations (no down migrations). Schema design must optimize for the two hot-path queries the gateway executes on every MCP request:
1. `SELECT config FROM mcp_servers WHERE slug = $1 AND is_active = TRUE`
2. `SELECT encrypted_value, iv FROM credentials WHERE server_id = $1`

Both must be covered by indexes. These queries execute on every `tools/list` and `tools/call` invocation; a full sequential scan at any scale is unacceptable.

#### Tables

**`users`** — Platform user records, populated via Clerk webhook on first sign-in.

```sql
CREATE TABLE users (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    clerk_id    TEXT NOT NULL UNIQUE,      -- Clerk user ID (immutable)
    email       TEXT NOT NULL,
    plan        TEXT NOT NULL DEFAULT 'free',  -- 'free', 'pro', 'team', 'enterprise'
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_users_clerk_id ON users (clerk_id);
```

**`mcp_servers`** — One row per user-created MCP server. The `config` JSONB column holds the complete server definition (tool schemas, upstream URL templates, field mappings, transform rules, auth type reference). The gateway reads this on every connection establishment.

```sql
CREATE TABLE mcp_servers (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id       UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    slug          TEXT NOT NULL UNIQUE,        -- Unique URL segment: /mcp/{slug}
    display_name  TEXT NOT NULL,
    description   TEXT,
    config        JSONB NOT NULL DEFAULT '{}', -- Full server definition
    is_active     BOOLEAN NOT NULL DEFAULT TRUE,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
-- Hot-path query index: partial index on active servers only, for gateway lookups by slug
CREATE UNIQUE INDEX idx_mcp_servers_slug_active
    ON mcp_servers (slug)
    WHERE is_active = TRUE;
CREATE INDEX idx_mcp_servers_user_id ON mcp_servers (user_id);
-- JSONB index for control-plane queries that filter/search config fields
CREATE INDEX idx_mcp_servers_config ON mcp_servers USING GIN (config);
```

**`credentials`** — Encrypted API credentials. One row per auth configuration per server. AES-256-GCM encryption. Plaintext is never stored anywhere in this table or any log.

```sql
CREATE TABLE credentials (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    server_id        UUID NOT NULL REFERENCES mcp_servers(id) ON DELETE CASCADE,
    credential_type  TEXT NOT NULL,         -- 'bearer_token' | 'api_key_header' | 'api_key_query' | 'basic_auth'
    key_name         TEXT,                  -- Header/query param name for api_key type (e.g., 'X-API-Key')
    encrypted_value  BYTEA NOT NULL,        -- AES-256-GCM ciphertext (binary; not base64)
    iv               BYTEA NOT NULL,        -- 12-byte GCM IV, unique per row, never reused
    created_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    rotated_at       TIMESTAMPTZ,           -- NULL until first rotation
    CONSTRAINT credentials_unique_per_server UNIQUE (server_id, credential_type, key_name)
);
-- Hot-path query index: gateway fetches credentials by server_id on every tool call
CREATE INDEX idx_credentials_server_id ON credentials (server_id);
```

**`server_tokens`** — Per-server bearer tokens for MCP client authentication. Only the SHA-256 hash of the raw token is stored. The raw token is shown to the user once at creation time and never retrievable thereafter.

```sql
CREATE TABLE server_tokens (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    server_id   UUID NOT NULL REFERENCES mcp_servers(id) ON DELETE CASCADE,
    token_hash  TEXT NOT NULL UNIQUE,    -- SHA-256 hex of the raw token
    hint        TEXT,                   -- Last 4 chars of raw token (display-only)
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    revoked_at  TIMESTAMPTZ             -- NULL = active; non-NULL = revoked
);
-- Hot-path query index: gateway validates bearer token on every MCP request
CREATE UNIQUE INDEX idx_server_tokens_hash_active
    ON server_tokens (token_hash)
    WHERE revoked_at IS NULL;
CREATE INDEX idx_server_tokens_server_id ON server_tokens (server_id);
```

**`audit_log`** — Append-only security audit log. No UPDATE or DELETE on this table, ever. Captures: credential access, auth failures, SSRF blocks, server CRUD operations, admin actions.

```sql
CREATE TABLE audit_log (
    id             BIGSERIAL PRIMARY KEY,     -- Sequential PK: cheap range scans; avoids UUID random-insert bloat
    timestamp      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    user_id        UUID REFERENCES users(id), -- NULL for unauthenticated/system events
    server_id      UUID REFERENCES mcp_servers(id), -- NULL for user-level events
    action         TEXT NOT NULL,             -- 'credential_access' | 'auth_failure' | 'ssrf_block' | 'server_create' | ...
    success        BOOLEAN NOT NULL,
    error_msg      TEXT,                      -- Sanitized plain-English error; NEVER contains credential values
    metadata       JSONB,                     -- Supplementary context: IP, user agent, tool name, request ID
    correlation_id TEXT                       -- Distributed trace correlation ID
);
CREATE INDEX idx_audit_log_timestamp   ON audit_log (timestamp DESC);
CREATE INDEX idx_audit_log_server_id   ON audit_log (server_id, timestamp DESC);
CREATE INDEX idx_audit_log_user_id     ON audit_log (user_id, timestamp DESC);
CREATE INDEX idx_audit_log_action      ON audit_log (action, timestamp DESC);
-- TODO (post-MVP): add row-level security policy preventing DELETE/UPDATE from app role
```

#### Acceptance Criteria

- `sqlx migrate run` against a fresh PostgreSQL 15+ instance creates all five tables with correct columns, types, constraints, and indexes — no errors.
- `sqlx migrate run` a second time is a no-op (idempotent). `sqlx migrate info` reports all migrations applied.
- All foreign key constraints enforce referential integrity: inserting a `credential` with a non-existent `server_id` fails with a constraint violation.
- `EXPLAIN ANALYZE` confirms `idx_mcp_servers_slug_active` is used for `SELECT * FROM mcp_servers WHERE slug = $1 AND is_active = TRUE` (index scan, no seq scan).
- `EXPLAIN ANALYZE` confirms `idx_server_tokens_hash_active` is used for `SELECT * FROM server_tokens WHERE token_hash = $1 AND revoked_at IS NULL` (index scan, no seq scan).
- `sqlx prepare --workspace` generates `sqlx-data.json` without errors. This file is committed to the repository. CI fails if `sqlx-data.json` is stale.
- A seed script (`migrations/seed_dev.sql`) inserts one test user, one test server, one encrypted credential (dummy ciphertext acceptable), and one server token. The script uses `INSERT ... ON CONFLICT DO NOTHING` and is idempotent. `make db-reset` runs this after migration.
- `updated_at` columns on `users` and `mcp_servers` are updated automatically by a PostgreSQL trigger on row modification.

#### Technical Notes

- Migration files are named `NNN_description.sql` (e.g., `001_initial_schema.sql`). Never renumber or rename existing migration files; sqlx uses the filename as the migration identity.
- The `config` JSONB column schema is validated in the Rust application layer via `serde` deserialization, not in PostgreSQL. This allows config schema evolution without new migrations for additive changes.
- `credentials.encrypted_value` is `BYTEA` not `TEXT`. AES-256-GCM ciphertext is binary. Base64-encoding wastes storage and CPU. Decode to display if human-readable output is required.
- `audit_log` uses `BIGSERIAL` not UUID as the primary key. Append-only high-write tables with UUID PKs suffer B-tree index bloat from random insertion order. BIGSERIAL inserts sequentially.
- `sqlx-cli` version must match the `sqlx` crate version pinned in `Cargo.lock`. Pin both in `[workspace.dependencies]`.

#### Out of Scope

- OAuth 2.0 token storage tables (post-MVP).
- Multi-tenant row-level security policies (deferred; add at Growth tier per SUMMARY.md §8).
- Partitioning `audit_log` by timestamp (add at ~10M rows, estimated 6 months post-launch).

---

### S-004: Shared Rust Libraries — mcp-protocol, mcp-crypto, mcp-common

**Story ID:** S-004
**Epic:** EPIC-00
**Priority:** P0
**Estimated Effort:** 5 points
**Dependencies:** S-000

#### Description

Implement the three shared Rust library crates that all services depend on. These crates define the shared vocabulary of the system. Their public APIs are stable contracts; breaking changes require updating all consumers simultaneously and bumping the crate version.

#### `libs/mcp-protocol` — MCP JSON-RPC 2.0 Types

Pure data types for all MCP protocol messages. No business logic. Serialization via `serde`.

Required types (aligned with MCP specification, MVP scope only):

```rust
// JSON-RPC 2.0 envelope types
pub enum RequestId { Number(i64), String(String) }

pub struct JsonRpcRequest {
    pub jsonrpc: String,   // validated: must be "2.0"
    pub id: RequestId,
    pub method: String,
    pub params: Option<serde_json::Value>,
}

pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: RequestId,
    pub result: Option<serde_json::Value>,
    pub error: Option<JsonRpcError>,
}

pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    pub data: Option<serde_json::Value>,
}

// Standard JSON-RPC error codes
pub mod error_codes {
    pub const PARSE_ERROR: i32 = -32700;
    pub const INVALID_REQUEST: i32 = -32600;
    pub const METHOD_NOT_FOUND: i32 = -32601;
    pub const INVALID_PARAMS: i32 = -32602;
    pub const INTERNAL_ERROR: i32 = -32603;
}

// MCP protocol types — MVP scope: initialize, tools/list, tools/call only
pub struct InitializeParams { pub protocol_version: String, pub client_info: ClientInfo, pub capabilities: ClientCapabilities }
pub struct InitializeResult { pub protocol_version: String, pub server_info: ServerInfo, pub capabilities: ServerCapabilities }
pub struct ToolsListResult { pub tools: Vec<ToolDefinition> }
pub struct ToolCallParams { pub name: String, pub arguments: Option<serde_json::Value> }
pub struct ToolCallResult { pub content: Vec<ToolContent>, pub is_error: Option<bool> }

pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,  // JSON Schema object
}

pub enum ToolContent {
    Text { text: String },
}
```

#### `libs/mcp-crypto` — AES-256-GCM Encryption

Encryption and decryption primitives for credential storage. This crate is the sole implementation of encryption across the entire codebase. No other crate reimplements encryption logic.

Required API:

```rust
/// Encrypts `plaintext` with AES-256-GCM using a random 12-byte IV.
/// Returns (ciphertext, iv) as separate byte vectors.
/// The IV is unique per call; callers MUST store both values.
pub fn encrypt(key: &[u8; 32], plaintext: &[u8]) -> Result<(Vec<u8>, Vec<u8>), CryptoError>;

/// Decrypts AES-256-GCM `ciphertext` using `iv`.
/// Returns plaintext bytes on success.
pub fn decrypt(key: &[u8; 32], ciphertext: &[u8], iv: &[u8]) -> Result<Vec<u8>, CryptoError>;

/// Derives a 32-byte key from an environment variable (key must be 32 bytes of base64 or hex).
/// Fails fast at startup if the key is missing, wrong length, or malformed.
pub fn load_encryption_key(env_var: &str) -> Result<[u8; 32], CryptoError>;

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("encryption failed")] EncryptionFailed,
    #[error("decryption failed: authentication tag mismatch")] DecryptionFailed,
    #[error("invalid key: {0}")] InvalidKey(String),
}
```

Implementation uses `aes-gcm` crate (audited). IV generation uses `rand::thread_rng()`. No IV is ever reused.

#### `libs/mcp-common` — Error Types, Config Loader, Telemetry Wiring

Shared cross-cutting concerns used by all three services.

**Error types:**

```rust
// Canonical application error type, usable in axum handlers
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("not found: {0}")] NotFound(String),
    #[error("unauthorized")] Unauthorized,
    #[error("forbidden")] Forbidden,
    #[error("bad request: {0}")] BadRequest(String),
    #[error("internal error")] Internal(#[from] anyhow::Error),
    #[error("upstream error: status={status}")] Upstream { status: u16, body: String },
    #[error("ssrf blocked: {url}")] SsrfBlocked { url: String },
}

// impl IntoResponse for AppError — converts to JSON error body (see S-007 for wire format)
```

**Config loader:**

```rust
/// Loads typed config from environment variables. Fails fast at startup with
/// a clear error listing all missing/malformed variables.
pub trait FromEnv: Sized {
    fn from_env() -> Result<Self, ConfigError>;
}

pub struct DatabaseConfig { pub url: String, pub max_connections: u32 }
pub struct ServerConfig { pub host: String, pub port: u16 }
pub struct EncryptionConfig { pub key_env_var: String }
pub struct ClerkConfig { pub publishable_key: String, pub secret_key: String }
```

**Telemetry wiring:**

```rust
/// Initializes OpenTelemetry SDK: tracer, meter, and log bridge.
/// Must be called once at service startup before any work begins.
/// OTEL_EXPORTER_OTLP_ENDPOINT controls the collector URL (default: http://localhost:4317).
pub fn init_telemetry(service_name: &str, service_version: &str) -> Result<TelemetryGuard, TelemetryError>;

/// Dropping this guard flushes all pending spans and metrics before process exit.
pub struct TelemetryGuard { /* opaque */ }
```

#### Acceptance Criteria

- `cargo test -p mcp-crypto` passes; tests cover: encrypt/decrypt roundtrip, decrypt with wrong key returns `Err(DecryptionFailed)`, decrypt with tampered ciphertext returns `Err(DecryptionFailed)`, no two `encrypt()` calls with the same plaintext produce the same IV.
- `cargo test -p mcp-protocol` passes; tests cover: serialize `JsonRpcRequest` to expected JSON, deserialize a raw JSON-RPC request string into `JsonRpcRequest`, deserialize a `tools/call` params object.
- `cargo test -p mcp-common` passes; tests cover: `AppError::NotFound` maps to HTTP 404 JSON body, `AppError::Unauthorized` maps to HTTP 401, `load_encryption_key` returns error for missing env var, `load_encryption_key` returns error for wrong-length value.
- All public types and functions in all three crates have `rustdoc` documentation comments. `cargo doc --no-deps --workspace` generates documentation without warnings.
- No `unwrap()` or `expect()` in production code paths (enforced by Clippy lints from S-000).

#### Out of Scope

- Actual service implementations (EPIC-01, EPIC-02).
- JSONPath transform library (used in gateway, implemented in EPIC-02).
- SSRF blocklist implementation (EPIC-01).

---

### S-005: Telemetry Foundation — OpenTelemetry Traces, Metrics, and Structured Logs

**Story ID:** S-005
**Epic:** EPIC-00
**Priority:** P0
**Estimated Effort:** 5 points
**Dependencies:** S-004

#### Description

Wire OpenTelemetry distributed tracing, Prometheus-compatible metrics, and structured logging into all three Rust services using the `init_telemetry()` function defined in `mcp-common`. Configure a local OTEL Collector in Docker Compose that receives telemetry from all services and exposes Prometheus metrics and a Jaeger-compatible trace UI for local development. Telemetry must be present from the first line of service code; retrofitting observability after the fact costs more than the initial setup.

This story matters specifically for Week 2's high-risk gateway work (SUMMARY.md §21): if the gateway's JSON-RPC parser or credential injection flow behaves unexpectedly, traces and structured logs are the primary debugging tool.

#### Acceptance Criteria

- Every incoming HTTP request to any service produces an OpenTelemetry span with: service name, HTTP method, route pattern (not raw path — avoid cardinality explosion), HTTP status code, and duration.
- Spans are exported to the local OTEL Collector container (via OTLP gRPC). The collector is included in `docker-compose.yml`.
- Jaeger UI (or compatible) is accessible at `http://localhost:16686` in the local Docker Compose environment and shows traces from all three services.
- Every log line is emitted as structured JSON (not plain text) with fields: `timestamp` (RFC 3339), `level`, `service`, `version`, `message`, `trace_id` (when inside a span), `span_id` (when inside a span). The `trace_id` enables correlating log lines with traces.
- Prometheus-compatible metrics are exposed at `/metrics` on each service. Required metrics at minimum:
  - `http_requests_total{service, method, route, status_code}` — counter
  - `http_request_duration_seconds{service, method, route}` — histogram
  - `db_query_duration_seconds{service, query_name}` — histogram (for sqlx queries)
- `OTEL_EXPORTER_OTLP_ENDPOINT` environment variable controls the collector URL. Setting `OTEL_SDK_DISABLED=true` disables all telemetry (for test environments where OTLP export is not wanted).
- Dropping the `TelemetryGuard` returned by `init_telemetry()` flushes all pending spans before process exit. A test verifies this by initializing telemetry, emitting one span, dropping the guard, and confirming the span reached the collector (via a test OTLP receiver).
- Credential values, bearer tokens, and personally identifiable data (email except in user-context logs) are never present in any span attribute, log field, or metric label.

#### Technical Notes

- Use the `opentelemetry` + `opentelemetry-otlp` + `opentelemetry-sdk` crates. Use `tracing` + `tracing-opentelemetry` for the bridge between Rust's `tracing` ecosystem and OTEL.
- Use `tracing-subscriber` with `EnvFilter` (respects `RUST_LOG` / `OTEL_LOG_LEVEL`) and a JSON formatter layer (`tracing-subscriber::fmt::layer().json()`).
- The OTEL Collector in `docker-compose.yml` uses the official `otel/opentelemetry-collector-contrib` image with a minimal config: OTLP receiver → Prometheus exporter + Jaeger exporter.
- Route-level metric labels use axum's `MatchedPath` extractor to capture the route pattern (e.g., `/mcp/:server_slug`) not the raw URL. This prevents high cardinality from user-provided slug values.
- The `TelemetryGuard` implements `Drop` by calling `opentelemetry::global::shutdown_tracer_provider()` and `opentelemetry::global::shutdown_meter_provider()`.

#### Out of Scope

- Production observability backend (Datadog/Prometheus remote write) — configured at deployment time via environment variables; the code is already compatible.
- Log aggregation pipeline (Loki, Elasticsearch) — local dev uses stdout JSON; production configuration is a DevOps concern.
- Alerting rules — post-MVP.

---

### S-006: Configuration Management — Environment-Validated Config with Fail-Fast Startup

**Story ID:** S-006
**Epic:** EPIC-00
**Priority:** P0
**Estimated Effort:** 3 points
**Dependencies:** S-004

#### Description

Implement a consistent, validated, environment-variable-based configuration system for all three services, using the `FromEnv` trait defined in `mcp-common`. Each service reads its configuration at startup, validates that all required variables are present and well-formed, and fails immediately with a clear human-readable error listing every missing/malformed variable before binding to any port. Silent misconfiguration in production is eliminated.

This is P0 because misconfigured services that start but behave incorrectly are harder to debug than services that refuse to start at all, and because Week 2's gateway work depends on the credential injector URL and encryption key being reliably loaded.

#### Acceptance Criteria

- Each service defines a top-level `Config` struct implementing `FromEnv` that collects all required environment variables in one place.
- Starting any service with a missing required environment variable (e.g., `DATABASE_URL`) prints a clear error: `Configuration error: missing required environment variable DATABASE_URL` and exits with a non-zero status code before attempting to bind to any port or connect to any dependency.
- Starting any service with a malformed value (e.g., `PORT=not-a-number`) prints: `Configuration error: PORT must be a valid port number (1-65535), got "not-a-number"` and exits.
- When multiple environment variables are missing, all missing variables are reported in a single error message (not just the first one). Developers fix all errors at once, not one at a time.
- `.env.example` at the repository root lists every variable for every service with: the variable name, a human-readable description, an example value, and whether it is required or optional with default.
- Running `cargo test -p mcp-common` includes tests: `FromEnv` succeeds with all required vars set, fails with single missing var, fails with multiple missing vars (error message lists both).
- The configuration for each service is documented in the service's own `README.md` within its directory.

#### Required Environment Variables per Service

**All services:**
| Variable | Description | Example |
|---|---|---|
| `RUST_LOG` | Log filter (optional, default: `info`) | `info,mcp_gateway=debug` |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | OTEL collector gRPC endpoint | `http://otel-collector:4317` |
| `OTEL_SDK_DISABLED` | Disable telemetry export (optional) | `true` |

**`platform-api`:**
| Variable | Description | Example |
|---|---|---|
| `DATABASE_URL` | PostgreSQL connection URL | `postgres://user:pass@localhost/mcp` |
| `PORT` | HTTP bind port | `3001` |
| `CLERK_SECRET_KEY` | Clerk backend secret key | `sk_live_...` |
| `ENCRYPTION_KEY` | 32-byte AES key (base64) | `<32-byte base64>` |

**`gateway`:**
| Variable | Description | Example |
|---|---|---|
| `DATABASE_URL` | PostgreSQL connection URL | `postgres://user:pass@localhost/mcp` |
| `PORT` | HTTP bind port | `3000` |
| `CREDENTIAL_INJECTOR_URL` | Sidecar Unix socket or HTTP URL | `http://credential-injector:3002` |

**`credential-injector`:**
| Variable | Description | Example |
|---|---|---|
| `DATABASE_URL` | PostgreSQL connection URL | `postgres://user:pass@localhost/mcp` |
| `PORT` | HTTP bind port | `3002` |
| `ENCRYPTION_KEY` | 32-byte AES key (base64) | `<32-byte base64>` |

#### Technical Notes

- Use the `envy` crate or manual `std::env::var()` — the preference is manual parsing to produce the multi-error aggregation described above. `envy` returns only the first error.
- Implement error aggregation using a `Vec<String>` of error messages; if non-empty after all vars are checked, join them with newlines and return a single `ConfigError::Multiple(Vec<String>)`.
- Config structs are instantiated exactly once, at the top of `main()`, before any async runtime or axum router is created. Pass `Arc<Config>` to all handlers via axum state.

#### Out of Scope

- Secret fetching from a secrets manager (AWS Secrets Manager, Vault) — environment variables are the interface; the source of those values in production is a DevOps concern.
- Runtime config reloading — config is static per process lifetime; hot reload of MCP server configs is handled by LISTEN/NOTIFY in the gateway (EPIC-02), not the service config.

---

### S-007: API Error Response Contract — Standard Error Shape for Platform API

**Story ID:** S-007
**Epic:** EPIC-00
**Priority:** P0
**Estimated Effort:** 2 points
**Dependencies:** S-000, S-004

#### Description

Define and document the standard HTTP error response body shape that the Platform API will return for all error conditions. This contract is established in EPIC-00 so that the frontend (built in EPIC-03) can be coded against a stable shape from the start, and so all EPIC-01 service stories implement it consistently without per-story debate about error formats.

This is an API design and scaffolding task, not a full service implementation. It produces: the canonical error struct, the axum `IntoResponse` implementation for `AppError`, a JSON Schema for the error body, and documentation.

#### Error Response Wire Format

All Platform API error responses use HTTP status codes correctly and return a JSON body:

```json
{
  "error": {
    "code": "server_not_found",
    "message": "No MCP server found with slug 'my-api'.",
    "request_id": "req_01ABCDE..."
  }
}
```

| Field | Type | Description |
|---|---|---|
| `error.code` | string | Machine-readable snake_case error code (stable across versions) |
| `error.message` | string | Human-readable English description (may change across versions) |
| `error.request_id` | string | Correlation ID for support and log tracing (optional in tests) |

**HTTP status ↔ error code mapping:**

| HTTP Status | Error Code | Description |
|---|---|---|
| 400 | `bad_request` | Invalid input, missing required field, or malformed value |
| 401 | `unauthorized` | Missing or invalid authentication token |
| 403 | `forbidden` | Authenticated but not authorized to access this resource |
| 404 | `not_found` | Resource does not exist or is not visible to the caller |
| 409 | `conflict` | Resource conflict (e.g., duplicate slug) |
| 422 | `validation_error` | Input passes parsing but fails business rule validation |
| 429 | `rate_limited` | Rate limit exceeded |
| 500 | `internal_error` | Unhandled internal error; details in server logs |
| 502 | `upstream_error` | Upstream API returned an unexpected response |
| 503 | `service_unavailable` | Dependency unavailable (database, credential injector) |

#### Acceptance Criteria

- `AppError::into_response()` (from `libs/mcp-common`) produces the JSON body shape above for every variant.
- Every error response includes a `request_id` header (`X-Request-ID`) matching the `error.request_id` field in the body. The request ID is generated at the axum middleware layer (one per request) and injected into the span context.
- `cargo test -p mcp-common` includes tests: `AppError::NotFound("x")` serializes to `{"error": {"code": "not_found", "message": "..."}}` with HTTP 404, `AppError::Unauthorized` serializes to HTTP 401.
- A JSON Schema file (`docs/api/error-schema.json`) describes the error response shape. The frontend team references this schema for type generation.
- The error response shape is documented in `docs/api/README.md` with examples for each HTTP status code.
- Error messages never expose internal implementation details (stack traces, SQL error messages, raw file paths).

#### Technical Notes

- The `request_id` is a `ulid` (lexicographically sortable, URL-safe) generated per-request by an axum middleware layer. This middleware is implemented as part of this story and is applied to all routes.
- The `IntoResponse` implementation for `AppError` uses axum's `Json` extractor to serialize the error body and sets the HTTP status code from the variant.
- Internal errors (`AppError::Internal`) log the original error at `ERROR` level (with full context and trace ID) but return only `"code": "internal_error"` to the client, never the internal error message.

#### Out of Scope

- Full Platform API service scaffolding (EPIC-01).
- MCP gateway error responses (the gateway uses JSON-RPC 2.0 error format, not this HTTP API format; defined in `mcp-protocol` in S-004).
- API versioning URL prefix (`/v1/`) — applied when routes are defined in EPIC-01, guided by the contract established here.

---

### S-008: Health Check Endpoints — Liveness, Readiness

**Story ID:** S-008
**Epic:** EPIC-00
**Priority:** P0
**Estimated Effort:** 2 points
**Dependencies:** S-004

#### Description

Add `/health/live` and `/health/ready` endpoints to all three Rust services. These endpoints are used by Docker Compose `healthcheck` directives (established in S-002) and will be used by the Bootstrap-tier hosting platform (Railway/Fly.io) for deployment health gating. They must be implemented before any EPIC-01 work begins because S-002's Docker Compose `depends_on: condition: service_healthy` requires them.

#### Endpoint Specifications

**`GET /health/live`** — Liveness probe. Answers: "Is this process alive?" Does not check external dependencies. Always returns 200 if the process can handle HTTP requests.

Response `200 OK`:
```json
{ "status": "ok", "service": "mcp-gateway", "version": "0.1.0" }
```

**`GET /health/ready`** — Readiness probe. Answers: "Is this service ready to handle traffic?" Checks critical dependencies. Returns 200 only when all dependencies are healthy; returns 503 otherwise.

Response `200 OK` (ready):
```json
{
  "status": "ready",
  "service": "mcp-gateway",
  "version": "0.1.0",
  "checks": {
    "database": "ok",
    "credential_injector": "ok"
  }
}
```

Response `503 Service Unavailable` (not ready):
```json
{
  "status": "degraded",
  "service": "mcp-gateway",
  "version": "0.1.0",
  "checks": {
    "database": "ok",
    "credential_injector": "error: connection refused"
  }
}
```

**Dependency checks per service:**

| Service | Dependency Checks |
|---|---|
| `gateway` | PostgreSQL (one `SELECT 1`), Credential Injector (`GET /health/live`) |
| `platform-api` | PostgreSQL (one `SELECT 1`) |
| `credential-injector` | PostgreSQL (one `SELECT 1`) |

#### Acceptance Criteria

- All three services expose `GET /health/live` returning `200` immediately after startup (no dependency checks).
- All three services expose `GET /health/ready` returning `200` when all dependencies are healthy, `503` when any dependency is unreachable.
- `GET /health/ready` completes within 2 seconds (timeout on each dependency check).
- Health check endpoints are excluded from authentication middleware (no bearer token required).
- Health check endpoints are excluded from rate limiting middleware.
- Health check endpoints do not emit a full OTEL trace span (to avoid polluting traces with polling noise). They may emit a metric increment for monitoring.
- `docker compose up` reliably reaches all-healthy status using the `condition: service_healthy` checks from S-002, which call `/health/ready`.
- `cargo test` includes a test for each service: simulate a healthy DB → `GET /health/ready` returns 200; simulate a DB failure → `GET /health/ready` returns 503.

#### Technical Notes

- Implement health check routes as a separate axum `Router` that is merged into the main router but before auth/rate-limit middleware is applied.
- The `version` field is populated from `CARGO_PKG_VERSION` at compile time via `env!("CARGO_PKG_VERSION")`.
- Keep dependency check timeouts short (500ms per check, 1.5s total). Health probes that take 30s are useless.
- Do not share the main database connection pool for health checks; use a dedicated `sqlx::PgPool` with `max_connections: 1` to avoid competing with real requests.

#### Out of Scope

- Deep health checks (e.g., reading from a specific table, verifying encryption key) — liveness and readiness checks only.
- Startup probe (separate from liveness) — added if Railway/Fly.io requires it during deployment; not needed for local Docker Compose.
- Metrics endpoint (`/metrics`) — defined in S-005; referenced here for completeness only.

---

### S-009: Integration Test Framework — Test Database, Mock Upstream, and MCP Client Harness

**Story ID:** S-009
**Epic:** EPIC-00
**Priority:** P1
**Estimated Effort:** 5 points
**Dependencies:** S-000, S-003, S-004

#### Description

Build the shared integration testing infrastructure used by all service-level tests in subsequent epics. The three components are: (1) `TestDatabase` — spins up an isolated PostgreSQL database per test, runs migrations, and tears down after the test; (2) `MockUpstream` — a lightweight HTTP server for simulating upstream REST APIs in gateway tests; (3) `TestMcpClient` — a minimal MCP JSON-RPC client for end-to-end gateway assertions. These live in `libs/common/src/testing/`, gated by a `testing` Cargo feature that is never compiled into production binaries.

**This is P1** (not P0) because it supports and validates the P0 work but does not block Day 1 of EPIC-01 development. It should complete within the same sprint as EPIC-01's first P0 stories.

#### Components

**1. `TestDatabase`** — Per-test isolated PostgreSQL database via `testcontainers-rs`:

```rust
pub struct TestDatabase {
    pool: PgPool,
    db_name: String,  // unique per instance: format!("mcp_test_{}", uuid)
}

impl TestDatabase {
    pub async fn new() -> Self;          // creates DB, runs migrations
    pub fn pool(&self) -> &PgPool;       // returns connection pool
    pub async fn seed_default(&self);    // runs seed_dev.sql
}

impl Drop for TestDatabase {
    fn drop(&mut self) { /* async cleanup: DROP DATABASE */ }
}
```

**2. `MockUpstream`** — Lightweight in-process HTTP stub:

```rust
pub struct MockUpstream {
    pub base_url: String,  // e.g., "http://127.0.0.1:34501"
}

impl MockUpstream {
    pub fn new() -> Self;  // binds to random port

    // Register a handler: on matching request, respond with StubResponse
    pub fn stub(&self, method: &str, path: &str, response: StubResponse) -> &Self;

    // Assert that a request was received with the given header
    pub fn assert_received_header(&self, method: &str, path: &str, header: &str, value: &str);

    // Assert total number of requests to this endpoint
    pub fn assert_call_count(&self, method: &str, path: &str, expected: usize);
}

pub struct StubResponse {
    pub status: u16,
    pub body: serde_json::Value,
    pub delay_ms: Option<u64>,  // simulate slow upstreams for timeout tests
}
```

**3. `TestMcpClient`** — Minimal MCP JSON-RPC 2.0 client for gateway end-to-end tests:

```rust
pub struct TestMcpClient {
    gateway_url: String,
    bearer_token: String,
}

impl TestMcpClient {
    pub fn new(gateway_url: &str, server_slug: &str, bearer_token: &str) -> Self;

    pub async fn initialize(&self) -> Result<InitializeResult, TestError>;
    pub async fn tools_list(&self) -> Result<ToolsListResult, TestError>;
    pub async fn tools_call(
        &self,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<ToolCallResult, TestError>;
}
```

#### Acceptance Criteria

- `cargo test --workspace --features testing` passes with 10 parallel integration tests each using their own `TestDatabase` — no cross-test data contamination.
- Dropping a `TestDatabase` instance removes the isolated database. A subsequent `SHOW DATABASES` does not list it.
- `MockUpstream::assert_received_header` can verify that the credential injector added the correct `Authorization: Bearer <token>` header to an upstream request, without the test body knowing the plaintext credential (the test seeds an encrypted credential; the gateway decrypts and injects it; the mock captures and asserts the final header value).
- `TestMcpClient` can perform a full `initialize` → `tools/list` → `tools/call` sequence against a locally running gateway instance backed by a `TestDatabase` and a `MockUpstream`. The `tools/call` response matches the field mappings defined in the test server config.
- Integration tests for `platform-api` cover: user upsert (Clerk sync), server create/read/update/delete, credential write (verify ciphertext stored, not plaintext), server token generation and revocation.
- Integration tests for `gateway` cover: `tools/list` returns correct tool definitions from config, `tools/call` proxies to `MockUpstream` and applies JSONPath field mappings, SSRF-blocked URL returns correct JSON-RPC error, invalid bearer token returns HTTP 401.
- The full integration test suite runs in CI (Woodpecker CI) in under 90 seconds including PostgreSQL container startup.
- The `testing` Cargo feature is never compiled in production builds. CI verifies: `cargo build --release --workspace` succeeds with `testing` feature absent.

#### Technical Notes

- Use the `testcontainers` crate (not `testcontainers-modules`). Use the `postgres` container image. Configure `POSTGRES_DB=mcp_test`.
- Each `TestDatabase` creates a uniquely-named database via a superuser connection, then connects to it and runs `sqlx migrate run`. After the test, the drop implementation runs `DROP DATABASE {db_name}` via the superuser connection.
- `MockUpstream` binds to `127.0.0.1:0` (OS-assigned port). Use `TcpListener::bind("127.0.0.1:0")` and read the port from `listener.local_addr()`.
- For gateway integration tests, spawn the gateway as a `tokio::task` within the test process, injecting the test config (pointing to `TestDatabase` pool and `MockUpstream` URL). Avoid launching a separate process for speed.
- Gate the entire `testing` module behind `#[cfg(feature = "testing")]`. Declare the feature in `[features]` in `libs/common/Cargo.toml` with `default = []`.

---

## Epic Summary Table

| Story | Title | Points | Priority | Dependencies |
|-------|-------|--------|----------|--------------|
| S-000 | Monorepo Scaffolding | 3 | P0 | None |
| S-001 | CI/CD Pipeline | 5 | P0 | S-000 |
| S-002 | Docker Compose Local Dev | 3 | P0 | S-000, S-001 |
| S-003 | PostgreSQL Schema v1 | 5 | P0 | S-000 |
| S-004 | Shared Rust Libraries | 5 | P0 | S-000 |
| S-005 | Telemetry Foundation | 5 | P0 | S-004 |
| S-006 | Configuration Management | 3 | P0 | S-004 |
| S-007 | API Error Response Contract | 2 | P0 | S-000, S-004 |
| S-008 | Health Check Endpoints | 2 | P0 | S-004 |
| S-009 | Integration Test Framework | 5 | P1 | S-000, S-003, S-004 |
| **Total** | | **38** | | |

---

## Critical Path

```
S-000 (Monorepo)
  ├── S-001 (CI/CD)
  │     └── S-002 (Docker Compose)
  ├── S-003 (PostgreSQL Schema)          ← parallel with S-004
  └── S-004 (Shared Libs)
        ├── S-005 (Telemetry)            ─┐
        ├── S-006 (Config Management)    ─┤ parallel
        ├── S-007 (Error Contract)       ─┤
        └── S-008 (Health Checks)        ─┘
              └── [EPIC-01 begins]
S-009 (Integration Test Framework)       ← P1; parallel with early EPIC-01, completes by end of sprint 1
```

**Minimum unblocking path to EPIC-01:** S-000 → S-003 + S-004 (parallel) → S-005 + S-006 + S-007 + S-008 (parallel).

**S-001** and **S-002** can be worked in parallel from S-000 and do not block EPIC-01 conceptually, but CI must be green before any EPIC-01 PR is merged.

**S-009** is P1: valuable, does not block EPIC-01 story starts, but must complete before end of the sprint in which EPIC-01 P0 stories land (integration tests are the primary verification for gateway work in Week 2).

---

## Risks and Mitigations

| Risk | Probability | Impact | Mitigation |
|------|-------------|--------|-----------|
| Rust compilation cold-start time slows CI beyond 5-min target | Medium | Schedule | BuildKit layer caching + `sccache` for Cargo registry; enforce in S-001 |
| `cargo-watch` on mounted Docker volumes is slow on macOS | High | Developer experience | Named volumes for `target/` and cargo registry avoid bind-mount overhead; documented in S-002 |
| sqlx offline mode (`sqlx-data.json`) gets stale frequently | Medium | CI noise | `sqlx prepare` runs in CI; stale file = build failure, which forces developers to regenerate |
| TR-1 from SUMMARY.md: no mature Rust MCP library (40%) | Medium | Week 2 timeline | `mcp-protocol` crate (S-004) is our own implementation; if any upstream library is chosen later, S-004 types become adapters |
| Team unfamiliar with OpenTelemetry Rust SDK | Low | S-005 schedule | `init_telemetry()` is isolated in `mcp-common`; one engineer owns it; surface area is small |

---

## Definition of Done (EPIC-00)

EPIC-00 is complete when:

1. All P0 stories have been reviewed, approved, and merged to `main`.
2. `docker compose up && docker compose ps` shows all services healthy.
3. `cargo build --workspace && cargo test --workspace` pass with zero warnings on CI.
4. The PostgreSQL schema is applied and `sqlx-data.json` is committed and current.
5. Telemetry is visible in the local Jaeger UI for a manual health-check request.
6. A new engineer (or the team itself, cold) can reach a working local environment in ≤ 15 minutes following only `README.md`.
7. S-009 (P1) is merged or has an active, unblocked PR before any EPIC-01 story is reviewed.
