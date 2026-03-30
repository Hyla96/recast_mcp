//! HTTP request handlers for the Platform API.
//!
//! Each sub-module corresponds to a resource area. Handlers are registered
//! in [`crate::build_router`] and rely on the full middleware stack defined
//! in `main.rs`.

pub mod users;

/// Clerk webhook handler — user lifecycle event processing.
pub mod webhooks;

/// Credential CRUD handlers.
pub mod credentials;

/// Server token CRUD handlers.
pub mod tokens;

/// MCP Server CRUD handlers.
pub mod servers;

/// Proxy test endpoint — dispatches upstream calls on behalf of the builder UI.
pub mod proxy;
