//! Request/response types for MCP Server CRUD endpoints.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── ServerConfig ──────────────────────────────────────────────────────────────

/// Server configuration for **reading from the database**.
///
/// Uses `#[serde(default)]` on every field so that JSONB rows written by older
/// schema versions (missing new fields) deserialize without error.
#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct ServerConfig {
    /// Base URL of the upstream REST API, e.g. `https://api.stripe.com`.
    #[serde(default)]
    pub upstream_base_url: Option<String>,
}

/// Server configuration accepted in **user-supplied request bodies**.
///
/// Uses `#[serde(deny_unknown_fields)]` so that typos or unsupported fields
/// are surfaced as a 422 / deserialization error rather than silently ignored.
#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ServerConfigInput {
    /// Base URL of the upstream REST API, e.g. `https://api.stripe.com`.
    pub upstream_base_url: Option<String>,
}

impl From<ServerConfigInput> for ServerConfig {
    fn from(input: ServerConfigInput) -> Self {
        Self {
            upstream_base_url: input.upstream_base_url,
        }
    }
}

// ── Response type ─────────────────────────────────────────────────────────────

/// A single MCP server as returned by the REST API.
#[derive(Debug, Serialize)]
pub struct ServerResponse {
    /// Unique server identifier.
    pub id: Uuid,
    /// URL-safe slug used as the MCP server path segment.
    pub slug: String,
    /// Human-readable server name.
    pub display_name: String,
    /// Optional description.
    pub description: Option<String>,
    /// Decoded server configuration.
    pub config: ServerConfig,
    /// Whether the server is currently active (accepting MCP requests).
    pub is_active: bool,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last-updated timestamp.
    pub updated_at: DateTime<Utc>,
    /// Full MCP URL for MCP clients to connect to this server.
    pub mcp_url: String,
}

// ── Request types ─────────────────────────────────────────────────────────────

/// Request body for `POST /v1/servers`.
#[derive(Deserialize)]
pub struct CreateServerRequest {
    /// Human-readable server name. Max 100 characters.
    pub display_name: String,
    /// Optional description.
    pub description: Option<String>,
    /// Server configuration. Defaults to empty config if omitted.
    #[serde(default)]
    pub config: ServerConfigInput,
}

/// Request body for `PUT /v1/servers/{id}`.
#[derive(Deserialize)]
pub struct UpdateServerRequest {
    /// Updated server name. Max 100 characters.
    pub display_name: Option<String>,
    /// Updated description.
    pub description: Option<String>,
    /// Updated server configuration.
    pub config: Option<ServerConfigInput>,
    /// Whether the server should be active.
    pub is_active: Option<bool>,
}

/// Query parameters for `GET /v1/servers`.
#[derive(Deserialize)]
pub struct ListServersQuery {
    /// Cursor: `id` of the last item from the previous page (forward pagination).
    pub after: Option<Uuid>,
    /// Maximum items per page. Default 20, maximum 100.
    pub limit: Option<u32>,
}

/// Response body for `GET /v1/servers`.
#[derive(Serialize)]
pub struct ListServersResponse {
    /// Page of server records.
    pub servers: Vec<ServerResponse>,
    /// Pagination metadata.
    pub pagination: PaginationMeta,
}

/// Pagination metadata accompanying list responses.
#[derive(Serialize)]
pub struct PaginationMeta {
    /// Total number of servers owned by the authenticated user (ignoring cursor).
    pub total: i64,
    /// Whether there are more pages after the current one.
    pub has_next: bool,
}

/// Request body for `POST /v1/servers/{id}/validate-url`.
#[derive(Deserialize)]
pub struct ValidateUrlRequest {
    /// The URL to validate against the SSRF blocklist.
    pub url: String,
}

/// Response body for `POST /v1/servers/{id}/validate-url`.
#[derive(Serialize)]
pub struct ValidateUrlResponse {
    /// Whether the URL passed SSRF Phase 1 validation.
    pub valid: bool,
    /// Present only when `valid = false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ValidateUrlError>,
}

/// Error detail included in [`ValidateUrlResponse`] when `valid = false`.
#[derive(Serialize)]
pub struct ValidateUrlError {
    /// Machine-readable error code (e.g. `"ssrf_blocked"`, `"invalid_url"`).
    pub code: String,
    /// Human-readable explanation.
    pub message: String,
}
