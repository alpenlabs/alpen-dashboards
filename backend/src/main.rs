mod bridge;
mod config;
mod retry_policy;
mod utils;

use axum::{routing::get, Json, Router};
use dotenvy::dotenv;
use jsonrpsee::core::client::ClientT;
use jsonrpsee::http_client::HttpClient;
use serde::{Deserialize, Serialize};
use std::{net::SocketAddr, sync::Arc};
use tokio::{
    net::TcpListener,
    sync::RwLock,
    time::{interval, sleep, Duration},
};
use tower_http::cors::{Any, CorsLayer};
use tracing::{error, info};

use crate::{
    bridge::{bridge_monitoring_task, get_bridge_status, SharedBridgeState},
    config::{BridgeMonitoringConfig, NetworkConfig},
    retry_policy::ExponentialBackoff,
    utils::create_rpc_client,
};

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "lowercase")]
enum Status {
    Online,
    Offline,
}

#[derive(Serialize, Clone, Debug)]
struct NetworkStatus {
    sequencer: Status,
    rpc_endpoint: Status,
    bundler_endpoint: Status,
}

/// Shared Network State
type SharedNetworkState = Arc<RwLock<NetworkStatus>>;

/// Calls `strata_syncStatus` using `jsonrpsee`
async fn call_rpc_status(
    config: &NetworkConfig,
    client: &HttpClient,
    retry_policy: ExponentialBackoff,
) -> Status {
    let mut retry_count: u64 = 0;

    loop {
        let response: Result<serde_json::Value, _> =
            client.request("strata_syncStatus", Vec::<()>::new()).await;
        match response {
            Ok(json) => {
                info!(?json, "RPC Response");
                if json.get("tip_height").is_some() {
                    return Status::Online;
                } else {
                    return Status::Offline;
                }
            }
            Err(e) => {
                if retry_count < config.max_retries() {
                    let delay_seconds = retry_policy.get_delay(retry_count);
                    if delay_seconds > 0 {
                        info!(?delay_seconds, "Retrying `strata_syncStatus` after");
                        sleep(Duration::from_secs(delay_seconds)).await;
                    }
                    retry_count += 1;
                } else {
                    error!(error = %e, "Could not get status");
                    return Status::Offline;
                }
            }
        }
    }
}

/// Checks bundler health (`/health`)
async fn check_bundler_health(client: &reqwest::Client, config: &NetworkConfig) -> Status {
    let url = config.bundler_url();
    if let Ok(resp) = client.get(url).send().await {
        let body = resp.text().await.unwrap_or_default();
        if body.contains("ok") {
            return Status::Online;
        }
    }
    Status::Offline
}

/// Periodically fetches real statuses
async fn fetch_statuses_task(state: SharedNetworkState, config: &NetworkConfig) {
    info!("Fetching statuses...");
    let mut interval = interval(Duration::from_secs(10));
    let sequencer_client = create_rpc_client(config.sequencer_url());
    let rpc_client = create_rpc_client(config.rpc_url());
    let http_client = reqwest::Client::new();
    let retry_policy =
        ExponentialBackoff::new(config.max_retries(), config.total_retry_time(), 1.5);

    loop {
        interval.tick().await;

        let sequencer = call_rpc_status(config, &sequencer_client, retry_policy).await;
        let rpc_endpoint = call_rpc_status(config, &rpc_client, retry_policy).await;
        let bundler_endpoint = check_bundler_health(&http_client, config).await;

        let new_status = NetworkStatus {
            sequencer,
            rpc_endpoint,
            bundler_endpoint,
        };

        info!(?new_status, "Updated Status");

        let mut locked_state = state.write().await;
        *locked_state = new_status;
    }
}

/// Handler to get the current network status
async fn get_network_status(state: SharedNetworkState) -> Json<NetworkStatus> {
    let data = state.read().await.clone();
    Json(data)
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    dotenv().ok();

    let config = Arc::new(config::NetworkConfig::new());

    let cors = CorsLayer::new().allow_origin(Any);

    // Shared state for network status
    let shared_state = Arc::new(RwLock::new(NetworkStatus {
        sequencer: Status::Offline, // Default state
        rpc_endpoint: Status::Offline,
        bundler_endpoint: Status::Offline,
    }));

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
            "/api/status",
            get(move || get_network_status(Arc::clone(&shared_state))),
        )
        .route(
            "/api/bridge_status",
            get(move || get_bridge_status(Arc::clone(&bridge_state))),
        )
        .layer(cors);

    let addr = SocketAddr::from(([0, 0, 0, 0], 3000));
    info!(%addr, "Server running at http://");

    let listener = TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
