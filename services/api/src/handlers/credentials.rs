//! Credential CRUD handlers.
//!
//! All handlers are protected by JWT auth middleware (applied at the router
//! level in `main.rs`). They verify server ownership before delegating to
//! [`crate::credentials::CredentialService`].
//!
//! # Ownership enforcement
//!
//! Every handler accepts a `{server_id}` path parameter. Before any operation,
//! it verifies `mcp_servers.user_id == authenticated_user.id`. If the server
//! does not exist, or belongs to another user, **404 Not Found** is returned
//! (not 403, to avoid disclosing that the resource exists).

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Extension, Json,
};
use mcp_common::AppError;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::{app_state::AppState, auth::AuthenticatedUser, credentials::CredentialMeta};

// ── Ownership helper ──────────────────────────────────────────────────────────

/// Verifies that `server_id` belongs to `user_id`. Returns `404 Not Found` if
/// the server does not exist or belongs to a different user.
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

// ── Request / Response types ──────────────────────────────────────────────────

/// Request body for `POST /v1/servers/{server_id}/credentials`.
#[derive(Deserialize)]
pub struct CreateCredentialRequest {
    /// Authentication type. One of `bearer`, `api_key_header`, `api_key_query`, `basic`.
    pub auth_type: String,
    /// Header or query-parameter name. Required for `api_key_header`/`api_key_query`.
    pub key_name: Option<String>,
    /// The raw credential value. Wrapped in `Zeroizing` before use; never stored in plain form.
    pub value: String,
}

/// Request body for `PUT /v1/servers/{server_id}/credentials/{id}`.
#[derive(Deserialize)]
pub struct RotateCredentialRequest {
    /// The new raw credential value.
    pub value: String,
}

/// Response body for `GET /v1/servers/{server_id}/credentials`.
#[derive(Serialize)]
pub struct ListCredentialsResponse {
    /// Credential metadata list — no sensitive fields.
    pub credentials: Vec<CredentialMeta>,
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// `POST /v1/servers/{server_id}/credentials`
///
/// Stores a new encrypted credential for the server. Returns 201 with
/// [`CredentialMeta`] (including the hint). The raw value is never echoed back.
pub async fn create_credential_handler(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(server_id): Path<Uuid>,
    Json(body): Json<CreateCredentialRequest>,
) -> Result<impl IntoResponse, AppError> {
    verify_server_ownership(&state.pool, server_id, user.id).await?;

    // Wrap value in Zeroizing so memory is wiped after encryption.
    let plaintext = Zeroizing::new(body.value);

    let meta = state
        .credential_service
        .store(
            server_id,
            &body.auth_type,
            body.key_name.as_deref(),
            plaintext,
            Some(user.id),
        )
        .await?;

    Ok((StatusCode::CREATED, Json(meta)))
}

/// `GET /v1/servers/{server_id}/credentials`
///
/// Lists credential metadata for the server. Never returns
/// `encrypted_payload`, `iv`, or `value` fields.
pub async fn list_credentials_handler(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(server_id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    verify_server_ownership(&state.pool, server_id, user.id).await?;

    let credentials = state
        .credential_service
        .list_for_server(server_id)
        .await?;

    Ok(Json(ListCredentialsResponse { credentials }))
}

/// `PUT /v1/servers/{server_id}/credentials/{id}`
///
/// Rotates a credential to a new value. Verifies that the credential belongs
/// to the specified server (which in turn belongs to the authenticated user).
pub async fn rotate_credential_handler(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path((server_id, credential_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<RotateCredentialRequest>,
) -> Result<impl IntoResponse, AppError> {
    verify_server_ownership(&state.pool, server_id, user.id).await?;

    let new_plaintext = Zeroizing::new(body.value);

    let meta = state
        .credential_service
        .rotate(credential_id, server_id, new_plaintext, Some(user.id))
        .await?;

    Ok(Json(meta))
}

/// `DELETE /v1/servers/{server_id}/credentials/{id}`
///
/// Deletes a credential. Returns 204 No Content on success.
pub async fn delete_credential_handler(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path((server_id, credential_id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse, AppError> {
    verify_server_ownership(&state.pool, server_id, user.id).await?;

    state
        .credential_service
        .delete(credential_id, server_id, Some(user.id))
        .await?;

    Ok(StatusCode::NO_CONTENT)
}
