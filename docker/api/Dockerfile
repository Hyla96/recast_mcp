# syntax=docker/dockerfile:1
#
# Multi-stage build for the Recast MCP Platform API service.
#
# Stages:
#   chef     — installs cargo-chef for dependency caching
#   planner  — generates a dependency recipe from workspace manifests
#   builder  — caches deps (recipe), then compiles the release binary
#   runtime  — minimal debian:bookworm-slim image with only the binary
#
# Build:
#   docker buildx build --platform linux/amd64,linux/arm64 \
#     -f docker/api/Dockerfile -t mcp-api:latest --push .
#
# Run:
#   docker run --env-file .env -p 3001:3001 mcp-api:latest

# ─── Stage 1: cargo-chef installer ──────────────────────────────────
FROM rust:1.85-slim AS chef

RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config \
    && rm -rf /var/lib/apt/lists/* \
    && cargo install cargo-chef --locked

WORKDIR /build

# ─── Stage 2: Recipe generation ─────────────────────────────────────
FROM chef AS planner

COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ─── Stage 3: Dependency caching + release build ────────────────────
FROM chef AS builder

COPY --from=planner /build/recipe.json recipe.json
ENV SQLX_OFFLINE=true
RUN cargo chef cook --release --recipe-path recipe.json

COPY . .
RUN SQLX_OFFLINE=true cargo build --release -p mcp-api

# ─── Stage 4: Minimal runtime image ─────────────────────────────────
FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/mcp-api /usr/local/bin/mcp-api

USER 1000:1000

EXPOSE 3001

CMD ["/usr/local/bin/mcp-api"]
