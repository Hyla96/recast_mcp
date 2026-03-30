// Shared test infrastructure for Platform API integration tests.
//
// This module is included by each test file via `mod helpers;` and provides:
//   - TestRsaKey / build_test_rsa_key / test_key    — shared RSA key pair
//   - TEST_ISSUER constant
//   - TestClaims struct
//   - make_jwt / make_jwt_with_offset               — JWT helpers
//   - make_state_with_jwks                          — AppState + MockUpstream builder
#![allow(dead_code)]

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use mcp_common::{testing::MockUpstream, AuditLogger};
use rand::thread_rng;
use rsa::{pkcs1::EncodeRsaPrivateKey, traits::PublicKeyParts, RsaPrivateKey};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, OnceLock};

use mcp_api::{
    app_state::{AppState, SsrfValidatorFn},
    auth::JwksCache,
    config::ApiConfig,
    credentials::CredentialService,
    servers::ServerService,
};
use mcp_crypto::CryptoKey;

// ── RSA test key ─────────────────────────────────────────────────────────────

pub struct TestRsaKey {
    /// Private key in PKCS#1 PEM format, for jsonwebtoken's EncodingKey.
    pub encoding_key: EncodingKey,
    /// Key ID used in JWKS and JWT headers.
    pub kid: &'static str,
    /// Pre-serialized JWKS JSON served by MockUpstream.
    pub jwks_json: String,
}

/// Build a fresh `TestRsaKey` with the given `kid`.
/// Prefer `test_key()` for the shared singleton to avoid repeated key generation.
pub fn build_test_rsa_key(kid: &'static str) -> TestRsaKey {
    let mut rng = thread_rng();
    let priv_key = RsaPrivateKey::new(&mut rng, 2048).expect("RSA key gen");
    let pub_key = rsa::RsaPublicKey::from(&priv_key);

    let pem = priv_key
        .to_pkcs1_pem(rsa::pkcs1::LineEnding::LF)
        .expect("PEM export");
    let encoding_key = EncodingKey::from_rsa_pem(pem.as_bytes()).expect("EncodingKey");

    let n = URL_SAFE_NO_PAD.encode(pub_key.n().to_bytes_be());
    let e = URL_SAFE_NO_PAD.encode(pub_key.e().to_bytes_be());

    let jwks_json = serde_json::json!({
        "keys": [{
            "kty": "RSA",
            "kid": kid,
            "n": n,
            "e": e,
            "alg": "RS256",
            "use": "sig"
        }]
    })
    .to_string();

    TestRsaKey { encoding_key, kid, jwks_json }
}

static SHARED_TEST_RSA_KEY: OnceLock<TestRsaKey> = OnceLock::new();

/// Returns the shared RSA key singleton (generated once per test process).
pub fn test_key() -> &'static TestRsaKey {
    SHARED_TEST_RSA_KEY.get_or_init(|| build_test_rsa_key("shared-test-key-001"))
}

// ── JWT helpers ───────────────────────────────────────────────────────────────

pub const TEST_ISSUER: &str = "https://test-clerk.example.dev";

/// All claims in a Clerk-style JWT used for testing.
#[derive(Debug, Serialize, Deserialize)]
pub struct TestClaims {
    pub sub: String,
    pub email: String,
    pub iss: String,
    pub exp: u64,
    pub nbf: u64,
}

/// Build a signed JWT valid for 1 hour.
pub fn make_jwt(sub: &str, email: &str) -> String {
    make_jwt_with_offset(sub, email, 3600)
}

/// Build a signed JWT with a custom expiry offset.
///
/// Positive `exp_offset_secs` → token expires in the future.
/// Negative `exp_offset_secs` → token is already expired.
pub fn make_jwt_with_offset(sub: &str, email: &str, exp_offset_secs: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before epoch")
        .as_secs();

    let exp = if exp_offset_secs >= 0 {
        now + exp_offset_secs as u64
    } else {
        now.saturating_sub((-exp_offset_secs) as u64)
    };

    let claims = TestClaims {
        sub: sub.to_string(),
        email: email.to_string(),
        iss: TEST_ISSUER.to_string(),
        exp,
        nbf: now - 60, // slight past to avoid nbf clock skew
    };

    let key = test_key();
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(key.kid.to_string());

    encode(&header, &claims, &key.encoding_key).expect("JWT encode")
}

// ── AppState builder ──────────────────────────────────────────────────────────

/// Returns a passthrough SSRF validator that always returns `Ok(())`.
///
/// Required in integration tests where the proxy target is `127.0.0.1`
/// (MockUpstream), which would otherwise be blocked by the real SSRF check.
pub fn passthrough_ssrf_validator() -> SsrfValidatorFn {
    Arc::new(|_url: url::Url| Box::pin(async { Ok(()) }))
}

/// Starts a `MockUpstream` serving the shared JWKS and builds an `AppState`.
///
/// The returned `MockUpstream` must be kept alive for the duration of the test —
/// dropping it shuts down the stub HTTP server.
///
/// Uses a passthrough SSRF validator (allowing `127.0.0.1`) and a 150 ms
/// proxy timeout so proxy test scenarios complete quickly.
pub async fn make_state_with_jwks(pool: sqlx::PgPool) -> (AppState, MockUpstream) {
    let key = test_key();
    let mock = MockUpstream::start().await;
    mock.set_response_body(serde_json::from_str(&key.jwks_json).expect("JWKS JSON"));

    let jwks_url = format!("{}/jwks", mock.url());

    let crypto_key = Arc::new(CryptoKey::from_bytes([0x42u8; 32]));
    let audit_logger = AuditLogger::new(pool.clone());
    let credential_service =
        CredentialService::new(pool.clone(), crypto_key, audit_logger.clone());
    let server_service = ServerService::new(
        pool.clone(),
        audit_logger.clone(),
        "https://mcp.test.example.com".to_string(),
    );

    let http_client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_millis(500))
        .build()
        .expect("test HTTP client");

    let state = AppState {
        pool: pool.clone(),
        config: Arc::new(ApiConfig {
            port: 3001,
            database_url: "postgres://test".to_string(),
            clerk_secret_key: "sk_test_xxx".to_string(),
            clerk_jwks_url: jwks_url.clone(),
            clerk_webhook_secret: "whsec_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
                .to_string(),
            encryption_key: "4".repeat(64),
            clerk_issuer: TEST_ISSUER.to_string(),
            cors_origins: vec![],
            gateway_base_url: "https://mcp.test.example.com".to_string(),
        }),
        audit_logger,
        jwks_cache: JwksCache::new(&jwks_url),
        credential_service,
        server_service,
        http_client,
        ssrf_validator: passthrough_ssrf_validator(),
        proxy_timeout: std::time::Duration::from_millis(150),
    };

    (state, mock)
}
