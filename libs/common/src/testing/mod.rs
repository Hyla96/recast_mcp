//! Test-support utilities for integration tests.
//!
//! This module is only compiled when the `testing` Cargo feature is active.
//! Services activate it by listing `mcp-common` in `[dev-dependencies]` with
//! `features = ["testing"]`. It is never present in release builds.
//!
//! # Provided utilities
//!
//! - [`TestDatabase`] — per-test isolated PostgreSQL database, auto-dropped on `Drop`.
//! - [`MockUpstream`] — lightweight in-process HTTP stub that records all requests.
//! - [`TestMcpClient`] — minimal JSON-RPC client for driving gateway endpoints.

pub mod database;
pub mod mcp_client;
pub mod mock_upstream;

pub use database::{TestDatabase, TestDatabaseError};
pub use mcp_client::TestMcpClient;
pub use mock_upstream::{MockUpstream, RecordedRequest};
