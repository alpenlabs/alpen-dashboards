mod bridge;
mod config;
mod network;
mod utils;

use axum::{routing::get, Router};
use std::{net::SocketAddr, sync::Arc};
use tokio::net::TcpListener;
use tower_http::cors::{Any, CorsLayer};
use tracing::info;

use crate::{
    bridge::status::{bridge_monitoring_task, get_bridge_status},
    bridge::types::BridgeMonitoringContext,
    config::Config,
    network::{
        status::{fetch_statuses_task, get_network_status},
        types::NetworkMonitoringContext,
    },
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // Load configuration from TOML
    let config_path = std::env::var("CONFIG_PATH").unwrap_or_else(|_| "config.toml".to_string());
    let config = Arc::new(Config::load_from_path(&config_path));

    let cors = CorsLayer::new().allow_origin(Any);

    let network_context = Arc::new(NetworkMonitoringContext::new(config.network.clone()));
    let network_context_clone = Arc::clone(&network_context);

    tokio::spawn(async move {
        fetch_statuses_task(network_context_clone).await;
    });

    // Bridge monitoring
    let bridge_context = Arc::new(BridgeMonitoringContext::new(config.bridge.clone()));
    let bridge_context_clone = Arc::clone(&bridge_context);

    tokio::spawn(async move {
        bridge_monitoring_task(bridge_context_clone).await;
    });

    let app = Router::new()
        .route(
            "/api/status",
            get(move || get_network_status(network_context)),
        )
        .route(
            "/api/bridge_status",
            get(move || get_bridge_status(bridge_context)),
        )
        .layer(cors);

    let addr = SocketAddr::from((
        config.server.host().parse::<std::net::IpAddr>()?,
        config.server.port(),
    ));
    info!(%addr, "Server running at http://{}", addr);

    let listener = TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
