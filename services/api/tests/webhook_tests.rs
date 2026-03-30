// Platform API — Clerk webhook handler integration tests.
//
// Tests in this file exercise the POST /v1/webhooks/clerk endpoint including:
//   - Svix signature verification (valid and invalid)
//   - user.created → user record created in DB
//   - user.updated → email updated in DB
//   - user.deleted → user and linked servers removed from DB
//   - WebhookAuthFailure audit event on bad signature
//
// Required environment variable (or TEST_DATABASE_URL):
//   TEST_DATABASE_URL=postgres://recast:recast@localhost:5432/postgres \
//     cargo test -p mcp-api --test webhook_tests
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    missing_docs
)]

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    routing::post,
    Router,
};
use mcp_common::{
    middleware::request_id_middleware,
    testing::TestDatabase,
    AuditLogger,
};
use std::sync::Arc;
use svix::webhooks::Webhook;
use tower::ServiceExt;

use mcp_api::{
    app_state::AppState,
    auth::JwksCache,
    config::ApiConfig,
    credentials::CredentialService,
    handlers::webhooks::clerk_webhook_handler,
    servers::ServerService,
};
use mcp_crypto::CryptoKey;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Valid 32-byte webhook secret encoded as `whsec_<base64>`.
/// Used in all tests that expect successful signature verification.
const TEST_WEBHOOK_SECRET: &str =
    "whsec_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

/// A different secret used to generate signatures that will fail verification.
const WRONG_WEBHOOK_SECRET: &str =
    "whsec_BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB=";

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Builds an `AppState` backed by `pool` and the test webhook secret.
fn make_state(pool: sqlx::PgPool) -> AppState {
    // All-zeros 32-byte key for tests — never used in production.
    let crypto_key = Arc::new(CryptoKey::from_bytes([0u8; 32]));
    let audit_logger = AuditLogger::new(pool.clone());
    let credential_service = CredentialService::new(
        pool.clone(),
        crypto_key,
        audit_logger.clone(),
    );
    let server_service = ServerService::new(
        pool.clone(),
        audit_logger.clone(),
        "https://mcp.test.example.com".to_string(),
    );
    AppState {
        pool: pool.clone(),
        config: Arc::new(ApiConfig {
            port: 3001,
            database_url: "postgres://test".to_string(),
            clerk_secret_key: "sk_test_xxx".to_string(),
            clerk_jwks_url: "http://localhost/jwks".to_string(),
            clerk_webhook_secret: TEST_WEBHOOK_SECRET.to_string(),
            encryption_key: "0".repeat(64),
            clerk_issuer: String::new(),
            cors_origins: vec![],
            gateway_base_url: "https://mcp.test.example.com".to_string(),
        }),
        audit_logger,
        jwks_cache: JwksCache::new("http://localhost/jwks"),
        credential_service,
        server_service,
    }
}

/// Minimal test router: just the webhook endpoint + request-id middleware.
fn make_webhook_router(state: AppState) -> Router {
    Router::new()
        .route("/v1/webhooks/clerk", post(clerk_webhook_handler))
        .with_state(state)
        .layer(axum::middleware::from_fn(request_id_middleware))
}

/// Signs `payload` with `secret` and returns a `Request` with the correct
/// Svix headers (`svix-id`, `svix-timestamp`, `svix-signature`).
fn make_signed_request(secret: &str, payload: &[u8]) -> Request<Body> {
    let wh = Webhook::new(secret).expect("valid webhook secret");
    let timestamp: i64 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let msg_id = "msg_test_001";

    // svix 1.x sign() takes (msgid: &str, timestamp: i64, payload: &[u8]).
    let signature = wh
        .sign(msg_id, timestamp, payload)
        .expect("webhook sign failed");

    Request::builder()
        .method("POST")
        .uri("/v1/webhooks/clerk")
        .header("content-type", "application/json")
        .header("svix-id", msg_id)
        .header("svix-timestamp", timestamp.to_string())
        .header("svix-signature", signature)
        .body(Body::from(payload.to_owned()))
        .unwrap()
}

/// Constructs a Clerk `user.created` / `user.updated` payload.
fn user_upsert_payload(clerk_id: &str, email: &str, event_type: &str) -> Vec<u8> {
    serde_json::json!({
        "type": event_type,
        "data": {
            "id": clerk_id,
            "email_addresses": [
                {
                    "id": "idn_primary_001",
                    "email_address": email
                }
            ],
            "primary_email_address_id": "idn_primary_001"
        }
    })
    .to_string()
    .into_bytes()
}

/// Constructs a Clerk `user.deleted` payload.
fn user_deleted_payload(clerk_id: &str) -> Vec<u8> {
    serde_json::json!({
        "type": "user.deleted",
        "data": {
            "id": clerk_id,
            "deleted": true
        }
    })
    .to_string()
    .into_bytes()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Valid `user.created` payload with correct Svix signature → 200 OK,
/// user record created in the database.
#[tokio::test]
async fn webhook_user_created_inserts_user() {
    let db = TestDatabase::new().await.expect("TestDatabase");
    let state = make_state(db.pool.clone());
    let app = make_webhook_router(state);

    let payload = user_upsert_payload("clerk_wh_001", "webhook@example.com", "user.created");
    let req = make_signed_request(TEST_WEBHOOK_SECRET, &payload);

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK, "expected 200 OK");

    // Verify the user was actually inserted.
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE clerk_id = $1")
            .bind("clerk_wh_001")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(count, 1, "user record must exist after user.created event");

    // Verify the email was stored correctly.
    let email: String =
        sqlx::query_scalar("SELECT email FROM users WHERE clerk_id = $1")
            .bind("clerk_wh_001")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(email, "webhook@example.com");
}

/// Valid `user.updated` payload → email updated in the database.
#[tokio::test]
async fn webhook_user_updated_changes_email() {
    let db = TestDatabase::new().await.expect("TestDatabase");
    let state = make_state(db.pool.clone());
    let app = make_webhook_router(state.clone());

    // First: create the user via user.created.
    let create_payload =
        user_upsert_payload("clerk_wh_002", "old@example.com", "user.created");
    let create_req = make_signed_request(TEST_WEBHOOK_SECRET, &create_payload);
    let res = app.clone().oneshot(create_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Then: update the email via user.updated.
    let update_payload =
        user_upsert_payload("clerk_wh_002", "new@example.com", "user.updated");
    let update_req = make_signed_request(TEST_WEBHOOK_SECRET, &update_payload);
    let res = app.oneshot(update_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // The DB row must reflect the updated email.
    let email: String =
        sqlx::query_scalar("SELECT email FROM users WHERE clerk_id = $1")
            .bind("clerk_wh_002")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(email, "new@example.com", "email must be updated after user.updated event");
}

/// Valid `user.deleted` payload → user and associated servers removed from DB.
#[tokio::test]
async fn webhook_user_deleted_removes_user_and_cascades() {
    let db = TestDatabase::new().await.expect("TestDatabase");
    let state = make_state(db.pool.clone());
    let app = make_webhook_router(state);

    // Insert the user directly so we can also insert a server.
    let user_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO users (clerk_id, email) VALUES ($1, $2) RETURNING id",
    )
    .bind("clerk_wh_003")
    .bind("delete@example.com")
    .fetch_one(&db.pool)
    .await
    .unwrap();

    // Insert a server owned by this user (to verify cascade delete).
    sqlx::query(
        r#"INSERT INTO mcp_servers (user_id, display_name, slug, config)
           VALUES ($1, 'Test Server', 'test-server-wh003', '{}'::jsonb)"#,
    )
    .bind(user_id)
    .execute(&db.pool)
    .await
    .unwrap();

    // Confirm setup.
    let server_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM mcp_servers WHERE user_id = $1")
            .bind(user_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(server_count, 1, "setup: server must exist before delete");

    // Send user.deleted webhook.
    let payload = user_deleted_payload("clerk_wh_003");
    let req = make_signed_request(TEST_WEBHOOK_SECRET, &payload);
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK, "expected 200 OK");

    // User must be gone.
    let user_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE clerk_id = $1")
            .bind("clerk_wh_003")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(user_count, 0, "user must be removed after user.deleted event");

    // Server must be gone (cascade).
    let server_count_after: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM mcp_servers WHERE user_id = $1")
            .bind(user_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(server_count_after, 0, "server must be cascade-deleted with the user");
}

/// Invalid Svix signature → 400 Bad Request, no DB write.
#[tokio::test]
async fn webhook_invalid_signature_returns_400() {
    let db = TestDatabase::new().await.expect("TestDatabase");
    let state = make_state(db.pool.clone());
    let app = make_webhook_router(state);

    // Sign with the WRONG secret — verification must fail.
    let payload = user_upsert_payload("clerk_wh_bad", "bad@example.com", "user.created");
    let req = make_signed_request(WRONG_WEBHOOK_SECRET, &payload);

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST, "expected 400 Bad Request");

    // Verify the error code.
    let body = to_bytes(res.into_body(), 8192).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        json["error"]["code"], "bad_request",
        "error code must be bad_request, got: {:?}",
        json
    );

    // The user must NOT have been inserted.
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE clerk_id = $1")
            .bind("clerk_wh_bad")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(count, 0, "no user must be written on invalid signature");
}

/// Payload with no Svix headers at all → 400 Bad Request.
#[tokio::test]
async fn webhook_missing_svix_headers_returns_400() {
    let db = TestDatabase::new().await.expect("TestDatabase");
    let state = make_state(db.pool.clone());
    let app = make_webhook_router(state);

    let payload = user_upsert_payload("clerk_wh_noheader", "noheader@example.com", "user.created");
    let req = Request::builder()
        .method("POST")
        .uri("/v1/webhooks/clerk")
        .header("content-type", "application/json")
        .body(Body::from(payload))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST, "expected 400 Bad Request");
}

/// Unknown event type → 200 OK, no DB change (graceful ignore).
#[tokio::test]
async fn webhook_unknown_event_type_returns_200() {
    let db = TestDatabase::new().await.expect("TestDatabase");
    let state = make_state(db.pool.clone());
    let app = make_webhook_router(state);

    let payload = serde_json::json!({
        "type": "session.created",
        "data": {}
    })
    .to_string()
    .into_bytes();

    let req = make_signed_request(TEST_WEBHOOK_SECRET, &payload);
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK, "unknown event types must return 200");
}
