//! Graceful shutdown signal handling for the Platform API.
//!
//! Provides a future that resolves when the process receives `SIGTERM` or
//! `SIGINT` (`Ctrl-C`), used with `axum::serve::with_graceful_shutdown`.

/// Returns a future that resolves when `SIGTERM` or `SIGINT` (`Ctrl-C`) is received.
///
/// Used with `axum::serve::with_graceful_shutdown` to stop accepting new
/// connections while allowing in-flight requests to complete.
pub async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::error!("failed to install Ctrl-C handler: {e}");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(e) => {
                tracing::error!("failed to install SIGTERM handler: {e}");
                // Never resolves — ctrl_c branch still works.
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }

    tracing::info!("shutdown signal received — draining in-flight requests");
}
