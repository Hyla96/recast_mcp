//! User management handlers.
//!
//! Handlers in this module operate on the caller's own user record.
//! Authentication is enforced upstream by [`crate::auth::clerk_jwt_middleware`];
//! handlers can assume that [`crate::auth::AuthenticatedUser`] is present in
//! request extensions.

use axum::{extract::State, response::IntoResponse, Extension, Json};
use chrono::{DateTime, Utc};
use mcp_common::AppError;
use serde::Serialize;
use sqlx::Row;
use uuid::Uuid;

use crate::{app_state::AppState, auth::AuthenticatedUser};

// ── Response types ────────────────────────────────────────────────────────────

/// Response body for `GET /v1/users/me`.
#[derive(Serialize)]
pub struct UserResponse {
    /// Internal platform user UUID.
    pub id: Uuid,
    /// User email address.
    pub email: String,
    /// Timestamp when the user first authenticated on this platform.
    pub created_at: DateTime<Utc>,
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// `GET /v1/users/me` — returns the authenticated user's profile.
///
/// The authenticated user identity is injected by `clerk_jwt_middleware`.
/// This handler fetches `created_at` from the database to complete the
/// response (the middleware only provides `id` and `email`).
pub async fn me_handler(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let row = sqlx::query("SELECT created_at FROM users WHERE id = $1")
        .bind(user.id)
        .fetch_one(&state.pool)
        .await
        .map_err(|e| {
            tracing::error!("me_handler: db error: {e}");
            AppError::InternalServerError("Failed to fetch user record.".to_string())
        })?;

    let created_at: DateTime<Utc> = row.get("created_at");

    Ok(Json(UserResponse {
        id: user.id,
        email: user.email,
        created_at,
    }))
}
