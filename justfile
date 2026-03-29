# Recast MCP — project commands
# https://github.com/casey/just

set dotenv-load

# ─── Defaults ───────────────────────────────────────────────────────

# List available recipes
default:
    @just --list

# ─── Development ────────────────────────────────────────────────────

# Build all Rust crates
build:
    cargo build

# Build all Rust crates in release mode
build-release:
    cargo build --release

# Run all Rust tests
test:
    cargo test --workspace

# Run Clippy lints
lint:
    cargo clippy --workspace --all-targets -- -D warnings

# Format all Rust code
fmt:
    cargo fmt --all

# Check Rust formatting without modifying files
fmt-check:
    cargo fmt --all -- --check

# Run all checks (fmt, lint, test)
check: fmt-check lint test

# ─── Frontend (apps/web) ───────────────────────────────────────────

# Install frontend dependencies
fe-install:
    pnpm install

# Start frontend dev server
fe-dev:
    pnpm --filter @mcp/web dev

# Build frontend for production
fe-build:
    pnpm --filter @mcp/web build

# Lint frontend code
fe-lint:
    pnpm --filter @mcp/web lint

# ─── Database ──────────────────────────────────────────────────────

# Run sqlx migrations against DATABASE_URL
db-migrate:
    cargo sqlx migrate run

# Check migration status
db-status:
    cargo sqlx migrate info

# Create a new forward-only migration file
db-new-migration name:
    cargo sqlx migrate add {{name}}

# Seed the database with development data (idempotent)
db-seed:
    psql $DATABASE_URL -f migrations/seed_dev.sql

# Reset: drop and recreate the database, re-run migrations, re-seed
db-reset:
    sqlx database drop --force
    sqlx database create
    cargo sqlx migrate run
    psql $DATABASE_URL -f migrations/seed_dev.sql

# Prepare sqlx offline query cache (.sqlx/ directory) — requires DATABASE_URL
db-prepare:
    cargo sqlx prepare --workspace

# ─── Individual Services ───────────────────────────────────────────

# Run the gateway service
run-gateway:
    cargo run -p recast-gateway

# Run the platform API service
run-api:
    cargo run -p recast-api

# Run the credential injector sidecar
run-injector:
    cargo run -p recast-credential-injector

# ─── Docker ────────────────────────────────────────────────────────

# Build all Docker images
docker-build:
    docker compose build

# Start all services via Docker Compose
docker-up:
    docker compose up -d

# Stop all services
docker-down:
    docker compose down

# ─── Housekeeping ──────────────────────────────────────────────────

# Remove build artifacts
clean:
    cargo clean
    rm -rf apps/web/dist
