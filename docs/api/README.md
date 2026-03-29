# Platform API — Error Response Contract

All 4xx and 5xx responses from the Platform API use a single, stable JSON shape. This document describes that shape and lists every HTTP status code the API may return.

---

## Response Shape

```json
{
  "error": {
    "code": "not_found",
    "message": "not found: mcp_server with id abc123",
    "request_id": "01HWRF3J5X0000000000000000"
  }
}
```

| Field | Type | Description |
|---|---|---|
| `error.code` | `string` | Stable snake_case identifier for the error class. Never changes across versions. |
| `error.message` | `string` | Human-readable description. Never exposes stack traces, SQL errors, or internal file paths. |
| `error.request_id` | `string` | Per-request [ULID](https://github.com/ulid/spec). Always matches the `X-Request-ID` response header. |

The formal JSON Schema is at [`error-schema.json`](./error-schema.json).

---

## X-Request-ID Header

Every response includes an `X-Request-ID` header. For error responses, the header value is identical to `error.request_id`. For success responses, it is set by the request-ID middleware.

Provide the `request_id` / `X-Request-ID` value when filing issues or contacting support.

---

## HTTP Status Codes

### 400 Bad Request — `bad_request`

The request body or query parameters are malformed or fail validation.

```http
HTTP/1.1 400 Bad Request
Content-Type: application/json
X-Request-ID: 01HWRF3J5X0000000000000001

{
  "error": {
    "code": "bad_request",
    "message": "bad request: 'name' is required",
    "request_id": "01HWRF3J5X0000000000000001"
  }
}
```

### 401 Unauthorized — `unauthorized`

The request lacks valid authentication credentials, or the provided token has expired or been revoked.

```http
HTTP/1.1 401 Unauthorized
Content-Type: application/json
X-Request-ID: 01HWRF3J5X0000000000000002

{
  "error": {
    "code": "unauthorized",
    "message": "unauthorized: bearer token is invalid",
    "request_id": "01HWRF3J5X0000000000000002"
  }
}
```

### 403 Forbidden — `forbidden`

The authenticated user does not have permission to perform the requested action.

```http
HTTP/1.1 403 Forbidden
Content-Type: application/json
X-Request-ID: 01HWRF3J5X0000000000000003

{
  "error": {
    "code": "forbidden",
    "message": "forbidden: you do not own this server",
    "request_id": "01HWRF3J5X0000000000000003"
  }
}
```

### 404 Not Found — `not_found`

The requested resource does not exist.

```http
HTTP/1.1 404 Not Found
Content-Type: application/json
X-Request-ID: 01HWRF3J5X0000000000000004

{
  "error": {
    "code": "not_found",
    "message": "not found: mcp_server with id abc123",
    "request_id": "01HWRF3J5X0000000000000004"
  }
}
```

### 409 Conflict — `conflict`

The request conflicts with the current state of a resource (e.g. a slug that is already taken).

```http
HTTP/1.1 409 Conflict
Content-Type: application/json
X-Request-ID: 01HWRF3J5X0000000000000005

{
  "error": {
    "code": "conflict",
    "message": "conflict: slug 'my-api' is already in use",
    "request_id": "01HWRF3J5X0000000000000005"
  }
}
```

### 500 Internal Server Error — `internal_server_error`

An unexpected server-side error occurred. The `message` field will never include internal details such as stack traces or SQL error text.

```http
HTTP/1.1 500 Internal Server Error
Content-Type: application/json
X-Request-ID: 01HWRF3J5X0000000000000006

{
  "error": {
    "code": "internal_server_error",
    "message": "internal server error: an unexpected error occurred",
    "request_id": "01HWRF3J5X0000000000000006"
  }
}
```

---

## Guaranteed Invariants

- The `error.request_id` field is always a 26-character uppercase ULID.
- The `X-Request-ID` header always equals `error.request_id` for error responses.
- `error.code` values are stable and will not change in minor releases.
- `error.message` values are for human consumption only and may change; do not parse them programmatically — use `error.code` for branching logic.
- The response body always has exactly one top-level key (`error`).
