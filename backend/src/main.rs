mod bridge;
mod config;
mod network;
mod utils;

use axum::{routing::get, Router};
use dotenvy::dotenv;
use std::{net::SocketAddr, sync::Arc};
use tokio::net::TcpListener;
use tower_http::cors::{Any, CorsLayer};
use tracing::info;

use crate::{
    bridge::status::{bridge_monitoring_task, get_bridge_status},
    bridge::types::BridgeMonitoringContext,
    config::BridgeMonitoringConfig,
    network::status::{fetch_statuses_task, get_network_status, SharedNetworkState},
};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    dotenv().ok();

    let config = Arc::new(config::NetworkConfig::new());

    let cors = CorsLayer::new().allow_origin(Any);

    // Shared state for network status
    let shared_state = SharedNetworkState::default();

    // Spawn a background task to fetch real statuses
    let state_clone = Arc::clone(&shared_state);
    tokio::spawn({
        let config = Arc::clone(&config);
        async move {
            fetch_statuses_task(state_clone, &config).await;
        }
    });

    // bridge monitoring
    let bridge_monitoring_config = BridgeMonitoringConfig::new();
    let bridge_context = Arc::new(BridgeMonitoringContext::new(bridge_monitoring_config));
    let bridge_context_clone = Arc::clone(&bridge_context);

    tokio::spawn(async move {
        bridge_monitoring_task(bridge_context_clone).await;
    });

    let app = Router::new()
        .route(
            "/api/status",
            get(move || get_network_status(Arc::clone(&shared_state))),
        )
        .route(
            "/api/bridge_status",
            get(move || get_bridge_status(Arc::clone(&bridge_context))),
        )
        .layer(cors);

    let addr = SocketAddr::from(([0, 0, 0, 0], 3000));
    info!(%addr, "Server running at http://");

    let listener = TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
