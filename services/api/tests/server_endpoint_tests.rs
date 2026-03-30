// Platform API — Server CRUD endpoint integration tests.
//
// Tests exercise the HTTP endpoints for MCP server management:
//   POST   /v1/servers                   → 201 with ServerResponse
//   GET    /v1/servers                   → 200 with paginated list
//   GET    /v1/servers/{id}              → 200 / 404
//   PUT    /v1/servers/{id}              → 200 updated
//   DELETE /v1/servers/{id}             → 204, confirmed gone
//   POST   /v1/servers/{id}/validate-url → {valid: true|false}
//   Ownership enforcement                → user B cannot see user A's servers
//
// Required environment variable:
//   TEST_DATABASE_URL=postgres://recast:recast@localhost:5432/postgres \
//     cargo test -p mcp-api --test server_endpoint_tests
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
        servers::{
            create_server_handler, delete_server_handler, get_server_handler,
            list_servers_handler, update_server_handler, validate_url_handler,
        },
        users::me_handler,
    },
    middleware::panic_handler,
};

const GATEWAY_BASE_URL: &str = "https://mcp.test.example.com";

fn make_test_router(state: AppState) -> Router {
    let v1 = Router::new()
        .route("/v1/users/me", get(me_handler))
        .route(
            "/v1/servers",
            get(list_servers_handler).post(create_server_handler),
        )
        .route(
            "/v1/servers/{id}",
            get(get_server_handler)
                .put(update_server_handler)
                .delete(delete_server_handler),
        )
        .route(
            "/v1/servers/{id}/validate-url",
            axum::routing::post(validate_url_handler),
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

// ── Tests ──────────────────────────────────────────────────────────────────────

/// POST /v1/servers returns 201 with id, slug, display_name, mcp_url.
#[tokio::test]
async fn test_create_server_returns_201() {
    let db = TestDatabase::new().await.expect("TestDatabase");
    let (state, _mock) = make_state_with_jwks(db.pool.clone()).await;
    let app = make_test_router(state);

    let clerk_id = format!("user_{}", Uuid::new_v4().simple());
    let email = format!("{}@test.example.com", Uuid::new_v4().simple());
    let jwt = make_jwt(&clerk_id, &email);

    let body = serde_json::json!({
        "display_name": "My Test API",
        "description": "A test server",
        "config": {
            "upstream_base_url": "https://api.example.com"
        }
    });

    let req = Request::builder()
        .method("POST")
        .uri("/v1/servers")
        .header(header::AUTHORIZATION, format!("Bearer {jwt}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);

    let bytes = to_bytes(res.into_body(), 16384).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    assert!(json.get("id").is_some(), "response must have id");
    assert!(json.get("slug").is_some(), "response must have slug");
    assert_eq!(json["display_name"], "My Test API");
    assert_eq!(json["description"], "A test server");
    assert_eq!(json["is_active"], true);
    assert!(json["mcp_url"]
        .as_str()
        .unwrap()
        .starts_with(GATEWAY_BASE_URL));
    assert!(json["mcp_url"].as_str().unwrap().contains("/mcp/"));
}

/// POST /v1/servers with display_name > 100 chars returns 400 VALIDATION_ERROR.
#[tokio::test]
async fn test_create_server_display_name_too_long() {
    let db = TestDatabase::new().await.expect("TestDatabase");
    let (state, _mock) = make_state_with_jwks(db.pool.clone()).await;
    let app = make_test_router(state);

    let clerk_id = format!("user_{}", Uuid::new_v4().simple());
    let email = format!("{}@test.example.com", Uuid::new_v4().simple());
    let jwt = make_jwt(&clerk_id, &email);

    let long_name = "a".repeat(101);
    let body = serde_json::json!({ "display_name": long_name });

    let req = Request::builder()
        .method("POST")
        .uri("/v1/servers")
        .header(header::AUTHORIZATION, format!("Bearer {jwt}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    let bytes = to_bytes(res.into_body(), 8192).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["error"]["code"], "validation_error");
    assert_eq!(json["error"]["details"]["field"], "display_name");
}

/// POST /v1/servers with an SSRF-blocked upstream_base_url returns 422 SSRF_BLOCKED.
#[tokio::test]
async fn test_create_server_ssrf_blocked() {
    let db = TestDatabase::new().await.expect("TestDatabase");
    let (state, _mock) = make_state_with_jwks(db.pool.clone()).await;
    let app = make_test_router(state);

    let clerk_id = format!("user_{}", Uuid::new_v4().simple());
    let email = format!("{}@test.example.com", Uuid::new_v4().simple());
    let jwt = make_jwt(&clerk_id, &email);

    let body = serde_json::json!({
        "display_name": "Private API",
        "config": {
            "upstream_base_url": "http://192.168.1.100/api"
        }
    });

    let req = Request::builder()
        .method("POST")
        .uri("/v1/servers")
        .header(header::AUTHORIZATION, format!("Bearer {jwt}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let bytes = to_bytes(res.into_body(), 8192).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["error"]["code"], "ssrf_blocked");
}

/// GET /v1/servers returns only the caller's servers.
#[tokio::test]
async fn test_list_servers_returns_only_own_servers() {
    let db = TestDatabase::new().await.expect("TestDatabase");
    let (state, _mock) = make_state_with_jwks(db.pool.clone()).await;
    let app = make_test_router(state);

    // User A — creates a server via the API.
    let clerk_a = format!("user_{}", Uuid::new_v4().simple());
    let email_a = format!("{}@test.example.com", Uuid::new_v4().simple());
    let jwt_a = make_jwt(&clerk_a, &email_a);

    // User B — also creates a server.
    let clerk_b = format!("user_{}", Uuid::new_v4().simple());
    let email_b = format!("{}@test.example.com", Uuid::new_v4().simple());
    let jwt_b = make_jwt(&clerk_b, &email_b);

    let make_server = |jwt: &str, name: &str| {
        let body = serde_json::json!({ "display_name": name });
        Request::builder()
            .method("POST")
            .uri("/v1/servers")
            .header(header::AUTHORIZATION, format!("Bearer {}", jwt))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    };

    // Create server for A.
    let app2 = app.clone();
    let res = app2.oneshot(make_server(&jwt_a, "Server A")).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);

    // Create server for B.
    let app3 = app.clone();
    let res = app3.oneshot(make_server(&jwt_b, "Server B")).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);

    // List for A — should see only "Server A".
    let req = Request::builder()
        .method("GET")
        .uri("/v1/servers")
        .header(header::AUTHORIZATION, format!("Bearer {jwt_a}"))
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let bytes = to_bytes(res.into_body(), 16384).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let servers = json["servers"].as_array().unwrap();
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0]["display_name"], "Server A");
}

/// GET /v1/servers/{id} for another user's server returns 404.
#[tokio::test]
async fn test_get_server_foreign_user_returns_404() {
    let db = TestDatabase::new().await.expect("TestDatabase");
    let (state, _mock) = make_state_with_jwks(db.pool.clone()).await;
    let app = make_test_router(state);

    // User A creates a server.
    let clerk_a = format!("user_{}", Uuid::new_v4().simple());
    let email_a = format!("{}@test.example.com", Uuid::new_v4().simple());
    let jwt_a = make_jwt(&clerk_a, &email_a);

    let body = serde_json::json!({ "display_name": "User A Server" });
    let req = Request::builder()
        .method("POST")
        .uri("/v1/servers")
        .header(header::AUTHORIZATION, format!("Bearer {jwt_a}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let bytes = to_bytes(res.into_body(), 8192).await.unwrap();
    let created: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let server_id = created["id"].as_str().unwrap().to_string();

    // User B tries to GET user A's server.
    let clerk_b = format!("user_{}", Uuid::new_v4().simple());
    let email_b = format!("{}@test.example.com", Uuid::new_v4().simple());
    let jwt_b = make_jwt(&clerk_b, &email_b);

    let req = Request::builder()
        .method("GET")
        .uri(format!("/v1/servers/{server_id}"))
        .header(header::AUTHORIZATION, format!("Bearer {jwt_b}"))
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

/// PUT /v1/servers/{id} updates the server fields.
#[tokio::test]
async fn test_update_server_returns_updated_fields() {
    let db = TestDatabase::new().await.expect("TestDatabase");
    let (state, _mock) = make_state_with_jwks(db.pool.clone()).await;
    let app = make_test_router(state);

    let clerk_id = format!("user_{}", Uuid::new_v4().simple());
    let email = format!("{}@test.example.com", Uuid::new_v4().simple());
    let jwt = make_jwt(&clerk_id, &email);

    // Create.
    let create_body = serde_json::json!({ "display_name": "Original Name" });
    let req = Request::builder()
        .method("POST")
        .uri("/v1/servers")
        .header(header::AUTHORIZATION, format!("Bearer {jwt}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(create_body.to_string()))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let bytes = to_bytes(res.into_body(), 8192).await.unwrap();
    let created: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let server_id = created["id"].as_str().unwrap().to_string();

    // Update display_name and deactivate.
    let update_body = serde_json::json!({
        "display_name": "Updated Name",
        "is_active": false
    });
    let req = Request::builder()
        .method("PUT")
        .uri(format!("/v1/servers/{server_id}"))
        .header(header::AUTHORIZATION, format!("Bearer {jwt}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(update_body.to_string()))
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let bytes = to_bytes(res.into_body(), 8192).await.unwrap();
    let updated: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(updated["display_name"], "Updated Name");
    assert_eq!(updated["is_active"], false);
    assert_eq!(updated["id"], server_id.as_str());
}

/// DELETE /v1/servers/{id} removes the server; subsequent GET returns 404.
#[tokio::test]
async fn test_delete_server_removes_row() {
    let db = TestDatabase::new().await.expect("TestDatabase");
    let (state, _mock) = make_state_with_jwks(db.pool.clone()).await;
    let app = make_test_router(state);

    let clerk_id = format!("user_{}", Uuid::new_v4().simple());
    let email = format!("{}@test.example.com", Uuid::new_v4().simple());
    let jwt = make_jwt(&clerk_id, &email);

    // Create.
    let body = serde_json::json!({ "display_name": "To Be Deleted" });
    let req = Request::builder()
        .method("POST")
        .uri("/v1/servers")
        .header(header::AUTHORIZATION, format!("Bearer {jwt}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let bytes = to_bytes(res.into_body(), 8192).await.unwrap();
    let created: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let server_id = created["id"].as_str().unwrap().to_string();

    // Delete.
    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/v1/servers/{server_id}"))
        .header(header::AUTHORIZATION, format!("Bearer {jwt}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    // Confirm gone — GET now returns 404.
    let req = Request::builder()
        .method("GET")
        .uri(format!("/v1/servers/{server_id}"))
        .header(header::AUTHORIZATION, format!("Bearer {jwt}"))
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    // Confirm row gone directly in DB.
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM mcp_servers WHERE id = $1",
    )
    .bind(Uuid::parse_str(&server_id).unwrap())
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(count, 0, "server row must be removed from DB after DELETE");
}

/// POST /v1/servers/{id}/validate-url returns valid:true for a safe public URL.
#[tokio::test]
async fn test_validate_url_safe_returns_valid_true() {
    let db = TestDatabase::new().await.expect("TestDatabase");
    let (state, _mock) = make_state_with_jwks(db.pool.clone()).await;
    let app = make_test_router(state);

    let clerk_id = format!("user_{}", Uuid::new_v4().simple());
    let email = format!("{}@test.example.com", Uuid::new_v4().simple());
    let jwt = make_jwt(&clerk_id, &email);

    // Create a server first.
    let body = serde_json::json!({ "display_name": "Validate URL Test" });
    let req = Request::builder()
        .method("POST")
        .uri("/v1/servers")
        .header(header::AUTHORIZATION, format!("Bearer {jwt}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let bytes = to_bytes(res.into_body(), 8192).await.unwrap();
    let created: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let server_id = created["id"].as_str().unwrap().to_string();

    // Validate a safe URL.
    let validate_body = serde_json::json!({ "url": "https://api.stripe.com/v1/customers" });
    let req = Request::builder()
        .method("POST")
        .uri(format!("/v1/servers/{server_id}/validate-url"))
        .header(header::AUTHORIZATION, format!("Bearer {jwt}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(validate_body.to_string()))
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let bytes = to_bytes(res.into_body(), 8192).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["valid"], true);
    assert!(json.get("error").is_none(), "no error field on valid response");
}

/// POST /v1/servers/{id}/validate-url returns valid:false for a private IP.
#[tokio::test]
async fn test_validate_url_private_ip_returns_valid_false() {
    let db = TestDatabase::new().await.expect("TestDatabase");
    let (state, _mock) = make_state_with_jwks(db.pool.clone()).await;
    let app = make_test_router(state);

    let clerk_id = format!("user_{}", Uuid::new_v4().simple());
    let email = format!("{}@test.example.com", Uuid::new_v4().simple());
    let jwt = make_jwt(&clerk_id, &email);

    // Create a server.
    let body = serde_json::json!({ "display_name": "Validate URL Test 2" });
    let req = Request::builder()
        .method("POST")
        .uri("/v1/servers")
        .header(header::AUTHORIZATION, format!("Bearer {jwt}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let bytes = to_bytes(res.into_body(), 8192).await.unwrap();
    let created: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let server_id = created["id"].as_str().unwrap().to_string();

    // Validate a blocked URL.
    let validate_body = serde_json::json!({ "url": "http://10.0.0.1/internal" });
    let req = Request::builder()
        .method("POST")
        .uri(format!("/v1/servers/{server_id}/validate-url"))
        .header(header::AUTHORIZATION, format!("Bearer {jwt}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(validate_body.to_string()))
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let bytes = to_bytes(res.into_body(), 8192).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["valid"], false);
    assert!(json.get("error").is_some(), "error field must be present");
    assert_eq!(json["error"]["code"], "ssrf_blocked");
}

/// GET /v1/servers supports cursor pagination (after param) and returns has_next.
#[tokio::test]
async fn test_list_servers_cursor_pagination() {
    let db = TestDatabase::new().await.expect("TestDatabase");
    let (state, _mock) = make_state_with_jwks(db.pool.clone()).await;
    let app = make_test_router(state);

    let clerk_id = format!("user_{}", Uuid::new_v4().simple());
    let email = format!("{}@test.example.com", Uuid::new_v4().simple());
    let jwt = make_jwt(&clerk_id, &email);

    // Create 3 servers.
    for i in 0..3u8 {
        let body = serde_json::json!({ "display_name": format!("Server {i}") });
        let req = Request::builder()
            .method("POST")
            .uri("/v1/servers")
            .header(header::AUTHORIZATION, format!("Bearer {jwt}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);
    }

    // Fetch first page with limit=2.
    let req = Request::builder()
        .method("GET")
        .uri("/v1/servers?limit=2")
        .header(header::AUTHORIZATION, format!("Bearer {jwt}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let bytes = to_bytes(res.into_body(), 8192).await.unwrap();
    let first_page: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(first_page["pagination"]["total"], 3_i64);
    assert_eq!(first_page["pagination"]["has_next"], true);
    let first_servers = first_page["servers"].as_array().unwrap();
    assert_eq!(first_servers.len(), 2);

    // Fetch second page using the last item's id as cursor.
    let last_id = first_servers.last().unwrap()["id"].as_str().unwrap();
    let req = Request::builder()
        .method("GET")
        .uri(format!("/v1/servers?limit=2&after={last_id}"))
        .header(header::AUTHORIZATION, format!("Bearer {jwt}"))
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let bytes = to_bytes(res.into_body(), 8192).await.unwrap();
    let second_page: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(second_page["pagination"]["has_next"], false);
    let second_servers = second_page["servers"].as_array().unwrap();
    assert_eq!(second_servers.len(), 1);
}

/// DELETE /v1/servers/{id} by another user returns 404 (ownership enforced).
#[tokio::test]
async fn test_delete_server_foreign_user_returns_404() {
    let db = TestDatabase::new().await.expect("TestDatabase");
    let (state, _mock) = make_state_with_jwks(db.pool.clone()).await;
    let app = make_test_router(state);

    let clerk_a = format!("user_{}", Uuid::new_v4().simple());
    let email_a = format!("{}@test.example.com", Uuid::new_v4().simple());
    let jwt_a = make_jwt(&clerk_a, &email_a);

    // Create server as user A.
    let body = serde_json::json!({ "display_name": "User A Server" });
    let req = Request::builder()
        .method("POST")
        .uri("/v1/servers")
        .header(header::AUTHORIZATION, format!("Bearer {jwt_a}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let bytes = to_bytes(res.into_body(), 8192).await.unwrap();
    let created: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let server_id = created["id"].as_str().unwrap().to_string();

    // User B tries to DELETE user A's server.
    let clerk_b = format!("user_{}", Uuid::new_v4().simple());
    let email_b = format!("{}@test.example.com", Uuid::new_v4().simple());
    let jwt_b = make_jwt(&clerk_b, &email_b);

    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/v1/servers/{server_id}"))
        .header(header::AUTHORIZATION, format!("Bearer {jwt_b}"))
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}
