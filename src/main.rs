mod config;
mod models;
mod packets;
mod state;
mod proxy_server;
mod http_server;
mod utils;
mod rate;

use crate::config::Config;
use crate::state::AppState;
use std::sync::Arc;
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber)
        .expect("setting default subscriber failed");

    // Load config
    let config = Config::from_env()?;
    info!("Starting PlayerConnect Server V2...");
    info!("TCP Port: {}", config.player_connect_port);
    info!("HTTP Port: {}", config.player_connect_http_port);

    // Initialize state
    let state = Arc::new(AppState::new());

    // Start Proxy Server
    let proxy_state = state.clone();
    let proxy_port = config.player_connect_port;
    tokio::spawn(async move {
        if let Err(e) = proxy_server::run(proxy_state, proxy_port).await {
            tracing::error!("Proxy server error: {}", e);
        }
    });

    // Start HTTP Server
    let http_state = state.clone();
    let http_port = config.player_connect_http_port;
    http_server::run(http_state, http_port).await?;

    Ok(())
}
