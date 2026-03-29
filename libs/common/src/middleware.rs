//! Shared axum middleware for all services.

use axum::{extract::Request, middleware::Next, response::Response};
use ulid::Ulid;

/// A per-request identifier stored as a ULID string.
///
/// Inserted into request extensions by [`request_id_middleware`] so that
/// route handlers can extract it via `Extension<RequestId>` if they need to
/// embed it in non-error response bodies.
#[derive(Clone, Debug)]
pub struct RequestId(pub String);

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Axum middleware that generates a ULID per request.
///
/// The generated ULID is:
/// - Stored in the request's [`axum::extract::Extension`] as [`RequestId`] so
///   handlers can access it.
/// - Written to the `X-Request-ID` response header for **successful responses**
///   (i.e. those that do not already carry an `X-Request-ID` header).
///
/// Error responses produced by [`crate::AppError::into_response`] generate
/// their own ULID and set `X-Request-ID` directly, so the middleware skips
/// overwriting the header in that case — ensuring the header and body
/// `request_id` field always match.
///
/// # Usage
///
/// ```rust,ignore
/// use axum::{middleware, Router};
/// use mcp_common::middleware::request_id_middleware;
///
/// let app = Router::new()
///     /* routes */
///     .layer(middleware::from_fn(request_id_middleware));
/// ```
pub async fn request_id_middleware(mut req: Request, next: Next) -> Response {
    let request_id = Ulid::new().to_string();
    req.extensions_mut().insert(RequestId(request_id.clone()));

    let mut response = next.run(req).await;

    // Only inject the header when the response does not already carry one.
    // AppError::into_response() sets its own X-Request-ID so it is skipped here,
    // guaranteeing that the header and the body request_id field always match.
    if !response.headers().contains_key("x-request-id") {
        if let Ok(value) = request_id.parse() {
            response.headers_mut().insert("x-request-id", value);
        }
    }

    response
}
