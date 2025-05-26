use std::{sync::Arc, time::Duration};

use axum::Json;
use jsonrpsee::core::client::ClientT;
use jsonrpsee::http_client::HttpClient;
use tokio::{sync::RwLock, time::interval};
use tracing::{error, info};

use crate::{
    config::NetworkConfig,
    network::types::{NetworkStatus, Status},
    utils::{retry_policy::ExponentialBackoff, rpc_client::create_rpc_client},
};

/// Shared state type used across monitoring and handlers.
pub(crate) type SharedNetworkState = Arc<RwLock<NetworkStatus>>;

/// Check the status of batch producer and rpc endpoint.
async fn fetch_rpc_status(
    config: &NetworkConfig,
    client: &HttpClient,
    retry_policy: ExponentialBackoff,
) -> Status {
    let mut retries = 0;

    loop {
        let response: Result<serde_json::Value, _> =
            client.request("strata_syncStatus", Vec::<()>::new()).await;
        match response {
            Ok(resp) => {
                if resp.get("tip_height").is_some() {
                    return Status::Online;
                } else {
                    return Status::Offline;
                }
            }
            Err(err) => {
                if retries < config.max_retries() {
                    let delay = retry_policy.get_delay(retries);
                    info!(%delay, "Retrying strata_syncStatus...");
                    tokio::time::sleep(Duration::from_secs(delay)).await;
                    retries += 1;
                } else {
                    error!(%err, "Failed to fetch strata_syncStatus");
                    return Status::Offline;
                }
            }
        }
    }
}

/// Checks the health of bundler endpoint.
async fn fetch_bundler_endpoint_status(http: &reqwest::Client, config: &NetworkConfig) -> Status {
    match http.get(config.bundler_url()).send().await {
        Ok(resp) => match resp.text().await {
            Ok(body) if body.contains("ok") => Status::Online,
            _ => Status::Offline,
        },
        Err(err) => {
            error!(%err, "Failed to reach bundler endpoint");
            Status::Offline
        }
    }
}

/// Background task that periodically updates shared network status.
pub(crate) async fn monitor_network_task(state: SharedNetworkState, config: &NetworkConfig) {
    let rpc_client = create_rpc_client(config.rpc_url());
    let http_client = reqwest::Client::new();
    let mut interval = interval(Duration::from_secs(config.status_refetch_interval()));

    loop {
        interval.tick().await;
        let retry_policy =
            ExponentialBackoff::new(config.max_retries(), config.total_retry_time(), 1.5);
        let rpc_status = fetch_rpc_status(config, &rpc_client, retry_policy).await;
        let batch_producer_status = rpc_status;
        let bundler_status = fetch_bundler_endpoint_status(&http_client, config).await;

        let current_status =
            NetworkStatus::new(batch_producer_status, rpc_status, bundler_status);

        info!(?current_status, "Updated node service status");

        let mut locked = state.write().await;
        *locked = current_status;
    }
}

/// HTTP handler for GET `/api/network_status`
pub(crate) async fn get_network_status(state: SharedNetworkState) -> Json<NetworkStatus> {
    let locked = state.read().await;
    Json(locked.clone())
}
