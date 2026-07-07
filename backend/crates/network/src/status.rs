use anyhow::Result;
use axum::http::StatusCode;
use axum::Json;
use jsonrpsee::core::client::ClientT;
use jsonrpsee::http_client::HttpClient;
use std::sync::Arc;
use strata_tasks::ShutdownGuard;
use tokio::time::{interval, sleep, timeout, Duration};
use tracing::{error, info};

use super::types::{NetworkMonitoringContext, NetworkStatus, Status};
use status_config::NetworkMonitoringConfig;
use status_utils::{create_rpc_client, ExponentialBackoff};

const STRATA_CHAIN_STATUS_METHOD: &str = "strata_getChainStatus";
const ETH_BLOCK_NUMBER_METHOD: &str = "eth_blockNumber";

fn is_chain_status_response_online(json: &serde_json::Value) -> bool {
    json.get("tip").is_some()
}

fn is_eth_block_number_response_online(json: &serde_json::Value) -> bool {
    json.as_str()
        .and_then(|block_number| block_number.strip_prefix("0x"))
        .and_then(|block_number| u64::from_str_radix(block_number, 16).ok())
        .is_some()
}

async fn call_json_rpc_status(
    client: &HttpClient,
    method: &'static str,
    is_online: impl Fn(&serde_json::Value) -> bool,
    retry_policy: ExponentialBackoff,
) -> Status {
    let mut retry_count: u64 = 0;

    loop {
        let response: Result<serde_json::Value, _> = client.request(method, Vec::<()>::new()).await;
        match response {
            Ok(json) => {
                info!(?json, method, "rpc response");
                if is_online(&json) {
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
                            retry_count, method, "retrying rpc status request"
                        );
                        sleep(Duration::from_secs(delay_seconds)).await;
                    }
                    retry_count += 1;
                } else {
                    error!(
                        error = %e,
                        method,
                        "could not get network status"
                    );
                    return Status::Offline;
                }
            }
        }
    }
}

/// Calls the OL `strata_getChainStatus` method.
async fn call_sequencer_status(client: &HttpClient, retry_policy: ExponentialBackoff) -> Status {
    call_json_rpc_status(
        client,
        STRATA_CHAIN_STATUS_METHOD,
        is_chain_status_response_online,
        retry_policy,
    )
    .await
}

/// Calls the EVM `eth_blockNumber` method.
async fn call_rpc_endpoint_status(client: &HttpClient, retry_policy: ExponentialBackoff) -> Status {
    call_json_rpc_status(
        client,
        ETH_BLOCK_NUMBER_METHOD,
        is_eth_block_number_response_online,
        retry_policy,
    )
    .await
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

/// Periodically polls configured network service endpoints and updates status state.
pub async fn network_monitoring_task(
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
            call_sequencer_status(&sequencer_client, context.config().sequencer_retry_policy())
                .await;
        let rpc_endpoint =
            call_rpc_endpoint_status(&rpc_client, context.config().rpc_retry_policy()).await;
        let bundler_endpoint = check_bundler_health(&http_client, context.config()).await;

        let new_status = NetworkStatus::new(sequencer, rpc_endpoint, bundler_endpoint);

        info!(?new_status, "updated network status");

        context.set_status(new_status).await;
        context.mark_status_available();
    }

    Ok(())
}

/// Handler to get the current network status
pub async fn get_network_status(
    context: Arc<NetworkMonitoringContext>,
) -> std::result::Result<Json<NetworkStatus>, StatusCode> {
    let initial_status_wait_timeout = context.initial_status_wait_timeout();
    if timeout(
        initial_status_wait_timeout,
        context.wait_until_initial_status(),
    )
    .await
    .is_err()
    {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }

    Ok(Json(context.status().await))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{is_chain_status_response_online, is_eth_block_number_response_online};

    #[test]
    fn chain_status_response_with_tip_is_online() {
        assert!(is_chain_status_response_online(&json!({
            "tip": {
                "blkid": "0x00",
                "slot": 1,
                "epoch": 0,
                "is_terminal": false
            },
            "confirmed": {},
            "finalized": {},
            "latest": {}
        })));
    }

    #[test]
    fn chain_status_response_without_tip_is_offline() {
        assert!(!is_chain_status_response_online(&json!({
            "confirmed": {},
            "finalized": {},
            "latest": {}
        })));
    }

    #[test]
    fn eth_block_number_response_with_hex_string_is_online() {
        assert!(is_eth_block_number_response_online(&json!("0x1a")));
    }

    #[test]
    fn eth_block_number_response_with_non_hex_string_is_offline() {
        assert!(!is_eth_block_number_response_online(&json!("latest")));
    }

    #[test]
    fn eth_block_number_response_with_object_is_offline() {
        assert!(!is_eth_block_number_response_online(&json!({
            "blockNumber": "0x1a"
        })));
    }
}
