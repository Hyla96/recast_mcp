# Gateway Deployment Guide

## Overview

The gateway is a **stateless** Rust/axum process. Every piece of per-request routing state is derived from the in-memory config cache (pre-warmed from PostgreSQL at startup and kept in sync via LISTEN/NOTIFY). Multiple instances can run concurrently without coordination.

---

## Stateless Design

- **Config cache**: Each instance maintains its own warm cache seeded from PostgreSQL on startup. Hot-reload via `LISTEN/NOTIFY` keeps instances in sync within 2 seconds of a config change. The cache is a read-through layer; PostgreSQL is the source of truth.
- **Auth tokens**: Argon2id verification with a 30-second moka TTL cache per instance. No shared session state.
- **Circuit breakers**: Per-server, per-instance. State is NOT shared between instances. This is intentional — a flapping upstream will open circuit breakers on instances that observe failures independently.
- **SSE sessions**: Stored in a per-instance `DashMap`. Clients using the legacy SSE transport (`/sse/:slug`) require sticky sessions (see Load Balancer Config below).

---

## Load Balancer Configuration

### Primary transport (`POST /mcp/:slug`)

- **Routing**: Round-robin. The Streamable HTTP transport is stateless — any instance can serve any request.
- **Health check**: `GET /healthz/ready` (HTTP 200 = ready, 503 = not ready).
- **Liveness check**: `GET /healthz/live` (always 200 while process is alive).
- **Drain**: On `SIGTERM`, the gateway sets its readiness probe to 503 for 5 seconds before stopping TCP accept. Configure your LB to drain connections for at least 5 seconds after a 503 readiness response before removing the instance from the pool.

### Legacy SSE transport (`GET /sse/:slug`, `POST /messages/:slug`)

- **Routing**: Sticky sessions are required. The `GET /sse/:slug` endpoint establishes a long-lived SSE connection on one instance; subsequent `POST /messages/:slug` requests with the `session_id` parameter must route to the same instance.
- **Session affinity**: Use IP hash or a custom `X-Session-ID` affinity rule pointing at the `session_id` query parameter.
- **Session timeout**: 120 seconds of inactivity closes the SSE connection server-side.

---

## Environment Variables

| Variable | Default | Description |
|---|---|---|
| `DATABASE_URL` | (required) | PostgreSQL connection URL |
| `GATEWAY_PORT` | `3000` | TCP port to bind |
| `INJECTOR_SOCKET_PATH` | `/tmp/recast-injector.sock` | Unix socket to the credential injector sidecar |
| `GATEWAY_MAX_CONNECTIONS` | `10000` | Global in-flight request limit across all servers |
| `LOG_LEVEL` | `info` | Log verbosity: `trace`, `debug`, `info`, `warn`, `error` |
| `METRICS_TOKEN` | (unset) | Static bearer token for `GET /metrics`. Unset = open (dev only). |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4317` | OTLP gRPC endpoint for traces |
| `OTEL_SDK_DISABLED` | `false` | Set `true` to disable OpenTelemetry entirely |
| `GATEWAY_ALLOW_HTTP` | `false` | Allow non-HTTPS upstream URLs (dev only) |

---

## Health Check Contract

### `GET /healthz/live`

Returns HTTP 200 immediately as long as the process is alive. Does not check any dependencies.

```json
{"status": "ok", "instance_id": "550e8400-e29b-41d4-a716-446655440000"}
```

### `GET /healthz/ready`

Returns HTTP 200 when all three readiness conditions are met:

1. Config cache initial load completed.
2. PostgreSQL LISTEN connection established.
3. Credential injector sidecar socket reachable.

```json
{"status": "ready", "instance_id": "550e8400-...", "cache_entries": 4200}
```

Returns HTTP 503 with a `reason` field when any check fails:

```json
{"status": "not_ready", "instance_id": "550e8400-...", "reason": "sidecar socket unreachable"}
```

> **Note**: During graceful shutdown, the readiness probe returns 503 for the first 5 seconds after `SIGTERM` to signal the load balancer to drain traffic. Use `/healthz/ready` (not `/healthz/live`) for LB health checks so the drain works correctly.

### Legacy probes (`/health/live`, `/health/ready`)

The shared mcp-common probes are also mounted for backward compatibility. `/health/ready` checks only PostgreSQL connectivity and does not include `instance_id` or the full gateway readiness conditions. Prefer `/healthz/ready` for new deployments.

### `GET /metrics`

Returns Prometheus text-format metrics. Requires `Authorization: Bearer <METRICS_TOKEN>` when `METRICS_TOKEN` is configured. Returns HTTP 401 with `WWW-Authenticate: Bearer realm="metrics"` otherwise.

---

## Scale-Out Procedure

1. **Start new instance**: The new instance will load all active server configs from PostgreSQL (`load_all()`) during startup. For 100,000 rows this completes within 5 seconds on a healthy DB.
2. **Wait for readiness**: The LB should only route traffic to an instance after `GET /healthz/ready` returns 200. This ensures the config cache is warm.
3. **Verify LISTEN**: The readiness probe checks that the LISTEN/NOTIFY subscription is established. If the DB is temporarily unreachable, the probe stays 503 until the connection is restored.
4. **Rolling deploys**: Old instances serve traffic until the new ones are ready. The 5-second LB drain window on shutdown ensures zero requests are dropped.

### Cache warm-up time estimate

| Active servers | Startup time (DB on LAN) |
|---|---|
| 1,000 | < 0.1 s |
| 10,000 | < 0.5 s |
| 100,000 | < 5 s |
| 500,000 | < 25 s |

---

## Multi-Instance Example (docker-compose)

```yaml
services:
  gateway-1:
    image: registry/mcp-gateway:latest
    environment:
      DATABASE_URL: postgres://...
      INJECTOR_SOCKET_PATH: /run/sidecar.sock
      METRICS_TOKEN: ${METRICS_TOKEN}
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:3000/healthz/ready"]
      interval: 10s
      timeout: 5s
      retries: 3
      start_period: 15s

  gateway-2:
    image: registry/mcp-gateway:latest
    environment:
      DATABASE_URL: postgres://...
      INJECTOR_SOCKET_PATH: /run/sidecar.sock
      METRICS_TOKEN: ${METRICS_TOKEN}
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:3000/healthz/ready"]
      interval: 10s
      timeout: 5s
      retries: 3
      start_period: 15s
```

Both instances share the same PostgreSQL database. Config changes propagate to both within 2 seconds via LISTEN/NOTIFY.

---

## Sidecar Co-location

The gateway communicates with the credential injector sidecar over a Unix domain socket (`INJECTOR_SOCKET_PATH`). The sidecar must be co-located on the same host (pod or VM). In Kubernetes, deploy the sidecar as a container within the same pod:

```yaml
spec:
  containers:
    - name: gateway
      image: registry/mcp-gateway:latest
      volumeMounts:
        - name: sidecar-socket
          mountPath: /run/mcp-gateway
    - name: credential-injector
      image: registry/mcp-credential-injector:latest
      volumeMounts:
        - name: sidecar-socket
          mountPath: /run/mcp-gateway
  volumes:
    - name: sidecar-socket
      emptyDir: {}
```

If the sidecar is unreachable, `GET /healthz/ready` returns 503 and MCP `tools/call` requests on servers with auth requirements return JSON-RPC error `-32002`.
