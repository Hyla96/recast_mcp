//! MCP Server CRUD handlers.
//!
//! Endpoints:
//!   POST   /v1/servers                    — create server
//!   GET    /v1/servers                    — list servers (cursor-paginated)
//!   GET    /v1/servers/{id}               — get server
//!   PUT    /v1/servers/{id}               — full update
//!   DELETE /v1/servers/{id}               — delete server
//!   POST   /v1/servers/{id}/validate-url  — SSRF Phase 1 check on a URL
//!
//! All endpoints require JWT auth (applied at the router level in `main.rs`).
//! Ownership is enforced inside [`ServerService`] via
//! `WHERE id = $1 AND user_id = $2` — returns 404 for both non-existent and
//! foreign-owned servers to avoid resource enumeration.

mod types;
pub use types::*;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Extension, Json,
};
use mcp_common::{validate_url as ssrf_validate, AppError};
use url::Url;
use uuid::Uuid;

use crate::{app_state::AppState, auth::AuthenticatedUser};

// ── Validation helpers ────────────────────────────────────────────────────────
// These produce AppError responses and remain in the HTTP layer.

/// Validates `display_name` length constraint (max 100 chars).
fn validate_display_name(display_name: &str) -> Result<(), AppError> {
    if display_name.len() > 100 {
        return Err(AppError::Validation {
            field: "display_name".to_string(),
            message: "must be 100 characters or fewer".to_string(),
        });
    }
    if display_name.trim().is_empty() {
        return Err(AppError::Validation {
            field: "display_name".to_string(),
            message: "must not be blank".to_string(),
        });
    }
    Ok(())
}

/// Validates the `upstream_base_url` inside a [`ServerConfigInput`] against SSRF Phase 1.
///
/// Returns `Ok(())` if the URL is absent, valid, or passes the blocklist check.
fn validate_config_ssrf(config: &ServerConfigInput) -> Result<(), AppError> {
    let Some(ref raw_url) = config.upstream_base_url else {
        return Ok(());
    };
    let parsed = raw_url.parse::<Url>().map_err(|_| AppError::Validation {
        field: "config.upstream_base_url".to_string(),
        message: "invalid URL format".to_string(),
    })?;
    ssrf_validate(&parsed)
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// `POST /v1/servers`
///
/// Creates a new MCP server configuration. Generates a slug from `display_name`
/// with a random suffix, retrying up to 5 times on slug collisions.
///
/// Returns 201 with [`ServerResponse`], including the computed `mcp_url`.
pub async fn create_server_handler(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Json(body): Json<CreateServerRequest>,
) -> Result<impl IntoResponse, AppError> {
    validate_display_name(&body.display_name)?;
    validate_config_ssrf(&body.config)?;

    let config_value = serde_json::to_value(&body.config)
        .map_err(|e| AppError::InternalServerError(format!("config serialization failed: {e}")))?;

    let response = state
        .server_service
        .create_server(user.id, &body.display_name, body.description.as_deref(), config_value)
        .await?;

    Ok((StatusCode::CREATED, Json(response)))
}

/// `GET /v1/servers`
///
/// Lists the authenticated user's servers, ordered by `created_at DESC`.
/// Supports forward cursor pagination via the `after` query parameter (UUID of
/// the last item from the previous page). Returns total count and `has_next`.
pub async fn list_servers_handler(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Query(params): Query<ListServersQuery>,
) -> Result<impl IntoResponse, AppError> {
    let (servers, total, has_next) = state
        .server_service
        .list_servers(user.id, &params)
        .await?;

    Ok(Json(ListServersResponse {
        servers,
        pagination: PaginationMeta { total, has_next },
    }))
}

/// `GET /v1/servers/{id}`
///
/// Returns a single server. Returns 404 if the server does not exist or belongs
/// to another user.
pub async fn get_server_handler(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(server_id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let response = state.server_service.get_server(user.id, server_id).await?;
    Ok(Json(response))
}

/// `PUT /v1/servers/{id}`
///
/// Full update for a server. Only fields present in the request body are applied;
/// omitted fields retain their current database values. Config is validated via
/// [`ServerConfigInput`] (unknown fields rejected, SSRF Phase 1 on `upstream_base_url`).
pub async fn update_server_handler(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(server_id): Path<Uuid>,
    Json(body): Json<UpdateServerRequest>,
) -> Result<impl IntoResponse, AppError> {
    // Validate inputs before touching the DB.
    if let Some(ref dn) = body.display_name {
        validate_display_name(dn)?;
    }
    if let Some(ref cfg) = body.config {
        validate_config_ssrf(cfg)?;
    }

    // Serialize config if present.
    let config_value = body
        .config
        .map(|cfg| {
            serde_json::to_value(&cfg)
                .map_err(|e| AppError::InternalServerError(format!("config serialize failed: {e}")))
        })
        .transpose()?;

    let response = state
        .server_service
        .update_server(
            user.id,
            server_id,
            body.display_name,
            body.description.map(Some),
            config_value,
            body.is_active,
        )
        .await?;

    Ok(Json(response))
}

/// `DELETE /v1/servers/{id}`
///
/// Deletes a server and all associated resources (credentials, tokens, audit log
/// entries are cascaded by the database `ON DELETE CASCADE` constraints).
/// Returns 204 No Content. Returns 404 if the server does not exist or belongs
/// to another user.
pub async fn delete_server_handler(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(server_id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    state.server_service.delete_server(user.id, server_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /v1/servers/{id}/validate-url`
///
/// Runs SSRF Phase 1 (synchronous, no DNS) on the provided URL.
/// Returns `{"valid": true}` on pass, or `{"valid": false, "error": {...}}` on failure.
/// Requires ownership of the server — returns 404 if not owned.
pub async fn validate_url_handler(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(server_id): Path<Uuid>,
    Json(body): Json<ValidateUrlRequest>,
) -> Result<impl IntoResponse, AppError> {
    // Ownership check (404 for foreign/missing servers).
    state
        .server_service
        .check_ownership(user.id, server_id)
        .await?;

    let response = match body.url.parse::<Url>() {
        Err(_) => ValidateUrlResponse {
            valid: false,
            error: Some(ValidateUrlError {
                code: "invalid_url".to_string(),
                message: "URL could not be parsed".to_string(),
            }),
        },
        Ok(parsed) => match ssrf_validate(&parsed) {
            Ok(()) => ValidateUrlResponse {
                valid: true,
                error: None,
            },
            Err(AppError::SsrfBlocked { reason, .. }) => ValidateUrlResponse {
                valid: false,
                error: Some(ValidateUrlError {
                    code: "ssrf_blocked".to_string(),
                    message: reason,
                }),
            },
            Err(other) => return Err(other),
        },
    };

    Ok(Json(response))
}
