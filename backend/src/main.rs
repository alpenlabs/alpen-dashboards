mod activity;
mod bridge;
mod config;
mod network;
mod utils;
mod wallets;

use axum::{routing::get, Router};
use dotenvy::dotenv;
use std::{net::SocketAddr, sync::Arc};
use tokio::{net::TcpListener, sync::RwLock};
use tower_http::cors::{Any, CorsLayer};
use tracing::info;

use crate::{
    activity::{
        stats::{activity_monitoring_task, get_activity_stats},
        types::ActivityStats,
    },
    bridge::status::{bridge_monitoring_task, get_bridge_status, SharedBridgeState},
    config::{ActivityMonitoringConfig, BridgeMonitoringConfig},
    network::{
        status::{get_network_status, monitor_network_task, SharedNetworkState},
        types::NetworkStatus,
    },
    wallets::balance::{
        fetch_balances_task, get_wallets_with_balances, init_paymaster_wallets, SharedWallets,
    },
};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    dotenv().ok();

    let config = Arc::new(config::NetworkConfig::new());

    let cors = CorsLayer::new().allow_origin(Any);

    let shared_network_status: SharedNetworkState = Arc::new(RwLock::new(NetworkStatus::default()));

    tokio::spawn({
        let status_clone = Arc::clone(&shared_network_status);
        let config_clone = Arc::clone(&config);
        async move {
            monitor_network_task(status_clone, &config_clone).await;
        }
    });

    let shared_wallet_balances: SharedWallets = init_paymaster_wallets(&config.clone());
    tokio::spawn({
        let config_clone = Arc::clone(&config.clone());
        let wallet_balances_clone = Arc::clone(&shared_wallet_balances);
        async move {
            fetch_balances_task(wallet_balances_clone, &config_clone).await;
        }
    });

    // Activity monitoring
    let activity_monitoring_config = ActivityMonitoringConfig::new();
    let activity_stats = ActivityStats::with_config(&activity_monitoring_config);
    // Shared state for activity stats
    let shared_activity_stats = Arc::new(RwLock::new(activity_stats));
    tokio::spawn({
        let activity_stats_clone = Arc::clone(&shared_activity_stats);
        async move {
            activity_monitoring_task(activity_stats_clone, &activity_monitoring_config).await;
        }
    });

    // bridge monitoring
    let bridge_monitoring_config = BridgeMonitoringConfig::new();
    // Shared state for bridge status
    let bridge_state = SharedBridgeState::default();
    tokio::spawn({
        let bridge_state_clone = Arc::clone(&bridge_state);
        async move {
            bridge_monitoring_task(bridge_state_clone, &bridge_monitoring_config).await;
        }
    });

    let app = Router::new()
        .route(
            "/api/network_status",
            get(move || get_network_status(Arc::clone(&shared_network_status))),
        )
        .route(
            "/api/wallet_balances",
            get(move || get_wallets_with_balances(Arc::clone(&shared_wallet_balances))),
        )
        .route(
            "/api/bridge_status",
            get(move || get_bridge_status(Arc::clone(&bridge_state))),
        )
        .route(
            "/api/activity_stats",
            get(move || get_activity_stats(Arc::clone(&shared_activity_stats))),
        )
        .layer(cors);

    let addr = SocketAddr::from(([0, 0, 0, 0], 3000));
    info!(%addr, "Server running at http://");

    let listener = TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
