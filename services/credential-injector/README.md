# mcp-credential-injector

The Credential Injector is a sidecar service responsible for decrypting stored credentials and injecting them into upstream API requests on behalf of the Gateway. The Gateway never holds raw credential values.

## Configuration

All configuration is read from environment variables at startup. Missing or malformed required variables cause the process to exit immediately with a message listing every problem.

### Required

| Variable | Description |
|---|---|
| `DATABASE_URL` | PostgreSQL connection URL. Format: `postgres://<user>:<password>@<host>:<port>/<database>` |
| `ENCRYPTION_KEY` | 32-byte AES-256-GCM encryption key, hex-encoded (64 hex characters). Generate with: `openssl rand -hex 32` |

### Optional

| Variable | Default | Description |
|---|---|---|
| `INJECTOR_PORT` | `3002` | TCP port the injector binds to. |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4317` | OTLP gRPC endpoint for the OpenTelemetry Collector. |
| `OTEL_SDK_DISABLED` | `false` | Set to `true` to disable all telemetry (traces and OTLP export). |
| `RUST_LOG` | `info` | Structured log level: `trace`, `debug`, `info`, `warn`, `error`. |

## Running locally

```bash
# Copy and fill in the environment template
cp .env.example .env
# edit .env: set DATABASE_URL and ENCRYPTION_KEY

# Generate a new encryption key:
openssl rand -hex 32

# Run via Docker Compose (recommended)
make dev

# Or run directly
DATABASE_URL=postgres://recast:recast@localhost:5432/recast_mcp \
  ENCRYPTION_KEY=$(openssl rand -hex 32) \
  just run-injector
```

## Security

- `ENCRYPTION_KEY` is a 32-byte (256-bit) key for AES-256-GCM authenticated encryption.
- This value must never be logged, committed to version control, or included in error messages.
- The key value is not echoed in startup logs; only service/port/database_url fields are logged.

## Fail-fast startup

Starting the injector with missing or malformed required variables exits immediately:

```
$ mcp-credential-injector
credential-injector: configuration errors (2 total):
  - missing required environment variable: DATABASE_URL
  - missing required environment variable: ENCRYPTION_KEY
```

If `ENCRYPTION_KEY` is present but malformed:

```
credential-injector: configuration errors (1 total):
  - invalid value for environment variable ENCRYPTION_KEY: value is not valid hexadecimal
```
