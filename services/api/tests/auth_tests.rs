// Platform API — Clerk JWT authentication integration tests.
//
// Tests in this file exercise the full auth middleware stack including:
//   - JWT validation (signature, exp, nbf, iss)
//   - JWKS cache behaviour (cache hit / network call count)
//   - User upsert in PostgreSQL
//   - GET /v1/users/me endpoint
//
// Required environment variable (or TEST_DATABASE_URL):
//   TEST_DATABASE_URL=postgres://recast:recast@localhost:5432/postgres \
//     cargo test -p mcp-api --test auth_tests
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    missing_docs
)]

mod helpers;

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    routing::get,
    Router,
};
use mcp_common::{
    middleware::request_id_middleware,
    testing::{MockUpstream, TestDatabase},
    AuditLogger,
};
use std::sync::Arc;
use tower::ServiceExt;

use helpers::{make_jwt_with_offset, test_key, TEST_ISSUER};
use mcp_api::{
    auth::{clerk_jwt_middleware, JwksCache},
    app_state::AppState,
    config::ApiConfig,
    credentials::CredentialService,
    handlers::users::me_handler,
    middleware::panic_handler,
    servers::ServerService,
};
use mcp_common::AppError;
use mcp_crypto::CryptoKey;

// ── Test state / router builder ───────────────────────────────────────────────

/// Builds an AppState that uses the given pool and points JWKS at `jwks_url`.
fn make_state(pool: sqlx::PgPool, jwks_url: &str) -> AppState {
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
            clerk_jwks_url: jwks_url.to_string(),
            // Valid 32-byte base64 test secret (not used in auth tests).
            clerk_webhook_secret: "whsec_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
                .to_string(),
            encryption_key: "0".repeat(64),
            clerk_issuer: TEST_ISSUER.to_string(),
            cors_origins: vec![],
            gateway_base_url: "https://mcp.test.example.com".to_string(),
        }),
        audit_logger,
        jwks_cache: JwksCache::new(jwks_url),
        credential_service,
        server_service,
    }
}

/// Builds a minimal test router with real JWT auth and the /v1/users/me route.
fn make_auth_router(state: AppState) -> Router {
    let v1 = Router::new()
        .route("/v1/users/me", get(me_handler))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            clerk_jwt_middleware,
        ));

    Router::new()
        .merge(v1)
        .fallback(|| async {
            AppError::NotFound("not found".to_string()).into_response()
        })
        .with_state(state)
        .layer(axum::middleware::from_fn(request_id_middleware))
        .layer(tower_http::catch_panic::CatchPanicLayer::custom(
            panic_handler,
        ))
}

use axum::response::IntoResponse;

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Valid JWT → 200 OK with {id, email, created_at}.
#[tokio::test]
async fn auth_valid_jwt_returns_user_info() {
    let db = TestDatabase::new().await.expect("TestDatabase");
    let mock = MockUpstream::start().await;
    let key = test_key();
    mock.set_response_body(serde_json::from_str(&key.jwks_json).unwrap());

    let state = make_state(db.pool.clone(), &format!("{}/jwks", mock.url()));
    let app = make_auth_router(state);

    let token = make_jwt_with_offset("user_clerk_001", "alice@example.com", 3600);
    let req = Request::builder()
        .uri("/v1/users/me")
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK, "expected 200");

    let body = to_bytes(res.into_body(), 8192).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert!(json.get("id").is_some(), "response must have id");
    assert_eq!(json["email"], "alice@example.com");
    assert!(json.get("created_at").is_some(), "response must have created_at");
}

/// Missing Authorization header → 401 UNAUTHORIZED.
#[tokio::test]
async fn auth_missing_header_returns_401_unauthorized() {
    let db = TestDatabase::new().await.expect("TestDatabase");
    let mock = MockUpstream::start().await;
    let key = test_key();
    mock.set_response_body(serde_json::from_str(&key.jwks_json).unwrap());

    let state = make_state(db.pool.clone(), &format!("{}/jwks", mock.url()));
    let app = make_auth_router(state);

    let req = Request::builder()
        .uri("/v1/users/me")
        .body(Body::empty())
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "expected 401");

    let body = to_bytes(res.into_body(), 8192).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["code"], "unauthorized");
}

/// Expired JWT → 401 TOKEN_EXPIRED.
#[tokio::test]
async fn auth_expired_jwt_returns_401_token_expired() {
    let db = TestDatabase::new().await.expect("TestDatabase");
    let mock = MockUpstream::start().await;
    let key = test_key();
    mock.set_response_body(serde_json::from_str(&key.jwks_json).unwrap());

    let state = make_state(db.pool.clone(), &format!("{}/jwks", mock.url()));
    let app = make_auth_router(state);

    // exp set 3600 seconds in the past → expired
    let token = make_jwt_with_offset("user_clerk_002", "bob@example.com", -3600);
    let req = Request::builder()
        .uri("/v1/users/me")
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "expected 401");

    let body = to_bytes(res.into_body(), 8192).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        json["error"]["code"], "token_expired",
        "error code must be token_expired, got: {:?}",
        json["error"]["code"]
    );
}

/// Tampered JWT (corrupted signature) → 401 UNAUTHORIZED.
#[tokio::test]
async fn auth_tampered_jwt_returns_401_unauthorized() {
    let db = TestDatabase::new().await.expect("TestDatabase");
    let mock = MockUpstream::start().await;
    let key = test_key();
    mock.set_response_body(serde_json::from_str(&key.jwks_json).unwrap());

    let state = make_state(db.pool.clone(), &format!("{}/jwks", mock.url()));
    let app = make_auth_router(state);

    let token = make_jwt_with_offset("user_clerk_003", "carol@example.com", 3600);
    // Corrupt the last few characters of the signature (third JWT segment).
    let parts: Vec<&str> = token.splitn(3, '.').collect();
    assert_eq!(parts.len(), 3);
    let tampered = format!("{}.{}.{}XXXX", parts[0], parts[1], parts[2]);

    let req = Request::builder()
        .uri("/v1/users/me")
        .header("Authorization", format!("Bearer {tampered}"))
        .body(Body::empty())
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "expected 401");

    let body = to_bytes(res.into_body(), 8192).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["code"], "unauthorized");
}

/// Two requests with the same kid → MockUpstream receives exactly 1 JWKS request.
#[tokio::test]
async fn auth_jwks_cache_hit_no_second_network_call() {
    let db = TestDatabase::new().await.expect("TestDatabase");
    let mock = MockUpstream::start().await;
    let key = test_key();
    mock.set_response_body(serde_json::from_str(&key.jwks_json).unwrap());

    let jwks_url = format!("{}/jwks", mock.url());
    let state = make_state(db.pool.clone(), &jwks_url);
    let app = make_auth_router(state);

    let token = make_jwt_with_offset("user_clerk_004", "dave@example.com", 3600);

    // First request — triggers a JWKS fetch.
    let req1 = Request::builder()
        .uri("/v1/users/me")
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let res1 = app.clone().oneshot(req1).await.unwrap();
    assert_eq!(res1.status(), StatusCode::OK);

    // Second request — should hit the cache, no new network call.
    let token2 = make_jwt_with_offset("user_clerk_004", "dave@example.com", 3600);
    let req2 = Request::builder()
        .uri("/v1/users/me")
        .header("Authorization", format!("Bearer {token2}"))
        .body(Body::empty())
        .unwrap();
    let res2 = app.oneshot(req2).await.unwrap();
    assert_eq!(res2.status(), StatusCode::OK);

    // JWKS endpoint should have been called exactly once.
    let requests = mock.received_requests();
    let jwks_calls: Vec<_> = requests.iter().filter(|r| r.path == "/jwks").collect();
    assert_eq!(
        jwks_calls.len(),
        1,
        "JWKS endpoint must be called exactly once (cache hit on second request). \
         Got {} call(s).",
        jwks_calls.len()
    );
}

/// Valid JWT upserts the user; second login with same clerk_id but different
/// email updates the email in the users table.
#[tokio::test]
async fn auth_upsert_updates_email_on_second_login() {
    let db = TestDatabase::new().await.expect("TestDatabase");
    let mock = MockUpstream::start().await;
    let key = test_key();
    mock.set_response_body(serde_json::from_str(&key.jwks_json).unwrap());

    let jwks_url = format!("{}/jwks", mock.url());
    let state1 = make_state(db.pool.clone(), &jwks_url);
    let app1 = make_auth_router(state1);

    // First login — creates user record.
    let token1 = make_jwt_with_offset("user_clerk_005", "old@example.com", 3600);
    let req1 = Request::builder()
        .uri("/v1/users/me")
        .header("Authorization", format!("Bearer {token1}"))
        .body(Body::empty())
        .unwrap();
    let res1 = app1.oneshot(req1).await.unwrap();
    assert_eq!(res1.status(), StatusCode::OK);

    // Second login — same clerk_id, new email (simulates Clerk email change).
    let state2 = make_state(db.pool.clone(), &jwks_url);
    let app2 = make_auth_router(state2);
    let token2 = make_jwt_with_offset("user_clerk_005", "new@example.com", 3600);
    let req2 = Request::builder()
        .uri("/v1/users/me")
        .header("Authorization", format!("Bearer {token2}"))
        .body(Body::empty())
        .unwrap();
    let res2 = app2.oneshot(req2).await.unwrap();
    assert_eq!(res2.status(), StatusCode::OK);

    // Verify the DB row has the updated email.
    let row: (String,) = sqlx::query_as(
        "SELECT email FROM users WHERE clerk_id = $1",
    )
    .bind("user_clerk_005")
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(row.0, "new@example.com");
}

/// GET /v1/users/me with no Authorization header returns 401 UNAUTHORIZED.
/// This is an alias of auth_missing_header to match acceptance criterion phrasing.
#[tokio::test]
async fn me_no_header_returns_401() {
    let db = TestDatabase::new().await.expect("TestDatabase");
    let mock = MockUpstream::start().await;
    let key = test_key();
    mock.set_response_body(serde_json::from_str(&key.jwks_json).unwrap());

    let state = make_state(db.pool.clone(), &format!("{}/jwks", mock.url()));
    let app = make_auth_router(state);

    let req = Request::builder()
        .uri("/v1/users/me")
        .body(Body::empty())
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let body = to_bytes(res.into_body(), 8192).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["code"], "unauthorized");
}
