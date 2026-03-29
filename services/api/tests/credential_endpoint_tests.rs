// Platform API — Credential and Token CRUD endpoint integration tests.
//
// Tests exercise the HTTP endpoints for credentials and server tokens,
// including:
//   - POST /v1/servers/{server_id}/credentials → 201 with CredentialMeta + hint
//   - GET  /v1/servers/{server_id}/credentials → list without sensitive fields
//   - PUT  /v1/servers/{server_id}/credentials/{id} → rotate credential
//   - DELETE /v1/servers/{server_id}/credentials/{id} → 204
//   - POST /v1/servers/{server_id}/tokens → 201 with raw token (once)
//   - GET  /v1/servers/{server_id}/tokens → list without token field
//   - DELETE /v1/servers/{server_id}/tokens/{id} → 204, sets revoked_at
//   - Ownership enforcement: another user's server returns 404
//
// Required environment variable:
//   TEST_DATABASE_URL=postgres://recast:recast@localhost:5432/postgres \
//     cargo test -p mcp-api --test credential_endpoint_tests
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    missing_docs
)]

mod helpers;

use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    response::IntoResponse,
    routing::get,
    Router,
};
use mcp_common::{
    middleware::request_id_middleware,
    testing::TestDatabase,
    AppError,
};
use tower::ServiceExt;
use uuid::Uuid;

use helpers::{make_jwt, make_state_with_jwks};
use mcp_api::{
    app_state::AppState,
    auth::clerk_jwt_middleware,
    handlers::{
        credentials::{
            create_credential_handler, delete_credential_handler,
            list_credentials_handler, rotate_credential_handler,
        },
        tokens::{create_token_handler, list_tokens_handler, revoke_token_handler},
        users::me_handler,
    },
    middleware::panic_handler,
};

/// Builds the full test router with auth middleware applied.
fn make_test_router(state: AppState) -> Router {
    let v1 = Router::new()
        .route("/v1/users/me", get(me_handler))
        .route(
            "/v1/servers/{server_id}/credentials",
            get(list_credentials_handler).post(create_credential_handler),
        )
        .route(
            "/v1/servers/{server_id}/credentials/{id}",
            axum::routing::put(rotate_credential_handler)
                .delete(delete_credential_handler),
        )
        .route(
            "/v1/servers/{server_id}/tokens",
            get(list_tokens_handler).post(create_token_handler),
        )
        .route(
            "/v1/servers/{server_id}/tokens/{id}",
            axum::routing::delete(revoke_token_handler),
        )
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            clerk_jwt_middleware,
        ));

    Router::new()
        .merge(v1)
        .fallback(|| async { AppError::NotFound("not found".to_string()).into_response() })
        .with_state(state)
        .layer(axum::middleware::from_fn(request_id_middleware))
        .layer(tower_http::catch_panic::CatchPanicLayer::custom(panic_handler))
}

// ── DB helpers ────────────────────────────────────────────────────────────────

/// Inserts a test user (using the Clerk sub as clerk_id) and returns the user UUID.
async fn insert_user(pool: &sqlx::PgPool, clerk_id: &str, email: &str) -> Uuid {
    sqlx::query("INSERT INTO users (clerk_id, email) VALUES ($1, $2) RETURNING id")
        .bind(clerk_id)
        .bind(email)
        .fetch_one(pool)
        .await
        .expect("insert user")
        .try_get_unchecked("id")
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
    .try_get_unchecked("id")
}

// Helper trait for easy column access in tests
trait RowExt {
    fn try_get_unchecked<T: for<'r> sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>>(
        &self,
        column: &str,
    ) -> T;
}

impl RowExt for sqlx::postgres::PgRow {
    fn try_get_unchecked<
        T: for<'r> sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
    >(
        &self,
        column: &str,
    ) -> T {
        use sqlx::Row;
        self.try_get(column).expect(column)
    }
}

// ── Credential endpoint tests ─────────────────────────────────────────────────

/// POST /v1/servers/{server_id}/credentials returns 201 with CredentialMeta and hint.
/// The raw value must not appear in the response body.
#[tokio::test]
async fn test_create_credential_returns_201_with_hint() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let (state, _mock) = make_state_with_jwks(db.pool.clone()).await;
    let app = make_test_router(state);

    let clerk_id = format!("user_{}", Uuid::new_v4().simple());
    let email = format!("{}@test.example.com", Uuid::new_v4().simple());
    let user_id = insert_user(&db.pool, &clerk_id, &email).await;
    let server_id = insert_server(&db.pool, user_id).await;

    let jwt = make_jwt(&clerk_id, &email);
    let body = serde_json::json!({
        "auth_type": "bearer",
        "key_name": null,
        "value": "super-secret-token-12345"
    });

    let req = Request::builder()
        .method("POST")
        .uri(format!("/v1/servers/{server_id}/credentials"))
        .header(header::AUTHORIZATION, format!("Bearer {jwt}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);

    let bytes = to_bytes(res.into_body(), 16384).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    // Must include hint, auth_type, id, server_id, created_at.
    assert_eq!(json["auth_type"], "bearer");
    assert_eq!(json["server_id"], server_id.to_string());
    assert!(json["hint"].is_string(), "hint must be present");
    assert!(json["id"].is_string(), "id must be present");

    // The raw value must NOT appear anywhere in the response.
    let body_str = std::str::from_utf8(&bytes).unwrap();
    assert!(
        !body_str.contains("super-secret-token-12345"),
        "raw credential value must not appear in response body"
    );

    // Hint should be a prefix of the value + asterisks.
    assert_eq!(json["hint"], "supe****");
}

/// POST with api_key_header credential type stores key_name correctly.
#[tokio::test]
async fn test_create_credential_with_key_name() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let (state, _mock) = make_state_with_jwks(db.pool.clone()).await;
    let app = make_test_router(state);

    let clerk_id = format!("user_{}", Uuid::new_v4().simple());
    let email = format!("{}@test.example.com", Uuid::new_v4().simple());
    let user_id = insert_user(&db.pool, &clerk_id, &email).await;
    let server_id = insert_server(&db.pool, user_id).await;

    let jwt = make_jwt(&clerk_id, &email);
    let body = serde_json::json!({
        "auth_type": "api_key_header",
        "key_name": "X-API-Key",
        "value": "my-api-key-value"
    });

    let req = Request::builder()
        .method("POST")
        .uri(format!("/v1/servers/{server_id}/credentials"))
        .header(header::AUTHORIZATION, format!("Bearer {jwt}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);

    let bytes = to_bytes(res.into_body(), 16384).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(json["auth_type"], "api_key_header");
    assert_eq!(json["key_name"], "X-API-Key");
}

/// GET /v1/servers/{server_id}/credentials returns list without sensitive fields.
#[tokio::test]
async fn test_list_credentials_no_sensitive_fields() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let (state, _mock) = make_state_with_jwks(db.pool.clone()).await;

    let clerk_id = format!("user_{}", Uuid::new_v4().simple());
    let email = format!("{}@test.example.com", Uuid::new_v4().simple());
    let user_id = insert_user(&db.pool, &clerk_id, &email).await;
    let server_id = insert_server(&db.pool, user_id).await;

    // Pre-create a credential.
    state
        .credential_service
        .store(
            server_id,
            "bearer",
            None,
            zeroize::Zeroizing::new("secret-token".to_string()),
            Some(user_id),
        )
        .await
        .expect("store credential");

    let app = make_test_router(state);
    let jwt = make_jwt(&clerk_id, &email);

    let req = Request::builder()
        .method("GET")
        .uri(format!("/v1/servers/{server_id}/credentials"))
        .header(header::AUTHORIZATION, format!("Bearer {jwt}"))
        .body(Body::empty())
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let bytes = to_bytes(res.into_body(), 16384).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let body_str = std::str::from_utf8(&bytes).unwrap();

    // Must have credentials array.
    assert!(json["credentials"].is_array());
    assert_eq!(json["credentials"].as_array().unwrap().len(), 1);

    // Sensitive fields must not appear.
    assert!(!body_str.contains("encrypted_payload"), "encrypted_payload must not appear");
    assert!(!body_str.contains("encrypted_value"), "encrypted_value must not appear");
    assert!(!body_str.contains("iv"), "iv field must not appear in key position");
    assert!(!body_str.contains("secret-token"), "raw value must not appear");
}

/// PUT /v1/servers/{server_id}/credentials/{id} rotates the credential.
#[tokio::test]
async fn test_rotate_credential_returns_updated_meta() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let (state, _mock) = make_state_with_jwks(db.pool.clone()).await;

    let clerk_id = format!("user_{}", Uuid::new_v4().simple());
    let email = format!("{}@test.example.com", Uuid::new_v4().simple());
    let user_id = insert_user(&db.pool, &clerk_id, &email).await;
    let server_id = insert_server(&db.pool, user_id).await;

    let meta = state
        .credential_service
        .store(
            server_id,
            "bearer",
            None,
            zeroize::Zeroizing::new("old-token".to_string()),
            Some(user_id),
        )
        .await
        .expect("store");

    let app = make_test_router(state);
    let jwt = make_jwt(&clerk_id, &email);
    let cred_id = meta.id;

    let body = serde_json::json!({ "value": "new-rotated-token" });

    let req = Request::builder()
        .method("PUT")
        .uri(format!("/v1/servers/{server_id}/credentials/{cred_id}"))
        .header(header::AUTHORIZATION, format!("Bearer {jwt}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let bytes = to_bytes(res.into_body(), 16384).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let body_str = std::str::from_utf8(&bytes).unwrap();

    assert_eq!(json["id"], cred_id.to_string());
    assert_eq!(json["server_id"], server_id.to_string());

    // Raw new value must not appear in response.
    assert!(!body_str.contains("new-rotated-token"), "raw value must not appear in response");
}

/// DELETE /v1/servers/{server_id}/credentials/{id} returns 204.
#[tokio::test]
async fn test_delete_credential_returns_204() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let (state, _mock) = make_state_with_jwks(db.pool.clone()).await;

    let clerk_id = format!("user_{}", Uuid::new_v4().simple());
    let email = format!("{}@test.example.com", Uuid::new_v4().simple());
    let user_id = insert_user(&db.pool, &clerk_id, &email).await;
    let server_id = insert_server(&db.pool, user_id).await;

    let meta = state
        .credential_service
        .store(
            server_id,
            "bearer",
            None,
            zeroize::Zeroizing::new("delete-me".to_string()),
            Some(user_id),
        )
        .await
        .expect("store");

    let app = make_test_router(state.clone());
    let jwt = make_jwt(&clerk_id, &email);
    let cred_id = meta.id;

    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/v1/servers/{server_id}/credentials/{cred_id}"))
        .header(header::AUTHORIZATION, format!("Bearer {jwt}"))
        .body(Body::empty())
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    // Verify DB row is gone.
    let gone = sqlx::query("SELECT id FROM credentials WHERE id = $1")
        .bind(cred_id)
        .fetch_optional(&db.pool)
        .await
        .unwrap();
    assert!(gone.is_none(), "credential must be deleted from DB");
}

/// Accessing another user's server's credentials returns 404.
#[tokio::test]
async fn test_credential_access_other_users_server_returns_404() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let (state, _mock) = make_state_with_jwks(db.pool.clone()).await;

    // User A owns the server.
    let clerk_a = format!("user_a_{}", Uuid::new_v4().simple());
    let email_a = format!("a_{}@test.example.com", Uuid::new_v4().simple());
    let user_a = insert_user(&db.pool, &clerk_a, &email_a).await;
    let server_id = insert_server(&db.pool, user_a).await;

    // User B tries to access User A's server.
    let clerk_b = format!("user_b_{}", Uuid::new_v4().simple());
    let email_b = format!("b_{}@test.example.com", Uuid::new_v4().simple());
    insert_user(&db.pool, &clerk_b, &email_b).await;

    let app = make_test_router(state);
    let jwt_b = make_jwt(&clerk_b, &email_b);

    let req = Request::builder()
        .method("GET")
        .uri(format!("/v1/servers/{server_id}/credentials"))
        .header(header::AUTHORIZATION, format!("Bearer {jwt_b}"))
        .body(Body::empty())
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

// ── Token endpoint tests ──────────────────────────────────────────────────────

/// POST /v1/servers/{server_id}/tokens returns 201 with raw token (starts with mcp_live_).
#[tokio::test]
async fn test_create_token_returns_201_with_raw_token() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let (state, _mock) = make_state_with_jwks(db.pool.clone()).await;
    let app = make_test_router(state);

    let clerk_id = format!("user_{}", Uuid::new_v4().simple());
    let email = format!("{}@test.example.com", Uuid::new_v4().simple());
    let user_id = insert_user(&db.pool, &clerk_id, &email).await;
    let server_id = insert_server(&db.pool, user_id).await;

    let jwt = make_jwt(&clerk_id, &email);
    let body = serde_json::json!({ "description": "My test token" });

    let req = Request::builder()
        .method("POST")
        .uri(format!("/v1/servers/{server_id}/tokens"))
        .header(header::AUTHORIZATION, format!("Bearer {jwt}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);

    let bytes = to_bytes(res.into_body(), 16384).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    // Token must be present and start with mcp_live_.
    let token = json["token"].as_str().expect("token field must be present");
    assert!(
        token.starts_with("mcp_live_"),
        "token must start with mcp_live_; got: {token}"
    );
    assert_eq!(token.len(), 73, "mcp_live_ (9) + 64 hex chars = 73");

    // Hint must be present.
    assert!(json["hint"].is_string(), "hint must be present");
    let hint = json["hint"].as_str().unwrap();
    assert!(
        hint.starts_with("mcp_live_"),
        "hint must start with mcp_live_; got: {hint}"
    );
    assert!(hint.ends_with("****"), "hint must end with ****");

    // Other fields.
    assert_eq!(json["server_id"], server_id.to_string());
    assert_eq!(json["description"], "My test token");
    assert_eq!(json["is_active"], true);

    // Verify the token_hash is stored (not the raw token) in the DB.
    use sha2::{Digest, Sha256};
    use sqlx::Row;
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let expected_hash: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();

    let row = sqlx::query("SELECT token_hash FROM server_tokens WHERE id = $1")
        .bind(json["id"].as_str().and_then(|s| s.parse::<Uuid>().ok()).unwrap())
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let stored_hash: String = row.try_get("token_hash").unwrap();
    assert_eq!(stored_hash, expected_hash, "DB must store SHA-256 hash, not raw token");
    assert_ne!(stored_hash, token, "raw token must not be stored in DB");
}

/// GET /v1/servers/{server_id}/tokens returns list without token field.
#[tokio::test]
async fn test_list_tokens_no_token_field() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let (state, _mock) = make_state_with_jwks(db.pool.clone()).await;
    let app = make_test_router(state);

    let clerk_id = format!("user_{}", Uuid::new_v4().simple());
    let email = format!("{}@test.example.com", Uuid::new_v4().simple());
    let user_id = insert_user(&db.pool, &clerk_id, &email).await;
    let server_id = insert_server(&db.pool, user_id).await;

    let jwt = make_jwt(&clerk_id, &email);

    // Create a token first.
    let create_req = Request::builder()
        .method("POST")
        .uri(format!("/v1/servers/{server_id}/tokens"))
        .header(header::AUTHORIZATION, format!("Bearer {jwt}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"description":"list-test"}"#))
        .unwrap();
    let create_res = app.clone().oneshot(create_req).await.unwrap();
    assert_eq!(create_res.status(), StatusCode::CREATED);
    let create_bytes = to_bytes(create_res.into_body(), 16384).await.unwrap();
    let create_json: serde_json::Value = serde_json::from_slice(&create_bytes).unwrap();
    let raw_token = create_json["token"].as_str().unwrap().to_string();

    // Now list tokens.
    let list_req = Request::builder()
        .method("GET")
        .uri(format!("/v1/servers/{server_id}/tokens"))
        .header(header::AUTHORIZATION, format!("Bearer {jwt}"))
        .body(Body::empty())
        .unwrap();

    let list_res = app.oneshot(list_req).await.unwrap();
    assert_eq!(list_res.status(), StatusCode::OK);

    let bytes = to_bytes(list_res.into_body(), 16384).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let body_str = std::str::from_utf8(&bytes).unwrap();

    // Must have tokens array.
    assert!(json["tokens"].is_array());
    assert_eq!(json["tokens"].as_array().unwrap().len(), 1);

    // The raw token must NOT appear in the list response.
    assert!(
        !body_str.contains(&raw_token),
        "raw token must not appear in list response"
    );

    // Each token entry must have hint but no token field.
    let token_entry = &json["tokens"][0];
    assert!(
        token_entry.get("token").is_none(),
        "list response must not include token field"
    );
    assert!(token_entry["hint"].is_string(), "hint must be present in list");
    assert!(token_entry["is_active"].is_boolean());
}

/// DELETE /v1/servers/{server_id}/tokens/{id} returns 204 and sets revoked_at.
#[tokio::test]
async fn test_revoke_token_returns_204_and_sets_revoked_at() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let (state, _mock) = make_state_with_jwks(db.pool.clone()).await;
    let app = make_test_router(state);

    let clerk_id = format!("user_{}", Uuid::new_v4().simple());
    let email = format!("{}@test.example.com", Uuid::new_v4().simple());
    let user_id = insert_user(&db.pool, &clerk_id, &email).await;
    let server_id = insert_server(&db.pool, user_id).await;

    let jwt = make_jwt(&clerk_id, &email);

    // Create a token.
    let create_req = Request::builder()
        .method("POST")
        .uri(format!("/v1/servers/{server_id}/tokens"))
        .header(header::AUTHORIZATION, format!("Bearer {jwt}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"description":"revoke-test"}"#))
        .unwrap();
    let create_res = app.clone().oneshot(create_req).await.unwrap();
    assert_eq!(create_res.status(), StatusCode::CREATED);

    let create_bytes = to_bytes(create_res.into_body(), 16384).await.unwrap();
    let create_json: serde_json::Value = serde_json::from_slice(&create_bytes).unwrap();
    let token_id: Uuid = create_json["id"].as_str().unwrap().parse().unwrap();

    // Revoke the token.
    let revoke_req = Request::builder()
        .method("DELETE")
        .uri(format!("/v1/servers/{server_id}/tokens/{token_id}"))
        .header(header::AUTHORIZATION, format!("Bearer {jwt}"))
        .body(Body::empty())
        .unwrap();

    let revoke_res = app.oneshot(revoke_req).await.unwrap();
    assert_eq!(revoke_res.status(), StatusCode::NO_CONTENT);

    // Verify DB state: is_active = false, revoked_at is set.
    use sqlx::Row;
    let row = sqlx::query(
        "SELECT is_active, revoked_at FROM server_tokens WHERE id = $1",
    )
    .bind(token_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();

    let is_active: bool = row.try_get("is_active").unwrap();
    let revoked_at: Option<chrono::DateTime<chrono::Utc>> = row.try_get("revoked_at").unwrap();

    assert!(!is_active, "token must be inactive after revocation");
    assert!(revoked_at.is_some(), "revoked_at must be set after revocation");
}

/// Revoking a token on another user's server returns 404.
#[tokio::test]
async fn test_revoke_token_other_users_server_returns_404() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let (state, _mock) = make_state_with_jwks(db.pool.clone()).await;

    // User A owns the server and token.
    let clerk_a = format!("user_a_{}", Uuid::new_v4().simple());
    let email_a = format!("a_{}@test.example.com", Uuid::new_v4().simple());
    let user_a = insert_user(&db.pool, &clerk_a, &email_a).await;
    let server_id = insert_server(&db.pool, user_a).await;

    // Insert a token directly for user A.
    use sqlx::Row;
    let token_id: Uuid = sqlx::query(
        "INSERT INTO server_tokens (server_id, token_hash) VALUES ($1, $2) RETURNING id",
    )
    .bind(server_id)
    .bind("deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef")
    .fetch_one(&db.pool)
    .await
    .unwrap()
    .try_get("id")
    .unwrap();

    // User B tries to revoke user A's token.
    let clerk_b = format!("user_b_{}", Uuid::new_v4().simple());
    let email_b = format!("b_{}@test.example.com", Uuid::new_v4().simple());
    insert_user(&db.pool, &clerk_b, &email_b).await;

    let app = make_test_router(state);
    let jwt_b = make_jwt(&clerk_b, &email_b);

    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/v1/servers/{server_id}/tokens/{token_id}"))
        .header(header::AUTHORIZATION, format!("Bearer {jwt_b}"))
        .body(Body::empty())
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}
