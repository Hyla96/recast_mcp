// Platform API — CredentialService integration tests.
//
// Tests exercise store, rotate, delete, and list_for_server against a real
// PostgreSQL database (TestDatabase). Each test gets an isolated DB.
//
// Required environment variable:
//   TEST_DATABASE_URL=postgres://recast:recast@localhost:5432/postgres \
//     cargo test -p mcp-api --test credential_tests
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    missing_docs
)]

use mcp_api::credentials::CredentialService;
use mcp_common::{testing::TestDatabase, AuditLogger, AppError};
use mcp_crypto::CryptoKey;
use sqlx::Row;
use std::sync::Arc;
use uuid::Uuid;
use zeroize::Zeroizing;

// ── Helpers ───────────────────────────────────────────────────────────────────

/// A deterministic 32-byte test key (all 0x42 bytes).
fn test_crypto_key() -> Arc<CryptoKey> {
    Arc::new(CryptoKey::from_bytes([0x42u8; 32]))
}

/// Builds a `CredentialService` backed by the given pool and test key.
fn make_service(pool: sqlx::PgPool) -> CredentialService {
    let key = test_crypto_key();
    let logger = AuditLogger::new(pool.clone());
    CredentialService::new(pool, key, logger)
}

/// Inserts a test user and returns the user UUID.
async fn insert_user(pool: &sqlx::PgPool) -> Uuid {
    let clerk_id = format!("clerk_{}", Uuid::new_v4().simple());
    let email = format!("{}@test.example.com", Uuid::new_v4().simple());
    sqlx::query("INSERT INTO users (clerk_id, email) VALUES ($1, $2) RETURNING id")
        .bind(&clerk_id)
        .bind(&email)
        .fetch_one(pool)
        .await
        .expect("insert user")
        .try_get("id")
        .expect("user id")
}

/// Inserts a test MCP server owned by `user_id` and returns the server UUID.
async fn insert_server(pool: &sqlx::PgPool, user_id: Uuid) -> Uuid {
    let slug = format!("test-{}", Uuid::new_v4().simple());
    sqlx::query(
        "INSERT INTO mcp_servers (user_id, name, slug) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(user_id)
    .bind("Test Server")
    .bind(&slug)
    .fetch_one(pool)
    .await
    .expect("insert server")
    .try_get("id")
    .expect("server id")
}

// ── store tests ───────────────────────────────────────────────────────────────

/// Storing a credential encrypts the value: the raw DB payload differs from
/// the original plaintext (confirming AES-GCM encryption, not plain storage).
#[tokio::test]
async fn test_store_encrypts_value() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let service = make_service(db.pool.clone());

    let user_id = insert_user(&db.pool).await;
    let server_id = insert_server(&db.pool, user_id).await;

    let plaintext = "super-secret-api-key-12345";
    let meta = service
        .store(
            server_id,
            "bearer",
            None,
            Zeroizing::new(plaintext.to_string()),
            Some(user_id),
        )
        .await
        .expect("store succeeded");

    // Raw DB query: encrypted_payload must differ from plaintext.
    let row =
        sqlx::query("SELECT encrypted_payload FROM credentials WHERE id = $1")
            .bind(meta.id)
            .fetch_one(&db.pool)
            .await
            .expect("select credential");

    let stored_payload: Vec<u8> = row.try_get("encrypted_payload").expect("encrypted_payload");
    assert_ne!(
        stored_payload.as_slice(),
        plaintext.as_bytes(),
        "encrypted_payload must not equal original plaintext"
    );

    // The payload must be non-empty and at least 12 bytes (IV alone is 12).
    assert!(stored_payload.len() > 12, "payload should contain IV + ciphertext");
}

/// store returns a CredentialMeta with the correct non-sensitive fields.
#[tokio::test]
async fn test_store_returns_correct_meta() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let service = make_service(db.pool.clone());

    let user_id = insert_user(&db.pool).await;
    let server_id = insert_server(&db.pool, user_id).await;

    let meta = service
        .store(
            server_id,
            "api_key_header",
            Some("X-API-Key"),
            Zeroizing::new("my-api-key".to_string()),
            Some(user_id),
        )
        .await
        .expect("store succeeded");

    assert_eq!(meta.server_id, server_id);
    assert_eq!(meta.auth_type, "api_key_header");
    assert_eq!(meta.key_name.as_deref(), Some("X-API-Key"));
    // id must be a non-nil UUID
    assert_ne!(meta.id, Uuid::nil());
}

// ── list_for_server tests ─────────────────────────────────────────────────────

/// list_for_server returns metadata only — no sensitive BYTEA columns.
#[tokio::test]
async fn test_list_for_server_returns_meta_only() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let service = make_service(db.pool.clone());

    let user_id = insert_user(&db.pool).await;
    let server_id = insert_server(&db.pool, user_id).await;

    // Store two credentials.
    service
        .store(
            server_id,
            "bearer",
            None,
            Zeroizing::new("token-one".to_string()),
            Some(user_id),
        )
        .await
        .expect("store 1");
    service
        .store(
            server_id,
            "api_key_query",
            Some("api_key"),
            Zeroizing::new("token-two".to_string()),
            Some(user_id),
        )
        .await
        .expect("store 2");

    let list = service
        .list_for_server(server_id)
        .await
        .expect("list_for_server");

    assert_eq!(list.len(), 2, "expected two credentials");

    // Verify CredentialMeta fields — compile-time guarantee that sensitive
    // fields are absent, but also runtime check of non-sensitive content.
    for m in &list {
        assert_eq!(m.server_id, server_id);
        assert!(!m.auth_type.is_empty());
    }
}

/// list_for_server returns an empty list when the server has no credentials.
#[tokio::test]
async fn test_list_for_server_empty() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let service = make_service(db.pool.clone());

    let user_id = insert_user(&db.pool).await;
    let server_id = insert_server(&db.pool, user_id).await;

    let list = service
        .list_for_server(server_id)
        .await
        .expect("list_for_server");

    assert!(list.is_empty());
}

// ── rotate tests ──────────────────────────────────────────────────────────────

/// Rotating a credential changes the encrypted payload in the database.
#[tokio::test]
async fn test_rotate_changes_encrypted_payload() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let service = make_service(db.pool.clone());

    let user_id = insert_user(&db.pool).await;
    let server_id = insert_server(&db.pool, user_id).await;

    let meta = service
        .store(
            server_id,
            "bearer",
            None,
            Zeroizing::new("original-token".to_string()),
            Some(user_id),
        )
        .await
        .expect("store");

    // Capture the payload before rotation.
    let before: Vec<u8> = sqlx::query(
        "SELECT encrypted_payload FROM credentials WHERE id = $1",
    )
    .bind(meta.id)
    .fetch_one(&db.pool)
    .await
    .expect("select before")
    .try_get("encrypted_payload")
    .expect("payload before");

    // Rotate to a new value.
    let rotated = service
        .rotate(
            meta.id,
            server_id,
            Zeroizing::new("rotated-token".to_string()),
            Some(user_id),
        )
        .await
        .expect("rotate");

    assert_eq!(rotated.id, meta.id);
    assert_eq!(rotated.server_id, server_id);

    // Capture the payload after rotation.
    let after: Vec<u8> = sqlx::query(
        "SELECT encrypted_payload FROM credentials WHERE id = $1",
    )
    .bind(meta.id)
    .fetch_one(&db.pool)
    .await
    .expect("select after")
    .try_get("encrypted_payload")
    .expect("payload after");

    assert_ne!(
        before, after,
        "encrypted_payload must change after rotate"
    );
}

/// Rotating a credential that belongs to a different server returns Forbidden.
#[tokio::test]
async fn test_rotate_wrong_server_returns_forbidden() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let service = make_service(db.pool.clone());

    let user_id = insert_user(&db.pool).await;
    let server_a = insert_server(&db.pool, user_id).await;
    let server_b = insert_server(&db.pool, user_id).await;

    // Store credential under server A.
    let meta = service
        .store(
            server_a,
            "bearer",
            None,
            Zeroizing::new("token-for-server-a".to_string()),
            Some(user_id),
        )
        .await
        .expect("store");

    // Try to rotate using server B's ID — must return Forbidden.
    let result = service
        .rotate(
            meta.id,
            server_b, // wrong server
            Zeroizing::new("new-token".to_string()),
            Some(user_id),
        )
        .await;

    assert!(
        matches!(result, Err(AppError::Forbidden(_))),
        "expected Forbidden, got: {result:?}"
    );
}

/// Rotating a non-existent credential returns NotFound.
#[tokio::test]
async fn test_rotate_not_found_returns_not_found() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let service = make_service(db.pool.clone());

    let user_id = insert_user(&db.pool).await;
    let server_id = insert_server(&db.pool, user_id).await;

    let result = service
        .rotate(
            Uuid::new_v4(), // random, non-existent ID
            server_id,
            Zeroizing::new("irrelevant".to_string()),
            Some(user_id),
        )
        .await;

    assert!(
        matches!(result, Err(AppError::NotFound(_))),
        "expected NotFound, got: {result:?}"
    );
}

// ── delete tests ──────────────────────────────────────────────────────────────

/// Deleting a credential removes the row from the database.
#[tokio::test]
async fn test_delete_removes_credential() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let service = make_service(db.pool.clone());

    let user_id = insert_user(&db.pool).await;
    let server_id = insert_server(&db.pool, user_id).await;

    let meta = service
        .store(
            server_id,
            "bearer",
            None,
            Zeroizing::new("delete-me-token".to_string()),
            Some(user_id),
        )
        .await
        .expect("store");

    service
        .delete(meta.id, server_id, Some(user_id))
        .await
        .expect("delete");

    // Confirm the row is gone.
    let found = sqlx::query("SELECT id FROM credentials WHERE id = $1")
        .bind(meta.id)
        .fetch_optional(&db.pool)
        .await
        .expect("select after delete");

    assert!(found.is_none(), "credential must be deleted from DB");
}

/// Deleting a credential that belongs to a different server returns Forbidden.
#[tokio::test]
async fn test_delete_wrong_server_returns_forbidden() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let service = make_service(db.pool.clone());

    let user_id = insert_user(&db.pool).await;
    let server_a = insert_server(&db.pool, user_id).await;
    let server_b = insert_server(&db.pool, user_id).await;

    let meta = service
        .store(
            server_a,
            "bearer",
            None,
            Zeroizing::new("token-for-server-a".to_string()),
            Some(user_id),
        )
        .await
        .expect("store");

    let result = service
        .delete(meta.id, server_b, Some(user_id)) // wrong server
        .await;

    assert!(
        matches!(result, Err(AppError::Forbidden(_))),
        "expected Forbidden, got: {result:?}"
    );

    // Credential must still exist (delete was rejected).
    let still_exists = sqlx::query("SELECT id FROM credentials WHERE id = $1")
        .bind(meta.id)
        .fetch_optional(&db.pool)
        .await
        .expect("select after rejected delete");

    assert!(still_exists.is_some(), "credential must not be deleted after Forbidden");
}

/// Deleting a non-existent credential returns NotFound.
#[tokio::test]
async fn test_delete_not_found_returns_not_found() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let service = make_service(db.pool.clone());

    let user_id = insert_user(&db.pool).await;
    let server_id = insert_server(&db.pool, user_id).await;

    let result = service
        .delete(Uuid::new_v4(), server_id, Some(user_id))
        .await;

    assert!(
        matches!(result, Err(AppError::NotFound(_))),
        "expected NotFound, got: {result:?}"
    );
}
