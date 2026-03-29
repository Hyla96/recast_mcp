//! MCP Server management service.
//!
//! Encapsulates all SQL queries for MCP server CRUD operations behind a clean
//! service interface. Handlers remain thin: they validate input, call the
//! service, and map to HTTP responses. All database interaction lives here.
//!
//! # Design
//!
//! - [`ServerService`] is cheaply cloneable — it holds a `PgPool` and
//!   `AuditLogger`, both of which are internally `Arc`-wrapped.
//! - All methods return [`AppError`] so handlers can propagate with `?`.
//! - Ownership is enforced via `WHERE id = $1 AND user_id = $2` — both
//!   non-existent and foreign-owned servers return `AppError::NotFound` to
//!   prevent resource enumeration.

use chrono::{DateTime, Utc};
use mcp_common::{AppError, AuditAction, AuditEvent, AuditLogger};
use rand::{distributions::Alphanumeric, rngs::OsRng, Rng};
use serde_json::Value as JsonValue;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::handlers::servers::{
    ListServersQuery, ServerConfig, ServerResponse,
};

// ── ServerService ─────────────────────────────────────────────────────────────

/// Service for creating, listing, fetching, updating, and deleting MCP servers.
///
/// `ServerService` is cheaply cloneable — all clones share the same pool
/// and audit logger (each is internally `Arc`-wrapped).
#[derive(Clone)]
pub struct ServerService {
    pool: PgPool,
    audit_logger: AuditLogger,
    /// Gateway base URL used to compute `mcp_url` in responses.
    gateway_base_url: String,
}

impl ServerService {
    /// Creates a new `ServerService`.
    ///
    /// # Arguments
    ///
    /// * `pool` — Shared PostgreSQL connection pool.
    /// * `audit_logger` — For emitting `ServerCreate`, `ServerUpdate`, and
    ///   `ServerDelete` audit events.
    /// * `gateway_base_url` — Base URL of the gateway, e.g.
    ///   `https://mcp.example.com`. Used to build `mcp_url` in responses.
    pub fn new(pool: PgPool, audit_logger: AuditLogger, gateway_base_url: String) -> Self {
        Self {
            pool,
            audit_logger,
            gateway_base_url,
        }
    }

    /// Inserts a new MCP server record.
    ///
    /// Generates a URL-safe slug from `display_name` with a 4-char random
    /// suffix, retrying up to 5 times on unique-constraint collisions.
    ///
    /// # Errors
    ///
    /// - [`AppError::InternalServerError`] — serialization, DB insert failure,
    ///   or slug collision exhaustion after 5 retries.
    pub async fn create_server(
        &self,
        user_id: Uuid,
        display_name: &str,
        description: Option<&str>,
        config_value: JsonValue,
    ) -> Result<ServerResponse, AppError> {
        let mut last_err: Option<AppError> = None;

        for _ in 0..5u8 {
            let slug = generate_slug(display_name);

            let result = sqlx::query(
                "INSERT INTO mcp_servers (user_id, name, slug, description, config_json, status)
                 VALUES ($1, $2, $3, $4, $5, 'active')
                 RETURNING id, name, slug, description, config_json, status, created_at, updated_at",
            )
            .bind(user_id)
            .bind(display_name)
            .bind(&slug)
            .bind(description)
            .bind(&config_value)
            .fetch_one(&self.pool)
            .await;

            match result {
                Ok(row) => {
                    let response = row_to_server_response(&row, &self.gateway_base_url)?;
                    let server_id = response.id;
                    self.emit_audit(AuditAction::ServerCreate, user_id, server_id);
                    return Ok(response);
                }
                Err(sqlx::Error::Database(db_err))
                    if db_err
                        .constraint()
                        .is_some_and(|c| c.contains("mcp_servers_user_id_slug_key")) =>
                {
                    // Slug collision — retry with a new suffix.
                    last_err = Some(AppError::InternalServerError(
                        "slug collision after max retries".to_string(),
                    ));
                    continue;
                }
                Err(e) => {
                    return Err(AppError::InternalServerError(format!(
                        "server insert failed: {e}"
                    )));
                }
            }
        }

        Err(last_err.unwrap_or_else(|| {
            AppError::InternalServerError("slug generation exhausted all retries".to_string())
        }))
    }

    /// Lists servers for a user with cursor-based pagination.
    ///
    /// Results are ordered by `created_at DESC, id DESC`. Fetches
    /// `limit + 1` rows to determine `has_next` without a second query.
    ///
    /// # Errors
    ///
    /// - [`AppError::NotFound`] — `after` cursor ID not found or not owned by user.
    /// - [`AppError::InternalServerError`] — DB failure.
    pub async fn list_servers(
        &self,
        user_id: Uuid,
        params: &ListServersQuery,
    ) -> Result<(Vec<ServerResponse>, i64, bool), AppError> {
        let page_size = params.limit.unwrap_or(20).min(100) as i64;

        let total: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM mcp_servers WHERE user_id = $1")
                .bind(user_id)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| {
                    AppError::InternalServerError(format!("count servers failed: {e}"))
                })?;

        let fetch_limit = page_size + 1;

        let rows = if let Some(after_id) = params.after {
            let cursor = sqlx::query(
                "SELECT created_at FROM mcp_servers WHERE id = $1 AND user_id = $2",
            )
            .bind(after_id)
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| {
                AppError::InternalServerError(format!("cursor lookup failed: {e}"))
            })?
            .ok_or_else(|| AppError::NotFound("pagination cursor not found".to_string()))?;

            let cursor_created_at: DateTime<Utc> = cursor
                .try_get("created_at")
                .map_err(|e| AppError::InternalServerError(format!("cursor decode: {e}")))?;

            sqlx::query(
                "SELECT id, name, slug, description, config_json, status, created_at, updated_at
                 FROM mcp_servers
                 WHERE user_id = $1
                   AND (created_at < $2 OR (created_at = $2 AND id < $3))
                 ORDER BY created_at DESC, id DESC
                 LIMIT $4",
            )
            .bind(user_id)
            .bind(cursor_created_at)
            .bind(after_id)
            .bind(fetch_limit)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::InternalServerError(format!("list servers failed: {e}")))?
        } else {
            sqlx::query(
                "SELECT id, name, slug, description, config_json, status, created_at, updated_at
                 FROM mcp_servers
                 WHERE user_id = $1
                 ORDER BY created_at DESC, id DESC
                 LIMIT $2",
            )
            .bind(user_id)
            .bind(fetch_limit)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::InternalServerError(format!("list servers failed: {e}")))?
        };

        let has_next = rows.len() as i64 > page_size;
        let rows = &rows[..rows.len().min(page_size as usize)];

        let servers: Result<Vec<ServerResponse>, AppError> = rows
            .iter()
            .map(|row| row_to_server_response(row, &self.gateway_base_url))
            .collect();

        Ok((servers?, total, has_next))
    }

    /// Fetches a single server by ID, enforcing user ownership.
    ///
    /// # Errors
    ///
    /// - [`AppError::NotFound`] — server does not exist or belongs to another user.
    /// - [`AppError::InternalServerError`] — DB failure.
    pub async fn get_server(
        &self,
        user_id: Uuid,
        server_id: Uuid,
    ) -> Result<ServerResponse, AppError> {
        let row = sqlx::query(
            "SELECT id, name, slug, description, config_json, status, created_at, updated_at
             FROM mcp_servers
             WHERE id = $1 AND user_id = $2",
        )
        .bind(server_id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::InternalServerError(format!("get server failed: {e}")))?
        .ok_or_else(|| AppError::NotFound("server not found".to_string()))?;

        row_to_server_response(&row, &self.gateway_base_url)
    }

    /// Updates a server's mutable fields, merging with current DB values.
    ///
    /// Only fields that are `Some` in the arguments are applied; `None` means
    /// "keep current DB value". The `config_value` argument is the fully
    /// serialized config JSON if an update was requested, or `None` to retain
    /// the existing value.
    ///
    /// # Errors
    ///
    /// - [`AppError::NotFound`] — server does not exist or belongs to another user.
    /// - [`AppError::InternalServerError`] — DB failure.
    pub async fn update_server(
        &self,
        user_id: Uuid,
        server_id: Uuid,
        display_name: Option<String>,
        description: Option<Option<String>>,
        config_value: Option<JsonValue>,
        is_active: Option<bool>,
    ) -> Result<ServerResponse, AppError> {
        // Ownership check — load the current row.
        let current = sqlx::query(
            "SELECT id, name, slug, description, config_json, status, created_at, updated_at
             FROM mcp_servers
             WHERE id = $1 AND user_id = $2",
        )
        .bind(server_id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::InternalServerError(format!("ownership check failed: {e}")))?
        .ok_or_else(|| AppError::NotFound("server not found".to_string()))?;

        // Merge: new value if provided, else current DB value.
        let new_name: String = display_name.unwrap_or_else(|| {
            current
                .try_get("name")
                .unwrap_or_else(|_| String::new())
        });
        let new_description: Option<String> = match description {
            Some(v) => v,
            None => current.try_get("description").unwrap_or(None),
        };
        let new_config_json: JsonValue = match config_value {
            Some(v) => v,
            None => current
                .try_get("config_json")
                .map_err(|e| AppError::InternalServerError(format!("row decode config_json: {e}")))?,
        };
        let new_status: String = match is_active {
            Some(true) => "active".to_string(),
            Some(false) => "inactive".to_string(),
            None => current
                .try_get("status")
                .unwrap_or_else(|_| "active".to_string()),
        };

        let updated = sqlx::query(
            "UPDATE mcp_servers
             SET name = $1, description = $2, config_json = $3, status = $4
             WHERE id = $5 AND user_id = $6
             RETURNING id, name, slug, description, config_json, status, created_at, updated_at",
        )
        .bind(&new_name)
        .bind(&new_description)
        .bind(&new_config_json)
        .bind(&new_status)
        .bind(server_id)
        .bind(user_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| AppError::InternalServerError(format!("server update failed: {e}")))?;

        self.emit_audit(AuditAction::ServerUpdate, user_id, server_id);

        row_to_server_response(&updated, &self.gateway_base_url)
    }

    /// Deletes a server (and cascades to credentials/tokens via DB constraints).
    ///
    /// Returns `Ok(())` if the server was deleted.
    ///
    /// # Errors
    ///
    /// - [`AppError::NotFound`] — server does not exist or belongs to another user.
    /// - [`AppError::InternalServerError`] — DB failure.
    pub async fn delete_server(&self, user_id: Uuid, server_id: Uuid) -> Result<(), AppError> {
        let result = sqlx::query("DELETE FROM mcp_servers WHERE id = $1 AND user_id = $2")
            .bind(server_id)
            .bind(user_id)
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::InternalServerError(format!("server delete failed: {e}")))?;

        if result.rows_affected() == 0 {
            return Err(AppError::NotFound("server not found".to_string()));
        }

        self.emit_audit(AuditAction::ServerDelete, user_id, server_id);

        Ok(())
    }

    /// Checks that a server exists and is owned by `user_id`.
    ///
    /// Returns `Ok(())` if the server exists and is owned by the caller.
    ///
    /// # Errors
    ///
    /// - [`AppError::NotFound`] — server does not exist or belongs to another user.
    /// - [`AppError::InternalServerError`] — DB failure.
    pub async fn check_ownership(
        &self,
        user_id: Uuid,
        server_id: Uuid,
    ) -> Result<(), AppError> {
        let exists = sqlx::query("SELECT id FROM mcp_servers WHERE id = $1 AND user_id = $2")
            .bind(server_id)
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| AppError::InternalServerError(format!("ownership check failed: {e}")))?;

        if exists.is_none() {
            return Err(AppError::NotFound("server not found".to_string()));
        }

        Ok(())
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    fn emit_audit(&self, action: AuditAction, user_id: Uuid, server_id: Uuid) {
        self.audit_logger.log(AuditEvent {
            action,
            user_id: Some(user_id),
            server_id: Some(server_id),
            success: true,
            error_msg: None,
            metadata: None,
            correlation_id: None,
        });
    }
}

// ── Private helpers ───────────────────────────────────────────────────────────

/// Generates a URL-safe slug from a display name with a 4-char random suffix.
///
/// Algorithm:
/// 1. Lowercase the input.
/// 2. Replace spaces and underscores with hyphens.
/// 3. Strip any character that is not alphanumeric or a hyphen.
/// 4. Truncate to 45 characters (leaving room for "-XXXX").
/// 5. Trim leading/trailing hyphens.
/// 6. Append "-<4 random lowercase alphanumeric chars>".
/// 7. Final slug is at most 50 characters.
fn generate_slug(display_name: &str) -> String {
    let base: String = display_name
        .to_lowercase()
        .chars()
        .map(|c| if c == ' ' || c == '_' { '-' } else { c })
        .filter(|c| c.is_alphanumeric() || *c == '-')
        .take(45)
        .collect();

    let base = base.trim_matches('-');

    let suffix: String = OsRng
        .sample_iter(&Alphanumeric)
        .take(4)
        .map(|b| char::from(b).to_ascii_lowercase())
        .collect();

    if base.is_empty() {
        suffix
    } else {
        format!("{base}-{suffix}")
    }
}

/// Decodes a `PgRow` into a [`ServerResponse`], computing `mcp_url` from the gateway URL.
fn row_to_server_response(
    row: &sqlx::postgres::PgRow,
    gateway_base_url: &str,
) -> Result<ServerResponse, AppError> {
    let id: Uuid = row
        .try_get("id")
        .map_err(|e| AppError::InternalServerError(format!("row decode id: {e}")))?;
    let name: String = row
        .try_get("name")
        .map_err(|e| AppError::InternalServerError(format!("row decode name: {e}")))?;
    let slug: String = row
        .try_get("slug")
        .map_err(|e| AppError::InternalServerError(format!("row decode slug: {e}")))?;
    let description: Option<String> = row
        .try_get("description")
        .map_err(|e| AppError::InternalServerError(format!("row decode description: {e}")))?;
    let config_json: JsonValue = row
        .try_get("config_json")
        .map_err(|e| AppError::InternalServerError(format!("row decode config_json: {e}")))?;
    let status: String = row
        .try_get("status")
        .map_err(|e| AppError::InternalServerError(format!("row decode status: {e}")))?;
    let created_at: DateTime<Utc> = row
        .try_get("created_at")
        .map_err(|e| AppError::InternalServerError(format!("row decode created_at: {e}")))?;
    let updated_at: DateTime<Utc> = row
        .try_get("updated_at")
        .map_err(|e| AppError::InternalServerError(format!("row decode updated_at: {e}")))?;

    let config: ServerConfig = serde_json::from_value(config_json).map_err(|e| {
        AppError::InternalServerError(format!("config_json deserialize failed: {e}"))
    })?;

    let is_active = status == "active";
    let mcp_url = format!("{gateway_base_url}/mcp/{slug}");

    Ok(ServerResponse {
        id,
        slug,
        display_name: name,
        description,
        config,
        is_active,
        created_at,
        updated_at,
        mcp_url,
    })
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn generate_slug_basic() {
        let slug = generate_slug("My API Server");
        assert!(slug.starts_with("my-api-server-"), "slug: {slug}");
        assert!(slug.len() <= 50, "slug too long: {slug}");
    }

    #[test]
    fn generate_slug_strips_special_chars() {
        let slug = generate_slug("Hello! @World#");
        assert!(slug.starts_with("hello-world-"), "slug: {slug}");
    }

    #[test]
    fn generate_slug_empty_name_produces_suffix_only() {
        let slug = generate_slug("!!!---");
        // base becomes empty after stripping → only the 4-char suffix
        assert_eq!(slug.len(), 4, "empty base should give 4-char suffix: {slug}");
    }

    #[test]
    fn generate_slug_long_name_truncated() {
        let long_name = "a".repeat(200);
        let slug = generate_slug(&long_name);
        assert!(slug.len() <= 50, "slug must be ≤ 50 chars: {slug}");
    }
}
