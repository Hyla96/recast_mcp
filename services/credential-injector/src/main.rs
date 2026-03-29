//! Credential Injector sidecar service.

use axum::{routing::get, Router};
use std::net::SocketAddr;

#[tokio::main]
async fn main() {
    tracing::info!("starting credential injector");

    let app = Router::new().route("/health/live", get(|| async { "ok" }));

    let addr = SocketAddr::from(([127, 0, 0, 1], 3002));
    tracing::info!("listening on {}", addr);

    match tokio::net::TcpListener::bind(&addr).await {
        Ok(listener) => {
            if let Err(e) = axum::serve(listener, app).await {
                tracing::error!("server error: {}", e);
                std::process::exit(1);
            }
        }
        Err(e) => {
            tracing::error!("failed to bind to {}: {}", addr, e);
            std::process::exit(1);
        }
    }
}
