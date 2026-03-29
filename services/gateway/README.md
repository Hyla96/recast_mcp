# mcp-gateway

The Gateway service is the entry point for all MCP traffic. It routes incoming JSON-RPC 2.0 requests (over Streamable HTTP or SSE) to the correct user-configured upstream API, injecting credentials via the Credential Injector sidecar.

## Configuration

All configuration is read from environment variables at startup. Missing or malformed required variables cause the process to exit immediately with a message listing every problem.

### Required

| Variable | Description |
|---|---|
| `DATABASE_URL` | PostgreSQL connection URL. Format: `postgres://<user>:<password>@<host>:<port>/<database>` |

### Optional

| Variable | Default | Description |
|---|---|---|
| `GATEWAY_PORT` | `3000` | TCP port the gateway binds to. |
| `INJECTOR_SOCKET_PATH` | `/tmp/recast-injector.sock` | Path to the credential injector Unix domain socket or HTTP address. |
| `UPSTREAM_MAX_RESPONSE_BYTES` | `102400` | Maximum upstream response body size in bytes (100 KB). |
| `UPSTREAM_TIMEOUT_SECS` | `30` | Upstream request timeout in seconds. |
| `RATE_LIMIT_CALLS_PER_MIN_PER_SERVER` | `100` | Maximum MCP tool calls per minute per server (token-bucket rate limit). |
| `RATE_LIMIT_CALLS_PER_MIN_PER_USER` | `1000` | Maximum MCP tool calls per minute per user (token-bucket rate limit). |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4317` | OTLP gRPC endpoint for the OpenTelemetry Collector. |
| `OTEL_SDK_DISABLED` | `false` | Set to `true` to disable all telemetry (traces and OTLP export). |
| `RUST_LOG` | `info` | Structured log level: `trace`, `debug`, `info`, `warn`, `error`. |

## Running locally

```bash
# Copy and fill in the environment template
cp .env.example .env
# edit .env as needed

# Run via Docker Compose (recommended)
make dev

# Or run directly (requires DATABASE_URL in environment)
DATABASE_URL=postgres://recast:recast@localhost:5432/recast_mcp just run-gateway
```

## Health endpoints

| Endpoint | Method | Description |
|---|---|---|
| `/health/live` | GET | Returns HTTP 200 immediately after the process starts. Body: `{"status":"ok","service":"mcp-gateway","version":"..."}` |
| `/health/ready` | GET | Returns HTTP 200 when PostgreSQL is reachable; HTTP 503 otherwise. Body includes a `checks.database` object with status and optional error message. Checks time out after 500 ms. |

Health endpoints are excluded from authentication middleware, rate limiting, and OpenTelemetry trace spans.

## Fail-fast startup

Starting the gateway with a missing required variable exits immediately:

```
$ mcp-gateway
gateway: configuration errors (1 total):
  - missing required environment variable: DATABASE_URL
```

Multiple missing or malformed variables are all listed in one message:

```
$ mcp-gateway
gateway: configuration errors (2 total):
  - missing required environment variable: DATABASE_URL
  - invalid value for environment variable GATEWAY_PORT: invalid digit found in string
```
