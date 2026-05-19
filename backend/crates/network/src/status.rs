use anyhow::Result;
use axum::Json;
use jsonrpsee::core::client::ClientT;
use jsonrpsee::http_client::HttpClient;
use std::sync::Arc;
use strata_tasks::ShutdownGuard;
use tokio::time::{interval, sleep, Duration};
use tracing::{error, info};

use super::types::{NetworkMonitoringContext, NetworkStatus, Status};
use status_config::NetworkMonitoringConfig;
use status_utils::{create_rpc_client, ExponentialBackoff};

/// Calls `strata_syncStatus` using `jsonrpsee`
async fn call_rpc_status(client: &HttpClient, retry_policy: ExponentialBackoff) -> Status {
    let mut retry_count: u64 = 0;

    loop {
        let response: Result<serde_json::Value, _> =
            client.request("strata_syncStatus", Vec::<()>::new()).await;
        match response {
            Ok(json) => {
                info!(?json, method = "strata_syncStatus", "rpc response");
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
                        info!(
                            delay_seconds,
                            retry_count,
                            method = "strata_syncStatus",
                            "retrying rpc status request"
                        );
                        sleep(Duration::from_secs(delay_seconds)).await;
                    }
                    retry_count += 1;
                } else {
                    error!(
                        error = %e,
                        method = "strata_syncStatus",
                        "could not get network status"
                    );
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
pub async fn fetch_statuses_task(
    context: Arc<NetworkMonitoringContext>,
    shutdown: ShutdownGuard,
) -> Result<()> {
    info!("fetching network statuses");
    let mut interval = interval(Duration::from_secs(
        context.config().status_refetch_interval(),
    ));
    let sequencer_client = create_rpc_client(context.config().sequencer_url());
    let rpc_client = create_rpc_client(context.config().rpc_url());
    let http_client = reqwest::Client::new();

    loop {
        tokio::select! {
            _ = shutdown.wait_for_shutdown() => break,
            _ = interval.tick() => {}
        }

        let sequencer =
            call_rpc_status(&sequencer_client, context.config().sequencer_retry_policy()).await;
        let rpc_endpoint = call_rpc_status(&rpc_client, context.config().rpc_retry_policy()).await;
        let bundler_endpoint = check_bundler_health(&http_client, context.config()).await;

        let new_status = NetworkStatus::new(sequencer, rpc_endpoint, bundler_endpoint);

        info!(?new_status, "updated network status");

        context.set_status(new_status).await;
        context.mark_status_available();
    }

    Ok(())
}

/// Handler to get the current network status
pub async fn get_network_status(context: Arc<NetworkMonitoringContext>) -> Json<NetworkStatus> {
    context.wait_until_status_available().await;

    Json(context.status().await)
}
