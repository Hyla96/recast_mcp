//! Clerk webhook handler.
//!
//! Processes Clerk user lifecycle events (`user.created`, `user.updated`,
//! `user.deleted`) and synchronises the local `users` table. Svix signature
//! verification is performed against the raw request bytes before any JSON
//! parsing, preventing signature bypass via payload manipulation.
//!
//! This route is intentionally excluded from the JWT authentication
//! middleware — see `build_router_with_timeout` in `main.rs`.

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use mcp_common::{AppError, AuditAction, AuditEvent, SanitizedErrorMsg};
use serde::Deserialize;
use serde_json::Value;
use svix::webhooks::Webhook;

use crate::app_state::AppState;

// ── Clerk payload types ───────────────────────────────────────────────────────

/// Top-level Clerk webhook payload.
#[derive(Debug, Deserialize)]
struct ClerkWebhookPayload {
    /// Clerk event type, e.g. `"user.created"`.
    #[serde(rename = "type")]
    event_type: String,
    /// Event-specific payload data.
    data: Value,
}

/// An email address entry in a Clerk user object.
#[derive(Debug, Deserialize)]
struct ClerkEmailAddress {
    /// Unique identifier for this email address entry.
    id: String,
    /// The email address string.
    email_address: String,
}

/// Relevant fields from a Clerk `data` payload for user events.
#[derive(Debug, Deserialize)]
struct ClerkUserData {
    /// Clerk user identifier (the local `clerk_id`).
    id: Option<String>,
    /// All email addresses associated with the user.
    #[serde(default)]
    email_addresses: Vec<ClerkEmailAddress>,
    /// Points to the primary email entry's `id`.
    primary_email_address_id: Option<String>,
}

// ── Handler ───────────────────────────────────────────────────────────────────

/// `POST /v1/webhooks/clerk` — processes Clerk user lifecycle events.
///
/// **Not** protected by JWT auth middleware. Authentication is provided
/// exclusively by Svix webhook signature verification against
/// `CLERK_WEBHOOK_SECRET`.
///
/// Event dispatch:
/// - `user.created` → upsert user record
/// - `user.updated` → update email in local users table
/// - `user.deleted` → hard-delete user (cascades to servers and credentials)
/// - Any other event type → logged and ignored; returns 200
///
/// Returns 400 Bad Request if Svix signature verification fails.
pub async fn clerk_webhook_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl IntoResponse, AppError> {
    // ── 1. Verify Svix signature ──────────────────────────────────────────
    // Verification MUST happen against the raw bytes before any parsing.
    let wh = Webhook::new(&state.config.clerk_webhook_secret).map_err(|e| {
        tracing::error!("webhook: invalid CLERK_WEBHOOK_SECRET format: {e}");
        AppError::InternalServerError("Webhook configuration error.".to_string())
    })?;

    if let Err(e) = wh.verify(&body, &headers) {
        tracing::warn!("webhook: Svix signature verification failed: {e}");
        state.audit_logger.log(AuditEvent {
            action: AuditAction::WebhookAuthFailure,
            user_id: None,
            server_id: None,
            success: false,
            error_msg: Some(SanitizedErrorMsg::new("invalid Svix webhook signature")),
            metadata: None,
            correlation_id: None,
        });
        return Err(AppError::BadRequest(
            "Invalid webhook signature.".to_string(),
        ));
    }

    // ── 2. Parse payload ──────────────────────────────────────────────────
    let payload: ClerkWebhookPayload = serde_json::from_slice(&body).map_err(|e| {
        tracing::warn!("webhook: failed to parse payload JSON: {e}");
        AppError::BadRequest("Invalid webhook payload.".to_string())
    })?;

    tracing::debug!(event_type = %payload.event_type, "webhook: processing Clerk event");

    // ── 3. Dispatch on event type ─────────────────────────────────────────
    match payload.event_type.as_str() {
        "user.created" | "user.updated" => {
            handle_user_upsert(&state, &payload.data).await?;
        }
        "user.deleted" => {
            handle_user_delete(&state, &payload.data).await?;
        }
        other => {
            tracing::debug!(event_type = %other, "webhook: ignoring unhandled event type");
        }
    }

    Ok(StatusCode::OK)
}

// ── Private helpers ───────────────────────────────────────────────────────────

/// Resolves the primary email address from a Clerk user `data` payload.
///
/// Returns `None` if the primary email cannot be determined — callers
/// fall back to an empty string so that existing records are not broken
/// by a missing email in a partial update.
fn extract_primary_email(user: &ClerkUserData) -> Option<String> {
    let primary_id = user.primary_email_address_id.as_deref()?;
    user.email_addresses
        .iter()
        .find(|e| e.id == primary_id)
        .map(|e| e.email_address.clone())
}

/// Upserts a user record for `user.created` and `user.updated` events.
async fn handle_user_upsert(state: &AppState, data: &Value) -> Result<(), AppError> {
    let user: ClerkUserData = serde_json::from_value(data.clone()).map_err(|e| {
        tracing::warn!("webhook: failed to deserialize user data: {e}");
        AppError::BadRequest("Invalid user payload.".to_string())
    })?;

    let clerk_id = user.id.clone().ok_or_else(|| {
        tracing::warn!("webhook: user.created/updated payload missing id");
        AppError::BadRequest("Missing user id in webhook payload.".to_string())
    })?;

    let email = extract_primary_email(&user).unwrap_or_default();

    sqlx::query(
        r#"INSERT INTO users (clerk_id, email)
           VALUES ($1, $2)
           ON CONFLICT (clerk_id)
           DO UPDATE SET email = EXCLUDED.email, updated_at = NOW()"#,
    )
    .bind(&clerk_id)
    .bind(&email)
    .execute(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("webhook: user upsert failed for clerk_id={clerk_id}: {e}");
        AppError::InternalServerError("Failed to upsert user record.".to_string())
    })?;

    tracing::info!(clerk_id = %clerk_id, email = %email, "webhook: user upserted");
    Ok(())
}

/// Hard-deletes a user record for `user.deleted` events.
///
/// Cascade deletes apply: mcp_servers and credentials linked to the user
/// are removed automatically via DB `ON DELETE CASCADE` constraints.
async fn handle_user_delete(state: &AppState, data: &Value) -> Result<(), AppError> {
    let user: ClerkUserData = serde_json::from_value(data.clone()).map_err(|e| {
        tracing::warn!("webhook: failed to deserialize user.deleted data: {e}");
        AppError::BadRequest("Invalid user payload.".to_string())
    })?;

    let clerk_id = match user.id {
        Some(id) => id,
        None => {
            // Clerk occasionally sends user.deleted with a null id if the user
            // was already purged. Treat this as a no-op rather than an error.
            tracing::warn!("webhook: user.deleted event has null id — skipping");
            return Ok(());
        }
    };

    let result = sqlx::query("DELETE FROM users WHERE clerk_id = $1")
        .bind(&clerk_id)
        .execute(&state.pool)
        .await
        .map_err(|e| {
            tracing::error!("webhook: user delete failed for clerk_id={clerk_id}: {e}");
            AppError::InternalServerError("Failed to delete user record.".to_string())
        })?;

    tracing::info!(
        clerk_id = %clerk_id,
        rows_affected = result.rows_affected(),
        "webhook: user deleted"
    );
    Ok(())
}
