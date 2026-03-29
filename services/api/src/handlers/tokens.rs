//! Server token CRUD handlers.
//!
//! Server tokens are per-server Bearer tokens issued to MCP clients. The raw
//! token is returned **only once** (in the 201 Created response). Subsequent
//! list operations return a non-sensitive `hint` only — the token cannot be
//! recovered after creation.
//!
//! Token format: `mcp_live_<64 hex chars>` (73 characters total).
//! Storage: SHA-256 hex hash of the full token string.
//!
//! # Security
//!
//! - The raw token never appears in logs, OTEL spans, error messages, or
//!   any response after the initial 201.
//! - Token generation uses `rand::rngs::OsRng` (OS CSPRNG).
//! - SHA-256 is collision-resistant; the gateway validates incoming tokens by
//!   hashing the presented value and comparing with `token_hash`.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Extension, Json,
};
use chrono::{DateTime, Utc};
use mcp_common::{AppError, AuditAction, AuditEvent, AuditLogger};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{app_state::AppState, auth::AuthenticatedUser};

// ── Ownership helper ──────────────────────────────────────────────────────────

/// Verifies that `server_id` belongs to `user_id`. Returns 404 if the server
/// does not exist or belongs to a different user.
async fn verify_server_ownership(
    pool: &sqlx::PgPool,
    server_id: Uuid,
    user_id: Uuid,
) -> Result<(), AppError> {
    let exists = sqlx::query(
        "SELECT id FROM mcp_servers WHERE id = $1 AND user_id = $2",
    )
    .bind(server_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| AppError::InternalServerError(format!("ownership check failed: {e}")))?;

    if exists.is_none() {
        return Err(AppError::NotFound(
            "server not found or access denied".to_string(),
        ));
    }

    Ok(())
}

// ── Token generation ──────────────────────────────────────────────────────────

const TOKEN_PREFIX: &str = "mcp_live_";

/// Generates a new server token using the OS CSPRNG.
///
/// Returns `(raw_token, token_hash, hint)`:
/// - `raw_token`: full `mcp_live_<64hex>` string — returned in 201 only, never logged.
/// - `token_hash`: lowercase hex SHA-256 of `raw_token` — stored in DB.
/// - `hint`: first 12 characters of `raw_token` + "****" — stored in DB, shown in list.
fn generate_token() -> (String, String, String) {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);

    let hex_body: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let raw_token = format!("{TOKEN_PREFIX}{hex_body}");

    // SHA-256 of the full token string.
    let mut hasher = Sha256::new();
    hasher.update(raw_token.as_bytes());
    let hash_bytes = hasher.finalize();
    let token_hash: String = hash_bytes.iter().map(|b| format!("{b:02x}")).collect();

    // Hint: first 12 chars (covers "mcp_live_XXX") + "****".
    let hint_prefix: String = raw_token.chars().take(12).collect();
    let hint = format!("{hint_prefix}****");

    (raw_token, token_hash, hint)
}

// ── Request / Response types ──────────────────────────────────────────────────

/// Request body for `POST /v1/servers/{server_id}/tokens`.
#[derive(Deserialize)]
pub struct CreateTokenRequest {
    /// Optional human-readable description for this token (e.g. `"Production client"`).
    pub description: Option<String>,
}

/// Full token response returned **only** in the 201 Created body.
///
/// After creation the raw `token` field is no longer retrievable.
#[derive(Serialize)]
pub struct TokenCreatedResponse {
    /// Unique token identifier.
    pub id: Uuid,
    /// The server this token grants access to.
    pub server_id: Uuid,
    /// Optional human-readable description.
    pub description: Option<String>,
    /// The raw token — returned once, store it securely.
    pub token: String,
    /// Non-sensitive preview (e.g. `"mcp_live_XXX****"`).
    pub hint: String,
    /// Whether the token is currently active.
    pub is_active: bool,
    /// When the token was created.
    pub created_at: DateTime<Utc>,
}

/// Non-sensitive token metadata for list responses (no `token` field).
#[derive(Serialize)]
pub struct TokenMeta {
    /// Unique token identifier.
    pub id: Uuid,
    /// The server this token grants access to.
    pub server_id: Uuid,
    /// Optional human-readable description.
    pub description: Option<String>,
    /// Non-sensitive preview; never the raw token.
    pub hint: Option<String>,
    /// Whether the token is currently active.
    pub is_active: bool,
    /// When the token was created.
    pub created_at: DateTime<Utc>,
    /// When the token was revoked, if applicable.
    pub revoked_at: Option<DateTime<Utc>>,
}

/// Response body for `GET /v1/servers/{server_id}/tokens`.
#[derive(Serialize)]
pub struct ListTokensResponse {
    /// Token metadata list — no raw token values.
    pub tokens: Vec<TokenMeta>,
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// `POST /v1/servers/{server_id}/tokens`
///
/// Generates a new server token. The raw token value is returned in this
/// 201 response only — store it immediately. Subsequent list calls return
/// the hint only.
pub async fn create_token_handler(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(server_id): Path<Uuid>,
    Json(body): Json<CreateTokenRequest>,
) -> Result<impl IntoResponse, AppError> {
    verify_server_ownership(&state.pool, server_id, user.id).await?;

    let (raw_token, token_hash, hint) = generate_token();

    let row = sqlx::query(
        "INSERT INTO server_tokens (server_id, token_hash, description, hint)
         VALUES ($1, $2, $3, $4)
         RETURNING id, server_id, description, hint, is_active, created_at",
    )
    .bind(server_id)
    .bind(&token_hash)
    .bind(body.description.as_deref())
    .bind(&hint)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| AppError::InternalServerError(format!("token insert failed: {e}")))?;

    use sqlx::Row;
    let id: Uuid = row
        .try_get("id")
        .map_err(|e| AppError::InternalServerError(format!("row decode id: {e}")))?;
    let description: Option<String> = row
        .try_get("description")
        .map_err(|e| AppError::InternalServerError(format!("row decode description: {e}")))?;
    let is_active: bool = row
        .try_get("is_active")
        .map_err(|e| AppError::InternalServerError(format!("row decode is_active: {e}")))?;
    let created_at: DateTime<Utc> = row
        .try_get("created_at")
        .map_err(|e| AppError::InternalServerError(format!("row decode created_at: {e}")))?;

    emit_audit(
        &state.audit_logger,
        AuditAction::ServerTokenGenerate,
        user.id,
        server_id,
        Some(id),
    );

    Ok((
        StatusCode::CREATED,
        Json(TokenCreatedResponse {
            id,
            server_id,
            description,
            token: raw_token,
            hint,
            is_active,
            created_at,
        }),
    ))
}

/// `GET /v1/servers/{server_id}/tokens`
///
/// Lists token metadata for the server. The `token` field is never included —
/// only the non-sensitive `hint` is returned.
pub async fn list_tokens_handler(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(server_id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    verify_server_ownership(&state.pool, server_id, user.id).await?;

    let rows = sqlx::query(
        "SELECT id, server_id, description, hint, is_active, created_at, revoked_at
         FROM server_tokens
         WHERE server_id = $1
         ORDER BY created_at DESC",
    )
    .bind(server_id)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| AppError::InternalServerError(format!("list tokens failed: {e}")))?;

    use sqlx::Row;
    let tokens: Result<Vec<TokenMeta>, AppError> = rows
        .iter()
        .map(|row| {
            Ok(TokenMeta {
                id: row
                    .try_get("id")
                    .map_err(|e| AppError::InternalServerError(format!("row decode id: {e}")))?,
                server_id: row.try_get("server_id").map_err(|e| {
                    AppError::InternalServerError(format!("row decode server_id: {e}"))
                })?,
                description: row.try_get("description").map_err(|e| {
                    AppError::InternalServerError(format!("row decode description: {e}"))
                })?,
                hint: row
                    .try_get("hint")
                    .map_err(|e| AppError::InternalServerError(format!("row decode hint: {e}")))?,
                is_active: row.try_get("is_active").map_err(|e| {
                    AppError::InternalServerError(format!("row decode is_active: {e}"))
                })?,
                created_at: row.try_get("created_at").map_err(|e| {
                    AppError::InternalServerError(format!("row decode created_at: {e}"))
                })?,
                revoked_at: row.try_get("revoked_at").map_err(|e| {
                    AppError::InternalServerError(format!("row decode revoked_at: {e}"))
                })?,
            })
        })
        .collect();

    Ok(Json(ListTokensResponse { tokens: tokens? }))
}

/// `DELETE /v1/servers/{server_id}/tokens/{id}`
///
/// Revokes a token by setting `revoked_at = NOW()` and `is_active = FALSE`.
/// Returns 204 No Content. Returns 404 if the token does not belong to the server.
pub async fn revoke_token_handler(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path((server_id, token_id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse, AppError> {
    verify_server_ownership(&state.pool, server_id, user.id).await?;

    // Verify the token belongs to this server.
    let existing = sqlx::query(
        "SELECT server_id FROM server_tokens WHERE id = $1",
    )
    .bind(token_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| AppError::InternalServerError(format!("token lookup failed: {e}")))?
    .ok_or_else(|| AppError::NotFound("token not found".to_string()))?;

    use sqlx::Row;
    let existing_server_id: Uuid = existing
        .try_get("server_id")
        .map_err(|e| AppError::InternalServerError(format!("row decode server_id: {e}")))?;

    if existing_server_id != server_id {
        return Err(AppError::NotFound("token not found".to_string()));
    }

    sqlx::query(
        "UPDATE server_tokens
         SET is_active = FALSE, revoked_at = NOW()
         WHERE id = $1",
    )
    .bind(token_id)
    .execute(&state.pool)
    .await
    .map_err(|e| AppError::InternalServerError(format!("token revoke failed: {e}")))?;

    emit_audit(
        &state.audit_logger,
        AuditAction::ServerTokenRevoke,
        user.id,
        server_id,
        Some(token_id),
    );

    Ok(StatusCode::NO_CONTENT)
}

// ── Audit helper ──────────────────────────────────────────────────────────────

fn emit_audit(
    logger: &AuditLogger,
    action: AuditAction,
    user_id: Uuid,
    server_id: Uuid,
    token_id: Option<Uuid>,
) {
    logger.log(AuditEvent {
        action,
        user_id: Some(user_id),
        server_id: Some(server_id),
        success: true,
        error_msg: None,
        metadata: token_id.map(|id| serde_json::json!({ "token_id": id.to_string() })),
        correlation_id: None,
    });
}
