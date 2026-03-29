# mcp-api (Platform API)

The Platform API is the control-plane service for Recast MCP. It handles user authentication (via Clerk), and provides CRUD endpoints for MCP servers, credentials, and server tokens.

## Configuration

All configuration is read from environment variables at startup. Missing or malformed required variables cause the process to exit immediately with a message listing every problem.

### Required

| Variable | Description |
|---|---|
| `DATABASE_URL` | PostgreSQL connection URL. Format: `postgres://<user>:<password>@<host>:<port>/<database>` |
| `CLERK_SECRET_KEY` | Clerk secret key for server-side JWT verification. Obtain from [Clerk Dashboard → API Keys](https://dashboard.clerk.com). |

### Optional

| Variable | Default | Description |
|---|---|---|
| `API_PORT` | `3001` | TCP port the API binds to. |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4317` | OTLP gRPC endpoint for the OpenTelemetry Collector. |
| `OTEL_SDK_DISABLED` | `false` | Set to `true` to disable all telemetry (traces and OTLP export). |
| `RUST_LOG` | `info` | Structured log level: `trace`, `debug`, `info`, `warn`, `error`. |

## Running locally

```bash
# Copy and fill in the environment template
cp .env.example .env
# edit .env: set DATABASE_URL and CLERK_SECRET_KEY

# Run via Docker Compose (recommended)
make dev

# Or run directly
DATABASE_URL=postgres://recast:recast@localhost:5432/recast_mcp \
  CLERK_SECRET_KEY=sk_test_... \
  just run-api
```

## Fail-fast startup

Starting the API with missing required variables exits immediately:

```
$ mcp-api
platform-api: configuration errors (2 total):
  - missing required environment variable: DATABASE_URL
  - missing required environment variable: CLERK_SECRET_KEY
```
