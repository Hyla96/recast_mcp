// Platform API integration tests.
//
// These tests require a live PostgreSQL instance.
// Set TEST_DATABASE_URL (or DATABASE_URL) before running:
//
//   TEST_DATABASE_URL=postgres://recast:recast@localhost:5432/postgres \
//     cargo test -p mcp-api --test integration_tests
//
// Each test receives its own isolated database (via TestDatabase) so tests run
// in parallel without data contamination.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, missing_docs)]

use mcp_common::testing::TestDatabase;
use sha2::{Digest, Sha256};

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

// ─── TestDatabase isolation (10 parallel tests) ───────────────────────────────

/// Each of these tests creates its own isolated database instance.
/// Running with `cargo test` (default parallel mode) exercises concurrent
/// creation / migration / teardown.

macro_rules! isolation_test {
    ($name:ident) => {
        #[tokio::test]
        async fn $name() {
            let db = TestDatabase::new().await.expect("TestDatabase::new");

            // The database should have the five core tables from migrations.
            let count: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = 'public'")
                    .fetch_one(&db.pool)
                    .await
                    .expect("query failed");

            // Expect exactly our 5 tables: users, mcp_servers, credentials,
            // server_tokens, audit_log.
            assert_eq!(count, 5, "expected 5 tables after migration");
        }
    };
}

isolation_test!(db_isolation_01);
isolation_test!(db_isolation_02);
isolation_test!(db_isolation_03);
isolation_test!(db_isolation_04);
isolation_test!(db_isolation_05);
isolation_test!(db_isolation_06);
isolation_test!(db_isolation_07);
isolation_test!(db_isolation_08);
isolation_test!(db_isolation_09);
isolation_test!(db_isolation_10);

// ─── TestDatabase drop removes the database ───────────────────────────────────

#[tokio::test]
async fn db_drop_removes_database() {
    let admin_url = std::env::var("TEST_DATABASE_URL")
        .or_else(|_| std::env::var("DATABASE_URL"))
        .expect("TEST_DATABASE_URL or DATABASE_URL must be set");

    // Replace db name with "postgres" for admin connection.
    let admin_url = {
        let base = if let Some(pos) = admin_url.find('?') {
            &admin_url[..pos]
        } else {
            &admin_url
        };
        let prefix = base.rfind('/').map(|p| &base[..p]).unwrap_or(base);
        format!("{prefix}/postgres")
    };

    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let db_name = {
        // Extract the database name by querying current_database().
        let name: String = sqlx::query_scalar("SELECT current_database()")
            .fetch_one(&db.pool)
            .await
            .expect("current_database");
        name
    };

    // Confirm the database exists.
    let admin = sqlx::PgPool::connect(&admin_url)
        .await
        .expect("admin connect");
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname = $1)")
            .bind(&db_name)
            .fetch_one(&admin)
            .await
            .expect("existence check");
    assert!(exists, "database should exist before drop");

    // Drop the TestDatabase — cleanup runs in the background thread.
    drop(db);

    // Wait briefly for the background cleanup thread to finish.
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let still_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname = $1)")
            .bind(&db_name)
            .fetch_one(&admin)
            .await
            .expect("post-drop existence check");
    assert!(!still_exists, "database should be gone after drop");
    admin.close().await;
}

// ─── User upsert ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn user_insert_and_upsert() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");

    // Initial insert.
    sqlx::query(
        "INSERT INTO users (clerk_id, email, plan) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
    )
    .bind("clerk_test_aaa")
    .bind("aaa@example.com")
    .bind("community")
    .execute(&db.pool)
    .await
    .expect("insert user");

    let email: String =
        sqlx::query_scalar("SELECT email FROM users WHERE clerk_id = $1")
            .bind("clerk_test_aaa")
            .fetch_one(&db.pool)
            .await
            .expect("fetch user");
    assert_eq!(email, "aaa@example.com");

    // Second insert with same clerk_id must be a no-op (ON CONFLICT DO NOTHING).
    sqlx::query(
        "INSERT INTO users (clerk_id, email, plan) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
    )
    .bind("clerk_test_aaa")
    .bind("other@example.com")
    .bind("pro")
    .execute(&db.pool)
    .await
    .expect("upsert noop");

    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE clerk_id = $1")
            .bind("clerk_test_aaa")
            .fetch_one(&db.pool)
            .await
            .expect("count");
    assert_eq!(count, 1, "upsert should not create a duplicate row");
}

// ─── Server CRUD ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn server_crud() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");

    // Insert prerequisite user.
    let user_id: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO users (clerk_id, email) VALUES ($1, $2) RETURNING id")
            .bind("clerk_server_crud")
            .bind("server_crud@example.com")
            .fetch_one(&db.pool)
            .await
            .expect("insert user returning id");

    // Create server.
    let config = serde_json::json!({"tools": []});
    let server_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO mcp_servers (user_id, name, slug, config_json, status)
         VALUES ($1, $2, $3, $4, 'draft') RETURNING id",
    )
    .bind(user_id)
    .bind("Test Server")
    .bind("test-server-crud")
    .bind(&config)
    .fetch_one(&db.pool)
    .await
    .expect("create server");

    // Read server.
    let name: String =
        sqlx::query_scalar("SELECT name FROM mcp_servers WHERE id = $1")
            .bind(server_id)
            .fetch_one(&db.pool)
            .await
            .expect("read server");
    assert_eq!(name, "Test Server");

    // Update server name.
    sqlx::query("UPDATE mcp_servers SET name = $1 WHERE id = $2")
        .bind("Renamed Server")
        .bind(server_id)
        .execute(&db.pool)
        .await
        .expect("update server");

    let updated_name: String =
        sqlx::query_scalar("SELECT name FROM mcp_servers WHERE id = $1")
            .bind(server_id)
            .fetch_one(&db.pool)
            .await
            .expect("read updated server");
    assert_eq!(updated_name, "Renamed Server");

    // Delete server.
    sqlx::query("DELETE FROM mcp_servers WHERE id = $1")
        .bind(server_id)
        .execute(&db.pool)
        .await
        .expect("delete server");

    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM mcp_servers WHERE id = $1")
            .bind(server_id)
            .fetch_one(&db.pool)
            .await
            .expect("count after delete");
    assert_eq!(count, 0, "server should be deleted");
}

// ─── Credential write — verify ciphertext not plaintext ───────────────────────

#[tokio::test]
async fn credential_write_stores_ciphertext_not_plaintext() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");

    let user_id: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO users (clerk_id, email) VALUES ($1, $2) RETURNING id")
            .bind("clerk_cred_test")
            .bind("cred@example.com")
            .fetch_one(&db.pool)
            .await
            .expect("insert user");

    let server_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO mcp_servers (user_id, name, slug, config_json, status)
         VALUES ($1, 'Cred Server', 'cred-server', '{}', 'active') RETURNING id",
    )
    .bind(user_id)
    .fetch_one(&db.pool)
    .await
    .expect("insert server");

    // Encrypt a plaintext credential using mcp-crypto.
    let key = mcp_crypto::CryptoKey::from_bytes([0x42u8; 32]);
    let plaintext = b"Bearer super-secret-token-12345";
    let iv_and_ciphertext = mcp_crypto::encrypt(&key, plaintext).expect("encrypt");

    // mcp-crypto returns IV (12 bytes) || ciphertext.
    // The DB schema stores them separately.
    let iv = &iv_and_ciphertext[..12];
    let encrypted_payload = &iv_and_ciphertext[12..];

    assert!(
        !encrypted_payload.windows(plaintext.len()).any(|w| w == plaintext),
        "encrypted payload must not contain plaintext"
    );

    sqlx::query(
        "INSERT INTO credentials (server_id, auth_type, encrypted_payload, iv)
         VALUES ($1, 'bearer', $2, $3)",
    )
    .bind(server_id)
    .bind(encrypted_payload)
    .bind(iv)
    .execute(&db.pool)
    .await
    .expect("insert credential");

    // Fetch stored bytes and confirm they do not equal the plaintext.
    let stored_payload: Vec<u8> =
        sqlx::query_scalar("SELECT encrypted_payload FROM credentials WHERE server_id = $1")
            .bind(server_id)
            .fetch_one(&db.pool)
            .await
            .expect("fetch credential");

    assert_ne!(
        stored_payload.as_slice(),
        plaintext.as_slice(),
        "stored payload must not be the plaintext credential"
    );

    // Verify roundtrip: reconstruct IV || ciphertext and decrypt.
    let mut full = Vec::new();
    full.extend_from_slice(iv);
    full.extend_from_slice(&stored_payload);
    let decrypted = mcp_crypto::decrypt(&key, &full).expect("decrypt");
    assert_eq!(decrypted.as_slice(), plaintext.as_ref(), "decrypted value must match original plaintext");
}

// ─── FK constraint: credential with unknown server_id rejected ────────────────

#[tokio::test]
async fn credential_fk_constraint_enforced() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");
    let fake_server_id = uuid::Uuid::new_v4();

    let result = sqlx::query(
        "INSERT INTO credentials (server_id, auth_type, encrypted_payload, iv)
         VALUES ($1, 'bearer', $2, $3)",
    )
    .bind(fake_server_id)
    .bind(b"dummy_ciphertext" as &[u8])
    .bind(b"dummy_iv_12byte_" as &[u8])
    .execute(&db.pool)
    .await;

    assert!(result.is_err(), "FK violation should cause an error");
}

// ─── Server token generation and revocation ───────────────────────────────────

#[tokio::test]
async fn server_token_generate_and_revoke() {
    let db = TestDatabase::new().await.expect("TestDatabase::new");

    let user_id: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO users (clerk_id, email) VALUES ($1, $2) RETURNING id")
            .bind("clerk_token_test")
            .bind("token@example.com")
            .fetch_one(&db.pool)
            .await
            .expect("insert user");

    let server_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO mcp_servers (user_id, name, slug, config_json, status)
         VALUES ($1, 'Token Server', 'token-server', '{}', 'active') RETURNING id",
    )
    .bind(user_id)
    .fetch_one(&db.pool)
    .await
    .expect("insert server");

    // Generate token: hash the raw token and store only the hash.
    let raw_token = "test-integration-token-abc123";
    let token_hash = sha256_hex(raw_token);

    let token_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO server_tokens (server_id, token_hash, description)
         VALUES ($1, $2, 'integration test token') RETURNING id",
    )
    .bind(server_id)
    .bind(&token_hash)
    .fetch_one(&db.pool)
    .await
    .expect("insert token");

    // Active token must be findable by its hash.
    let found: bool =
        sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM server_tokens WHERE token_hash = $1 AND is_active = true)",
        )
        .bind(&token_hash)
        .fetch_one(&db.pool)
        .await
        .expect("active token lookup");
    assert!(found, "active token should be found");

    // Revoke: set is_active = false and record revoked_at.
    sqlx::query(
        "UPDATE server_tokens SET is_active = false, revoked_at = NOW() WHERE id = $1",
    )
    .bind(token_id)
    .execute(&db.pool)
    .await
    .expect("revoke token");

    // Revoked token must not match the active-only query.
    let still_active: bool =
        sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM server_tokens WHERE token_hash = $1 AND is_active = true)",
        )
        .bind(&token_hash)
        .fetch_one(&db.pool)
        .await
        .expect("post-revoke lookup");
    assert!(!still_active, "revoked token must not appear in active lookup");
}
