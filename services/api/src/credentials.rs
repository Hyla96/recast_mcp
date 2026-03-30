//! Credential encryption service.
//!
//! Provides encrypted storage, rotation, deletion, and listing of API
//! credentials. Plaintext values never leave this module unencrypted — the
//! returned [`CredentialMeta`] contains only non-sensitive fields. This is
//! enforced at compile time by the struct definition (no `encrypted_payload`,
//! `iv`, or `value` fields on [`CredentialMeta`]).
//!
//! # Security contract
//!
//! - `plaintext` is accepted as [`Zeroizing<String>`] — the wrapper zeroes
//!   the backing memory when the value is dropped.
//! - Encryption uses AES-256-GCM with a per-row random IV. The output
//!   `IV || ciphertext` is stored in `encrypted_payload`; the IV is also
//!   stored separately in the `iv` column for the credential-injector sidecar.
//! - After a successful `rotate`, a `pg_notify('credential_updated', server_id)`
//!   is sent so the injector sidecar can evict its LRU cache entry immediately.
//! - Audit events are emitted for every mutating operation.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use mcp_common::{AppError, AuditAction, AuditEvent, AuditLogger};
use mcp_crypto::{encrypt, CryptoKey};
use serde::Serialize;
use sqlx::{PgPool, Row};
use uuid::Uuid;
use zeroize::Zeroizing;

// ── CredentialMeta ────────────────────────────────────────────────────────────

/// Non-sensitive metadata about a stored credential.
///
/// This struct deliberately omits all sensitive fields:
/// - No `encrypted_payload` column data.
/// - No `iv` column data.
/// - No `value` or `plaintext` field of any kind.
///
/// Compile-time enforcement: the struct definition makes it impossible to
/// accidentally include sensitive data in API responses.
#[derive(Debug, Clone, Serialize)]
pub struct CredentialMeta {
    /// Unique credential identifier.
    pub id: Uuid,
    /// The MCP server this credential belongs to.
    pub server_id: Uuid,
    /// Authentication type: `bearer`, `api_key_header`, `api_key_query`, or `basic`.
    pub auth_type: String,
    /// The header or query-parameter name for `api_key_header` / `api_key_query`
    /// credential types. `None` for `bearer` and `basic` credentials.
    pub key_name: Option<String>,
    /// Non-sensitive preview of the stored value (e.g. `"supe****"`).
    ///
    /// Computed at creation time from the first 4 characters of the plaintext.
    /// Stored in the database because the encrypted payload is not reversible.
    /// `None` for records created before this column was added.
    pub hint: Option<String>,
    /// Timestamp when this credential was originally created.
    pub created_at: DateTime<Utc>,
}

// ── Hint helpers ──────────────────────────────────────────────────────────────

/// Computes a non-sensitive display hint from the first 4 chars of a value.
///
/// Examples:
/// - `"super-secret-key"` → `"supe****"`
/// - `"abc"` → `"abc****"` (shorter than 4 chars is shown in full)
pub fn compute_hint(value: &str) -> String {
    let prefix: String = value.chars().take(4).collect();
    format!("{prefix}****")
}

// ── CredentialService ─────────────────────────────────────────────────────────

/// Service for storing, rotating, deleting, and listing encrypted credentials.
///
/// `CredentialService` is cheaply cloneable — all clones share the same pool,
/// key, and audit logger (each is internally `Arc`-wrapped).
#[derive(Clone)]
pub struct CredentialService {
    pool: PgPool,
    crypto_key: Arc<CryptoKey>,
    audit_logger: AuditLogger,
}

impl CredentialService {
    /// Creates a new `CredentialService`.
    ///
    /// # Arguments
    ///
    /// * `pool` — Shared PostgreSQL connection pool.
    /// * `crypto_key` — AES-256-GCM key for encrypting/decrypting credential values.
    /// * `audit_logger` — For emitting `CredentialCreate`, `CredentialRotate`,
    ///   and `CredentialDelete` audit events.
    pub fn new(pool: PgPool, crypto_key: Arc<CryptoKey>, audit_logger: AuditLogger) -> Self {
        Self {
            pool,
            crypto_key,
            audit_logger,
        }
    }

    /// Stores a new encrypted credential for the given server.
    ///
    /// The `plaintext` is encrypted immediately with AES-256-GCM using a
    /// per-row random IV. The [`Zeroizing`] wrapper ensures the plaintext is
    /// zeroed from memory as soon as encryption completes.
    ///
    /// # Arguments
    ///
    /// * `server_id` — The MCP server that owns this credential.
    /// * `credential_type` — One of `bearer`, `api_key_header`, `api_key_query`, `basic`.
    /// * `key_name` — For `api_key_header`: the header name. For `api_key_query`:
    ///   the query param name. `None` for `bearer` and `basic`.
    /// * `plaintext` — The raw credential value (API key, token, etc.). Zeroed on drop.
    /// * `user_id` — The authenticated user performing this action, for audit logging.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::InternalServerError`] if encryption or the database
    /// insert fails.
    pub async fn store(
        &self,
        server_id: Uuid,
        credential_type: &str,
        key_name: Option<&str>,
        plaintext: Zeroizing<String>,
        user_id: Option<Uuid>,
    ) -> Result<CredentialMeta, AppError> {
        // Compute hint before consuming plaintext (first 4 chars of value).
        let hint = compute_hint(plaintext.as_str());

        // Encrypt — produces IV (12 bytes) || ciphertext as a single blob.
        // Plaintext bytes are only read here; `plaintext` zeroes on drop.
        let encrypted = encrypt(&self.crypto_key, plaintext.as_bytes())
            .map_err(|e| AppError::InternalServerError(format!("encryption failed: {e}")))?;

        // IV is always the first 12 bytes; store separately for the injector sidecar.
        let iv_bytes = encrypted[..12].to_vec();

        let row = sqlx::query(
            "INSERT INTO credentials (server_id, auth_type, key_name, encrypted_payload, iv, hint)
             VALUES ($1, $2, $3, $4, $5, $6)
             RETURNING id, server_id, auth_type, key_name, hint, created_at",
        )
        .bind(server_id)
        .bind(credential_type)
        .bind(key_name)
        .bind(&encrypted[..])
        .bind(&iv_bytes[..])
        .bind(&hint)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| AppError::InternalServerError(format!("credential insert failed: {e}")))?;

        let meta = row_to_meta(&row)?;

        self.audit_logger.log(AuditEvent {
            action: AuditAction::CredentialCreate,
            user_id,
            server_id: Some(server_id),
            success: true,
            error_msg: None,
            metadata: Some(serde_json::json!({ "auth_type": credential_type })),
            correlation_id: None,
        });

        Ok(meta)
    }

    /// Rotates a credential, replacing its encrypted value atomically.
    ///
    /// Verifies that `credential_id` belongs to `server_id` before updating.
    /// After a successful update, sends `pg_notify('credential_updated', server_id)`
    /// so the credential-injector sidecar evicts the cache entry immediately.
    ///
    /// # Errors
    ///
    /// - [`AppError::NotFound`] — `credential_id` does not exist.
    /// - [`AppError::Forbidden`] — `credential_id` belongs to a different server.
    /// - [`AppError::InternalServerError`] — encryption or DB failure.
    pub async fn rotate(
        &self,
        credential_id: Uuid,
        server_id: Uuid,
        new_plaintext: Zeroizing<String>,
        user_id: Option<Uuid>,
    ) -> Result<CredentialMeta, AppError> {
        // Ownership check: fetch the credential to verify server ownership.
        let existing =
            sqlx::query("SELECT server_id FROM credentials WHERE id = $1")
                .bind(credential_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| AppError::InternalServerError(format!("db lookup failed: {e}")))?
                .ok_or_else(|| AppError::NotFound("credential not found".to_string()))?;

        let existing_server_id: Uuid = existing
            .try_get("server_id")
            .map_err(|e| AppError::InternalServerError(format!("row decode server_id: {e}")))?;

        if existing_server_id != server_id {
            return Err(AppError::Forbidden(
                "credential does not belong to this server".to_string(),
            ));
        }

        // Encrypt the new plaintext.
        let encrypted = encrypt(&self.crypto_key, new_plaintext.as_bytes())
            .map_err(|e| AppError::InternalServerError(format!("encryption failed: {e}")))?;

        let iv_bytes = encrypted[..12].to_vec();

        let row = sqlx::query(
            "UPDATE credentials
             SET encrypted_payload = $2, iv = $3
             WHERE id = $1
             RETURNING id, server_id, auth_type, key_name, hint, created_at",
        )
        .bind(credential_id)
        .bind(&encrypted[..])
        .bind(&iv_bytes[..])
        .fetch_one(&self.pool)
        .await
        .map_err(|e| AppError::InternalServerError(format!("credential update failed: {e}")))?;

        let meta = row_to_meta(&row)?;

        // Notify the injector sidecar to evict the cache entry for this server.
        let notify_payload = server_id.to_string();
        if let Err(e) = sqlx::query("SELECT pg_notify('credential_updated', $1)")
            .bind(&notify_payload)
            .execute(&self.pool)
            .await
        {
            tracing::warn!(
                error = %e,
                server_id = %server_id,
                "pg_notify credential_updated failed — injector cache may be stale"
            );
        }

        self.audit_logger.log(AuditEvent {
            action: AuditAction::CredentialRotate,
            user_id,
            server_id: Some(server_id),
            success: true,
            error_msg: None,
            metadata: Some(serde_json::json!({ "credential_id": credential_id.to_string() })),
            correlation_id: None,
        });

        Ok(meta)
    }

    /// Deletes a credential by ID.
    ///
    /// Verifies that `credential_id` belongs to `server_id` before deleting.
    ///
    /// # Errors
    ///
    /// - [`AppError::NotFound`] — `credential_id` does not exist.
    /// - [`AppError::Forbidden`] — `credential_id` belongs to a different server.
    /// - [`AppError::InternalServerError`] — DB failure.
    pub async fn delete(
        &self,
        credential_id: Uuid,
        server_id: Uuid,
        user_id: Option<Uuid>,
    ) -> Result<(), AppError> {
        // Ownership check: fetch the credential to verify server ownership.
        let existing =
            sqlx::query("SELECT server_id FROM credentials WHERE id = $1")
                .bind(credential_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| AppError::InternalServerError(format!("db lookup failed: {e}")))?
                .ok_or_else(|| AppError::NotFound("credential not found".to_string()))?;

        let existing_server_id: Uuid = existing
            .try_get("server_id")
            .map_err(|e| AppError::InternalServerError(format!("row decode server_id: {e}")))?;

        if existing_server_id != server_id {
            return Err(AppError::Forbidden(
                "credential does not belong to this server".to_string(),
            ));
        }

        sqlx::query("DELETE FROM credentials WHERE id = $1")
            .bind(credential_id)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                AppError::InternalServerError(format!("credential delete failed: {e}"))
            })?;

        self.audit_logger.log(AuditEvent {
            action: AuditAction::CredentialDelete,
            user_id,
            server_id: Some(server_id),
            success: true,
            error_msg: None,
            metadata: Some(serde_json::json!({ "credential_id": credential_id.to_string() })),
            correlation_id: None,
        });

        Ok(())
    }

    /// Lists credential metadata for a server. Never returns sensitive fields.
    ///
    /// Results are ordered by `created_at` descending (newest first).
    ///
    /// # Errors
    ///
    /// Returns [`AppError::InternalServerError`] on DB failures.
    pub async fn list_for_server(
        &self,
        server_id: Uuid,
    ) -> Result<Vec<CredentialMeta>, AppError> {
        let rows = sqlx::query(
            "SELECT id, server_id, auth_type, key_name, hint, created_at
             FROM credentials
             WHERE server_id = $1
             ORDER BY created_at DESC",
        )
        .bind(server_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::InternalServerError(format!("list credentials failed: {e}")))?;

        rows.iter().map(row_to_meta).collect()
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Maps a PostgreSQL row to [`CredentialMeta`].
///
/// Reads only the non-sensitive columns: `id`, `server_id`, `auth_type`,
/// `key_name`, `created_at`. The `encrypted_payload` and `iv` columns are
/// never touched by this function.
fn row_to_meta(row: &sqlx::postgres::PgRow) -> Result<CredentialMeta, AppError> {
    Ok(CredentialMeta {
        id: row
            .try_get("id")
            .map_err(|e| AppError::InternalServerError(format!("row decode id: {e}")))?,
        server_id: row
            .try_get("server_id")
            .map_err(|e| AppError::InternalServerError(format!("row decode server_id: {e}")))?,
        auth_type: row
            .try_get("auth_type")
            .map_err(|e| AppError::InternalServerError(format!("row decode auth_type: {e}")))?,
        key_name: row
            .try_get("key_name")
            .map_err(|e| AppError::InternalServerError(format!("row decode key_name: {e}")))?,
        hint: row
            .try_get("hint")
            .map_err(|e| AppError::InternalServerError(format!("row decode hint: {e}")))?,
        created_at: row
            .try_get("created_at")
            .map_err(|e| AppError::InternalServerError(format!("row decode created_at: {e}")))?,
    })
}
