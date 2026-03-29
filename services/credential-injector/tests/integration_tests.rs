// Credential Injector — integration tests.
//
// Tests cover the full POST /inject flow:
//   - Bearer token injection: MockUpstream receives correct Authorization header
//   - api_key_header injection: custom header forwarded
//   - api_key_query injection: query param appended to URL
//   - basic auth injection: Authorization: Basic <base64> forwarded
//   - Wrong shared secret → 403 Forbidden
//   - No credential for server_id → 404 credential_not_found
//   - SSRF blocked URL → 422 ssrf_blocked
//   - Upstream timeout → 504 upstream_timeout
//   - NOTIFY eviction: cache entry evicted on pg_notify
//
// NOTE: "Decrypted credential never appears in logs" is enforced by code review
// and the Zeroizing<Vec<u8>> wrapper that zeroes memory on drop. There are no
// explicit log statements in inject.rs that format or write the plaintext value.
//
// Required environment variable:
//   TEST_DATABASE_URL=postgres://recast:recast@localhost:5432/postgres \
//     cargo test -p mcp-credential-injector --test integration_tests
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    missing_docs
)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use mcp_common::testing::{MockUpstream, TestDatabase};
use mcp_common::AuditLogger;
use sqlx::Row;
use mcp_credential_injector::{
    build_app_state, build_inject_router,
    cache::new_cache,
    inject::RequestSkeleton,
};
use mcp_crypto::{encrypt, CryptoKey};
use uuid::Uuid;

// ── Test helpers ─────────────────────────────────────────────────────────────

const TEST_SECRET: &str = "test-shared-secret-1234";
const TEST_KEY_HEX: &str = "4242424242424242424242424242424242424242424242424242424242424242";

/// Starts a real axum server on a random port and returns the base URL.
///
/// The server uses `into_make_service_with_connect_info` so that
/// `ConnectInfo<SocketAddr>` works in the inject handler.
///
/// `skip_ssrf` must be `true` for tests that use `MockUpstream` (which binds
/// to `127.0.0.1`, a loopback address blocked by the production SSRF rules).
/// Pass `false` for tests that verify SSRF rejection.
async fn start_test_server(db: &TestDatabase, skip_ssrf: bool) -> String {
    let crypto_key = Arc::new(CryptoKey::from_hex(TEST_KEY_HEX).expect("test key"));
    let audit_logger = AuditLogger::new(db.pool.clone());
    let cache = Arc::new(new_cache());

    let mut state = build_app_state(
        db.pool.clone(),
        crypto_key,
        audit_logger,
        cache,
        vec!["127.0.0.1".parse().unwrap()],
        TEST_SECRET.to_string(),
        Duration::from_secs(5),
    )
    .expect("build_app_state");

    // Override for tests that point the injector at MockUpstream on localhost.
    state.skip_ssrf = skip_ssrf;

    let router = build_inject_router(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .ok();
    });

    format!("http://127.0.0.1:{}", addr.port())
}

/// Encrypts `plaintext` with the test key (same key used by `start_test_server`).
fn encrypt_test_credential(plaintext: &str) -> Vec<u8> {
    let key = CryptoKey::from_hex(TEST_KEY_HEX).expect("test key");
    encrypt(&key, plaintext.as_bytes()).expect("encrypt")
}

/// Inserts a test MCP server record and returns its id.
async fn insert_test_server(db: &TestDatabase) -> Uuid {
    let row = sqlx::query(
        "INSERT INTO mcp_servers (user_id, name, slug, status, config_json)
         VALUES (
             (SELECT id FROM users LIMIT 1),
             'Test Server',
             gen_random_uuid()::text,
             'active',
             '{}'
         )
         RETURNING id",
    )
    .fetch_one(&db.pool)
    .await
    .expect("insert server");

    row.try_get("id").expect("id")
}

/// Inserts a credential with the given auth_type and plaintext.
async fn insert_credential(
    db: &TestDatabase,
    server_id: Uuid,
    auth_type: &str,
    key_name: Option<&str>,
    plaintext: &str,
) -> Uuid {
    let encrypted = encrypt_test_credential(plaintext);
    let iv = encrypted[..12].to_vec();
    let hint = format!("{}****", &plaintext[..plaintext.len().min(4)]);

    let row = sqlx::query(
        "INSERT INTO credentials (server_id, auth_type, key_name, encrypted_payload, iv, hint)
         VALUES ($1, $2, $3, $4, $5, $6)
         RETURNING id",
    )
    .bind(server_id)
    .bind(auth_type)
    .bind(key_name)
    .bind(&encrypted[..])
    .bind(&iv[..])
    .bind(&hint)
    .fetch_one(&db.pool)
    .await
    .expect("insert credential");

    row.try_get("id").expect("id")
}

/// Inserts a test user record and returns its id.
async fn ensure_test_user(db: &TestDatabase) -> Uuid {
    let row = sqlx::query(
        "INSERT INTO users (clerk_id, email) VALUES ('test_clerk_id', 'test@example.com')
         ON CONFLICT (clerk_id) DO UPDATE SET email = EXCLUDED.email
         RETURNING id",
    )
    .fetch_one(&db.pool)
    .await
    .expect("upsert user");

    row.try_get("id").expect("id")
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_bearer_token_injection() {
    let db = TestDatabase::new().await.expect("test db");
    ensure_test_user(&db).await;
    let server_id = insert_test_server(&db).await;
    let token = "supersecrettoken";
    insert_credential(&db, server_id, "bearer", None, token).await;

    let mock = MockUpstream::start().await;
    // skip_ssrf=true: MockUpstream binds to 127.0.0.1 which is SSRF-blocked
    let base_url = start_test_server(&db, true).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base_url}/inject"))
        .bearer_auth(TEST_SECRET)
        .json(&RequestSkeleton {
            server_id,
            method: "GET".to_string(),
            url: format!("{}/api/resource", mock.url()),
            headers: std::collections::HashMap::new(),
            body: None,
        })
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status().as_u16(), 200);

    // Verify the upstream received the correct Authorization header.
    mock.assert_received_header("authorization", &format!("Bearer {token}"));
}

#[tokio::test]
async fn test_api_key_header_injection() {
    let db = TestDatabase::new().await.expect("test db");
    ensure_test_user(&db).await;
    let server_id = insert_test_server(&db).await;
    let api_key = "my-api-key-value";
    insert_credential(&db, server_id, "api_key_header", Some("X-Custom-Key"), api_key).await;

    let mock = MockUpstream::start().await;
    let base_url = start_test_server(&db, true).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base_url}/inject"))
        .bearer_auth(TEST_SECRET)
        .json(&RequestSkeleton {
            server_id,
            method: "POST".to_string(),
            url: format!("{}/api/resource", mock.url()),
            headers: std::collections::HashMap::new(),
            body: Some(serde_json::json!({"data": "value"})),
        })
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status().as_u16(), 200);
    mock.assert_received_header("x-custom-key", api_key);
}

#[tokio::test]
async fn test_api_key_query_injection() {
    let db = TestDatabase::new().await.expect("test db");
    ensure_test_user(&db).await;
    let server_id = insert_test_server(&db).await;
    let api_key = "query-api-key";
    insert_credential(&db, server_id, "api_key_query", Some("token"), api_key).await;

    let mock = MockUpstream::start().await;
    let base_url = start_test_server(&db, true).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base_url}/inject"))
        .bearer_auth(TEST_SECRET)
        .json(&RequestSkeleton {
            server_id,
            method: "GET".to_string(),
            url: format!("{}/api/data", mock.url()),
            headers: std::collections::HashMap::new(),
            body: None,
        })
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status().as_u16(), 200);

    // Verify the upstream received the request with the query parameter.
    let requests = mock.received_requests();
    assert!(!requests.is_empty(), "upstream should have received a request");
    assert!(
        requests[0].path.contains("token=query-api-key"),
        "query param should contain the API key; path = {}",
        requests[0].path
    );
}

#[tokio::test]
async fn test_basic_auth_injection() {
    let db = TestDatabase::new().await.expect("test db");
    ensure_test_user(&db).await;
    let server_id = insert_test_server(&db).await;
    let credentials = "user:password123";
    insert_credential(&db, server_id, "basic", None, credentials).await;

    let mock = MockUpstream::start().await;
    let base_url = start_test_server(&db, true).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base_url}/inject"))
        .bearer_auth(TEST_SECRET)
        .json(&RequestSkeleton {
            server_id,
            method: "GET".to_string(),
            url: format!("{}/api/resource", mock.url()),
            headers: std::collections::HashMap::new(),
            body: None,
        })
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status().as_u16(), 200);

    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let expected = format!("Basic {}", STANDARD.encode(credentials.as_bytes()));
    mock.assert_received_header("authorization", &expected);
}

#[tokio::test]
async fn test_wrong_shared_secret_returns_403() {
    let db = TestDatabase::new().await.expect("test db");
    ensure_test_user(&db).await;
    let server_id = insert_test_server(&db).await;
    insert_credential(&db, server_id, "bearer", None, "token").await;

    let base_url = start_test_server(&db, false).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base_url}/inject"))
        .bearer_auth("wrong-secret")
        .json(&RequestSkeleton {
            server_id,
            method: "GET".to_string(),
            url: "https://api.stripe.com/v1/test".to_string(),
            headers: std::collections::HashMap::new(),
            body: None,
        })
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status().as_u16(), 403);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["error"]["code"], "forbidden");
}

#[tokio::test]
async fn test_no_credential_returns_404() {
    let db = TestDatabase::new().await.expect("test db");
    ensure_test_user(&db).await;
    let server_id = insert_test_server(&db).await;
    // No credential inserted for this server.

    let base_url = start_test_server(&db, false).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base_url}/inject"))
        .bearer_auth(TEST_SECRET)
        .json(&RequestSkeleton {
            server_id,
            method: "GET".to_string(),
            url: "https://api.stripe.com/v1/test".to_string(),
            headers: std::collections::HashMap::new(),
            body: None,
        })
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status().as_u16(), 404);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["error"]["code"], "credential_not_found");
}

#[tokio::test]
async fn test_ssrf_blocked_url_returns_422() {
    let db = TestDatabase::new().await.expect("test db");
    ensure_test_user(&db).await;
    let server_id = insert_test_server(&db).await;
    insert_credential(&db, server_id, "bearer", None, "token").await;

    // skip_ssrf=false: this test specifically verifies the SSRF check fires
    let base_url = start_test_server(&db, false).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base_url}/inject"))
        .bearer_auth(TEST_SECRET)
        .json(&RequestSkeleton {
            server_id,
            method: "GET".to_string(),
            // Private IP — blocked by SSRF Phase 1
            url: "http://192.168.1.1/api".to_string(),
            headers: std::collections::HashMap::new(),
            body: None,
        })
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status().as_u16(), 422);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["error"]["code"], "ssrf_blocked");
}

#[tokio::test]
async fn test_upstream_timeout_returns_504() {
    let db = TestDatabase::new().await.expect("test db");
    ensure_test_user(&db).await;
    let server_id = insert_test_server(&db).await;
    insert_credential(&db, server_id, "bearer", None, "token").await;

    // Start a server that never responds.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let timeout_addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        // Accept connections but never write a response — simulates timeout.
        loop {
            if let Ok((_stream, _)) = listener.accept().await {
                // Hold the connection open forever.
                tokio::time::sleep(Duration::from_secs(3600)).await;
            }
        }
    });

    // Build a state with a very short upstream timeout (100 ms).
    let crypto_key = Arc::new(CryptoKey::from_hex(TEST_KEY_HEX).expect("test key"));
    let audit_logger = AuditLogger::new(db.pool.clone());
    let cache = Arc::new(new_cache());

    let mut state = build_app_state(
        db.pool.clone(),
        crypto_key,
        audit_logger,
        cache,
        vec!["127.0.0.1".parse().unwrap()],
        TEST_SECRET.to_string(),
        Duration::from_millis(200), // very short timeout
    )
    .expect("build_app_state");
    // The timeout server is on 127.0.0.1 — bypass SSRF for this test.
    state.skip_ssrf = true;

    let router = build_inject_router(state);
    let l2 = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let svc_addr = l2.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            l2,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .ok();
    });

    let base_url = format!("http://127.0.0.1:{}", svc_addr.port());
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base_url}/inject"))
        .bearer_auth(TEST_SECRET)
        .json(&RequestSkeleton {
            server_id,
            method: "GET".to_string(),
            url: format!("http://127.0.0.1:{}/api", timeout_addr.port()),
            headers: std::collections::HashMap::new(),
            body: None,
        })
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status().as_u16(), 504);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["error"]["code"], "upstream_timeout");
}

#[tokio::test]
async fn test_cache_eviction_on_notify() {
    let db = TestDatabase::new().await.expect("test db");
    ensure_test_user(&db).await;
    let server_id = insert_test_server(&db).await;
    let old_token = "old-token";
    insert_credential(&db, server_id, "bearer", None, old_token).await;

    let mock = MockUpstream::start().await;
    let base_url = start_test_server(&db, true).await;

    let client = reqwest::Client::new();
    // First request: populates cache with old_token.
    let resp = client
        .post(format!("{base_url}/inject"))
        .bearer_auth(TEST_SECRET)
        .json(&RequestSkeleton {
            server_id,
            method: "GET".to_string(),
            url: format!("{}/api/resource", mock.url()),
            headers: std::collections::HashMap::new(),
            body: None,
        })
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status().as_u16(), 200);
    mock.assert_received_header("authorization", &format!("Bearer {old_token}"));

    // Rotate the credential (replace DB row) and send notify.
    let new_token = "new-rotated-token";
    let new_encrypted = encrypt_test_credential(new_token);
    let new_iv = new_encrypted[..12].to_vec();
    sqlx::query(
        "UPDATE credentials SET encrypted_payload = $1, iv = $2
         WHERE server_id = $3",
    )
    .bind(&new_encrypted[..])
    .bind(&new_iv[..])
    .bind(server_id)
    .execute(&db.pool)
    .await
    .expect("update credential");

    // Send NOTIFY to trigger cache eviction.
    sqlx::query("SELECT pg_notify('credential_updated', $1)")
        .bind(server_id.to_string())
        .execute(&db.pool)
        .await
        .expect("pg_notify");

    // Allow the NOTIFY listener background task time to process the notification.
    tokio::time::sleep(Duration::from_millis(200)).await;

    mock.clear_requests();

    // Second request: cache miss should hit DB and pick up new token.
    let resp2 = client
        .post(format!("{base_url}/inject"))
        .bearer_auth(TEST_SECRET)
        .json(&RequestSkeleton {
            server_id,
            method: "GET".to_string(),
            url: format!("{}/api/resource", mock.url()),
            headers: std::collections::HashMap::new(),
            body: None,
        })
        .send()
        .await
        .expect("send");
    assert_eq!(resp2.status().as_u16(), 200);
    mock.assert_received_header("authorization", &format!("Bearer {new_token}"));
}

#[tokio::test]
async fn test_missing_authorization_header_returns_403() {
    let db = TestDatabase::new().await.expect("test db");
    let base_url = start_test_server(&db, false).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base_url}/inject"))
        // No Authorization header
        .json(&serde_json::json!({
            "server_id": Uuid::new_v4(),
            "method": "GET",
            "url": "https://api.stripe.com/v1/test"
        }))
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status().as_u16(), 403);
}
