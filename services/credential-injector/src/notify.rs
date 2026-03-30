//! PostgreSQL `LISTEN / NOTIFY` integration for credential cache invalidation.
//!
//! The Platform API sends `pg_notify('credential_updated', '<server_id>')` after
//! every credential rotation. This module subscribes to that channel in a
//! background task and evicts the corresponding LRU cache entry immediately,
//! ensuring the injector picks up new credentials on the next request.

use std::sync::Arc;
use std::time::Duration;

use sqlx::postgres::PgListener;
use uuid::Uuid;

use crate::cache::CredentialCache;

/// Reconnect delay after a `PgListener` error (network drop, server restart,
/// etc.).
const RECONNECT_DELAY_SECS: u64 = 5;

/// Spawns a background tokio task that subscribes to the `credential_updated`
/// PostgreSQL channel and evicts the matching entry from `cache` on every
/// notification.
///
/// The task reconnects automatically after errors, with a brief pause between
/// attempts to avoid hammering the database.
///
/// # Arguments
///
/// * `database_url` — PostgreSQL connection string used by [`PgListener`].
/// * `cache` — Shared credential LRU cache to evict from.
pub fn spawn_notify_listener(database_url: String, cache: Arc<CredentialCache>) {
    tokio::spawn(async move {
        loop {
            match run_listener(&database_url, &cache).await {
                Ok(()) => {
                    // Listener returned cleanly — unlikely in production; reconnect.
                    tracing::warn!("notify listener exited cleanly, reconnecting");
                }
                Err(e) => {
                    tracing::error!(
                        error = %e,
                        reconnect_delay_secs = RECONNECT_DELAY_SECS,
                        "notify listener error — reconnecting"
                    );
                }
            }
            tokio::time::sleep(Duration::from_secs(RECONNECT_DELAY_SECS)).await;
        }
    });
}

/// Connects a [`PgListener`] to the database, subscribes to `credential_updated`,
/// and loops until the connection drops.
async fn run_listener(
    database_url: &str,
    cache: &Arc<CredentialCache>,
) -> Result<(), sqlx::Error> {
    let mut listener = PgListener::connect(database_url).await?;
    listener.listen("credential_updated").await?;

    tracing::info!("notify listener connected — subscribed to 'credential_updated'");

    loop {
        let notification = listener.recv().await?;
        let payload = notification.payload();

        match payload.parse::<Uuid>() {
            Ok(server_id) => {
                let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
                guard.pop(&server_id);
                tracing::debug!(
                    server_id = %server_id,
                    "evicted credential cache entry after notify"
                );
            }
            Err(_) => {
                tracing::warn!(
                    payload = payload,
                    "received malformed credential_updated payload — ignoring"
                );
            }
        }
    }
}
