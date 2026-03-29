# Recast MCP

Recast MCP is a hosted, no-code platform that exposes any REST API to AI agents (Claude, Cursor, ChatGPT) as a live MCP server.

## Directory Layout

```
recast_mcp/
├── services/              # Rust services
│   ├── gateway/          # MCP protocol proxy (port 3000)
│   ├── api/              # Platform control plane (port 3001)
│   └── credential-injector/  # Credential sidecar (port 3002)
├── libs/                 # Shared Rust libraries
│   ├── mcp-protocol/     # MCP JSON-RPC types
│   ├── mcp-crypto/       # AES-256-GCM encryption
│   └── common/           # Shared types, errors, utilities
├── apps/                 # Frontend applications
│   └── web/              # React 19 + TypeScript web UI
├── Cargo.toml            # Rust workspace root
├── Cargo.lock            # Rust workspace lockfile
├── package.json          # Node.js workspace root (pnpm)
├── pnpm-lock.yaml        # Frontend lockfile
├── rust-toolchain.toml   # Pinned Rust version
├── .cargo/config.toml    # Cargo build configuration
└── docs/                 # Project documentation
```

## Tech Stack

- **Rust:** axum, tokio, serde, sqlx, aes-gcm
- **Frontend:** React 19, TypeScript, Vite, Zustand
- **Database:** PostgreSQL
- **Auth:** Clerk
- **CI/CD:** Woodpecker CI
- **Container:** Docker + docker-compose

## Prerequisites

- Rust (stable) — version pinned in `rust-toolchain.toml`
- Node.js 18+ and pnpm 9.0+
- PostgreSQL 15+ (for local development via docker-compose)
- Docker and docker-compose (optional, for containerized local setup)

## Quick Start

### Rust Setup

```bash
# Install Rust (if not already installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Build all Rust crates
cargo build --workspace

# Run tests
cargo test --workspace

# Check for warnings
cargo clippy --workspace -- -D warnings

# Run formatter check
cargo fmt --check
```

### Frontend Setup

```bash
# Install pnpm (if not already installed)
npm install -g pnpm

# Install frontend dependencies
pnpm install

# Type-check the web app
pnpm --filter @mcp/web type-check

# Build the web app
pnpm --filter @mcp/web build

# Run lint checks
pnpm --filter @mcp/web lint

# Start dev server (runs on localhost:5173)
pnpm --filter @mcp/web dev
```

### Local Development with Docker Compose

Coming soon — see TASK-005 in the roadmap.

## Architecture

### Services

#### Gateway (services/gateway)
Multi-tenant MCP proxy that routes incoming MCP requests to configured upstream APIs. Handles JSON-RPC 2.0 over HTTP and SSE.

#### Platform API (services/api)
Control plane for managing MCP servers, credentials, and authentication tokens. Provides REST API for the web UI.

#### Credential Injector (services/credential-injector)
Sidecar service that decrypts credentials and injects them into upstream requests via Unix domain socket.

### Libraries

#### mcp-protocol (libs/mcp-protocol)
Serializable types for MCP messages: `JsonRpcRequest`, `JsonRpcResponse`, `JsonRpcError`. Implements serialize/deserialize for JSON-RPC 2.0 compliance.

#### mcp-crypto (libs/mcp-crypto)
AES-256-GCM encryption/decryption for credentials. Generates random 12-byte IVs per encryption, stores IV + ciphertext.

#### common (libs/common)
Shared error types (`AppError`), traits (`FromEnv` for config loading), and utilities used by all services.

## Code Style and Lints

- **Clippy rules:** `deny(unwrap_used)`, `deny(expect_used)`, `deny(panic)`, `deny(unimplemented)`, `deny(todo)` — no panics in production code
- **Rust edition:** 2021
- **Formatting:** `cargo fmt` (enforced in CI)
- **Docs:** All public APIs must have rustdoc comments; `cargo doc --no-deps` must generate without warnings

## Testing

- All services include unit tests in `src/`
- Integration tests are gated behind a `testing` Cargo feature (see TASK-010 in roadmap)
- Run with `cargo test --workspace`

## Documentation

- **[docs/SUMMARY.md](docs/SUMMARY.md)** — Full product specification
- **[docs/stories/](docs/stories/)** — Epics and task breakdown
- **[CLAUDE.md](CLAUDE.md)** — Codebase conventions and build guidance

## Roadmap

The project is organized into 10 foundational tasks (TASK-001 through TASK-010) covering:
1. Monorepo scaffolding ✓
2. PostgreSQL schema
3. Shared libraries
4. CI/CD pipeline
5. Docker Compose local dev
6. Telemetry (OpenTelemetry)
7. Config management
8. API error contracts
9. Health check endpoints
10. Integration test framework

Followed by 6 epics:
- EPIC-01: Foundation Services
- EPIC-02: Gateway Core
- EPIC-03: Builder UI
- EPIC-04: Deployment & Ops
- EPIC-05: Scalability
- EPIC-06: Advanced Features

## Contributing

Ensure all code:
- Builds without warnings: `cargo build --workspace`
- Passes clippy: `cargo clippy --workspace -- -D warnings`
- Is formatted: `cargo fmt`
- Has tests: `cargo test --workspace`
- Has docs: `cargo doc --no-deps`

## License

Apache-2.0
