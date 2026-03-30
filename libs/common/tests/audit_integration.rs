// Audit logger integration tests.
//
// These tests require a live PostgreSQL instance and the `testing` feature.
// Run with:
//
//   TEST_DATABASE_URL=postgres://recast:recast@localhost:5432/postgres \
//     cargo test -p mcp-common --features testing --test audit_integration
//
// Each test receives its own isolated database (via TestDatabase) so tests run
// in parallel without data contamination.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, missing_docs)]
#![cfg(feature = "testing")]

use std::time::{Duration, Instant};

use mcp_common::{
    audit::{AuditAction, AuditEvent, AuditLogger},
    testing::TestDatabase,
    SanitizedErrorMsg,
};
use uuid::Uuid;

// ── Helper ────────────────────────────────────────────────────────────────────

fn make_event(action: AuditAction) -> AuditEvent {
    AuditEvent {
        action,
        user_id: None,
        server_id: None,
        success: true,
        error_msg: None,
        metadata: None,
        correlation_id: None,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Events must appear in audit_log within 200 ms of being enqueued.
#[tokio::test]
async fn test_events_appear_within_200ms() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let logger = AuditLogger::new(db.pool.clone());

    let user_id = Uuid::new_v4();
    logger.log(AuditEvent {
        action: AuditAction::AuthSuccess,
        user_id: Some(user_id),
        server_id: None,
        success: true,
        error_msg: None,
        metadata: None,
        correlation_id: None,
    });

    // Poll until the event appears or 200 ms elapse.
    let deadline = Instant::now() + Duration::from_millis(200);
    loop {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_log WHERE actor_id = $1 AND action = 'auth_success'",
        )
        .bind(user_id)
        .fetch_one(&db.pool)
        .await
        .expect("count query failed");

        if count == 1 {
            break;
        }

        assert!(
            Instant::now() < deadline,
            "event not visible in audit_log after 200ms"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    logger.shutdown().await;
}

/// All 15 AuditAction variants can be written to the database.
#[tokio::test]
async fn test_all_action_variants_written() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let logger = AuditLogger::new(db.pool.clone());

    let actions = [
        AuditAction::AuthSuccess,
        AuditAction::AuthFailure,
        AuditAction::WebhookAuthFailure,
        AuditAction::CredentialCreate,
        AuditAction::CredentialRotate,
        AuditAction::CredentialDelete,
        AuditAction::CredentialAccess,
        AuditAction::CredentialAccessFailure,
        AuditAction::SsrfBlock,
        AuditAction::RateLimitExceeded,
        AuditAction::ServerCreate,
        AuditAction::ServerUpdate,
        AuditAction::ServerDelete,
        AuditAction::ServerTokenGenerate,
        AuditAction::ServerTokenRevoke,
    ];

    for action in actions {
        logger.log(make_event(action));
    }

    logger.shutdown().await;

    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_log")
            .fetch_one(&db.pool)
            .await
            .expect("count query failed");

    assert_eq!(count, 15, "all 15 action variants should be persisted");
}

/// AuditEvent fields (user_id, server_id, success, error_msg, correlation_id)
/// are correctly stored in the database.
#[tokio::test]
async fn test_event_fields_stored_correctly() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let logger = AuditLogger::new(db.pool.clone());

    let user_id = Uuid::new_v4();
    let server_id = Uuid::new_v4();

    logger.log(AuditEvent {
        action: AuditAction::CredentialCreate,
        user_id: Some(user_id),
        server_id: Some(server_id),
        success: false,
        error_msg: Some(SanitizedErrorMsg::new("encryption key missing")),
        metadata: Some(serde_json::json!({"key_name": "stripe_key"})),
        correlation_id: Some("trace-xyz".to_string()),
    });

    logger.shutdown().await;

    let row = sqlx::query(
        "SELECT actor_id, action, resource_id, metadata
         FROM audit_log
         WHERE actor_id = $1",
    )
    .bind(user_id)
    .fetch_one(&db.pool)
    .await
    .expect("fetch failed");

    use sqlx::Row;
    let actor_id: Uuid = row.get("actor_id");
    let action: String = row.get("action");
    let resource_id: Uuid = row.get("resource_id");
    let metadata: serde_json::Value = row.get("metadata");

    assert_eq!(actor_id, user_id);
    assert_eq!(action, "credential_create");
    assert_eq!(resource_id, server_id);
    assert_eq!(metadata["success"], serde_json::Value::Bool(false));
    assert_eq!(metadata["error_msg"], "encryption key missing");
    assert_eq!(metadata["correlation_id"], "trace-xyz");
    assert_eq!(metadata["key_name"], "stripe_key");
}

/// The audit_log immutability trigger prevents UPDATE operations.
#[tokio::test]
async fn test_audit_log_immutable_trigger_blocks_update() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let logger = AuditLogger::new(db.pool.clone());

    logger.log(make_event(AuditAction::ServerCreate));
    logger.shutdown().await;

    // Attempting to UPDATE an audit_log row must fail.
    let result = sqlx::query("UPDATE audit_log SET action = 'tampered' WHERE TRUE")
        .execute(&db.pool)
        .await;

    assert!(result.is_err(), "UPDATE on audit_log must be rejected by trigger");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("immutable"),
        "error message must mention immutability; got: {err}"
    );
}

/// The audit_log immutability trigger prevents DELETE operations.
#[tokio::test]
async fn test_audit_log_immutable_trigger_blocks_delete() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let logger = AuditLogger::new(db.pool.clone());

    logger.log(make_event(AuditAction::AuthFailure));
    logger.shutdown().await;

    // Attempting to DELETE from audit_log must fail.
    let result = sqlx::query("DELETE FROM audit_log WHERE TRUE")
        .execute(&db.pool)
        .await;

    assert!(result.is_err(), "DELETE on audit_log must be rejected by trigger");
}

/// Batching: 50 events should be written in a single flush.
#[tokio::test]
async fn test_batch_flush_at_50_events() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let logger = AuditLogger::new(db.pool.clone());

    for _ in 0..50 {
        logger.log(make_event(AuditAction::CredentialAccess));
    }

    // Wait for the batch to flush (100ms timer + some buffer).
    tokio::time::sleep(Duration::from_millis(300)).await;

    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_log WHERE action = 'credential_access'")
            .fetch_one(&db.pool)
            .await
            .expect("count query failed");

    assert_eq!(count, 50, "50 events must be persisted after batch flush");

    logger.shutdown().await;
}
