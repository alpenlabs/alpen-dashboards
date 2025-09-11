use axum::Json;
use jsonrpsee::core::client::ClientT;
use jsonrpsee::http_client::HttpClient;
use std::sync::Arc;
use tokio::time::{interval, sleep, Duration};
use tracing::{error, info};

use super::types::{NetworkMonitoringContext, NetworkStatus, Status};
use crate::{
    config::NetworkMonitoringConfig,
    utils::{retry_policy::ExponentialBackoff, rpc_client::create_rpc_client},
};

/// Calls `strata_syncStatus` using `jsonrpsee`
async fn call_rpc_status(client: &HttpClient, retry_policy: ExponentialBackoff) -> Status {
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
                if retry_count < retry_policy.max_retries() {
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
pub(crate) async fn fetch_statuses_task(context: Arc<NetworkMonitoringContext>) {
    info!("Fetching statuses...");
    let mut interval = interval(Duration::from_secs(
        context.config.status_refetch_interval(),
    ));
    let sequencer_client = create_rpc_client(context.config.sequencer_url());
    let rpc_client = create_rpc_client(context.config.rpc_url());
    let http_client = reqwest::Client::new();

    loop {
        interval.tick().await;

        let sequencer =
            call_rpc_status(&sequencer_client, context.config.sequencer_retry_policy()).await;
        let rpc_endpoint = call_rpc_status(&rpc_client, context.config.rpc_retry_policy()).await;
        let bundler_endpoint = check_bundler_health(&http_client, &context.config).await;

        let new_status = NetworkStatus {
            sequencer,
            rpc_endpoint,
            bundler_endpoint,
        };

        info!(?new_status, "Updated Status");

        let mut locked_status = context.network_status.write().await;
        *locked_status = new_status;

        if !context
            .status_available
            .load(std::sync::atomic::Ordering::Acquire)
        {
            context
                .status_available
                .store(true, std::sync::atomic::Ordering::Release);
            context.initial_status_query_complete.notify_waiters();
        }
    }
}

/// Handler to get the current network status
pub(crate) async fn get_network_status(
    context: Arc<NetworkMonitoringContext>,
) -> Json<NetworkStatus> {
    // Wait for initial status query to complete if not yet available
    if !context
        .status_available
        .load(std::sync::atomic::Ordering::Acquire)
    {
        info!("Waiting for initial network status query to complete");
        context.initial_status_query_complete.notified().await;
    }

    let data = context.network_status.read().await.clone();
    Json(data)
}
