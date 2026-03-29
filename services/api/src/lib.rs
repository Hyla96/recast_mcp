//! Platform API — shared library components.
//!
//! This crate is structured as both a binary (`main.rs`) and a library so that
//! integration tests in `tests/` can import application types and handlers
//! directly. The binary's entry point in `main.rs` uses items exported here.

/// Shared application state threaded through all axum handlers.
pub mod app_state;

/// Clerk JWT authentication middleware and JWKS cache.
pub mod auth;

/// Environment-validated runtime configuration.
pub mod config;

/// Credential encryption service.
pub mod credentials;

/// MCP Server management service.
pub mod servers;

/// HTTP request handlers, organised by resource area.
pub mod handlers;

/// Platform API-specific middleware layers.
pub mod middleware;

/// Router assembly: CORS, metrics middleware, fallback handler, and route wiring.
pub mod router;

/// Graceful shutdown signal handling (SIGTERM / SIGINT).
pub mod shutdown;
