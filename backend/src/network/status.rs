use axum::Json;
use jsonrpsee::core::client::ClientT;
use jsonrpsee::http_client::HttpClient;
use std::sync::Arc;
use tokio::{
    sync::RwLock,
    time::{interval, sleep, Duration},
};
use tracing::{error, info};

use super::types::{NetworkStatus, Status};
use crate::{
    config::NetworkMonitoringConfig,
    utils::{retry_policy::ExponentialBackoff, rpc_client::create_rpc_client},
};

/// Shared Network State
pub(crate) type SharedNetworkState = Arc<RwLock<NetworkStatus>>;

/// Calls `strata_syncStatus` using `jsonrpsee`
async fn call_rpc_status(
    config: &NetworkMonitoringConfig,
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
async fn check_bundler_health(
    client: &reqwest::Client,
    config: &NetworkMonitoringConfig,
) -> Status {
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
pub(crate) async fn fetch_statuses_task(
    state: SharedNetworkState,
    config: &NetworkMonitoringConfig,
) {
    info!("Fetching statuses...");
    let mut interval = interval(Duration::from_secs(config.status_refetch_interval()));
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
pub(crate) async fn get_network_status(state: SharedNetworkState) -> Json<NetworkStatus> {
    let data = state.read().await.clone();
    Json(data)
}
