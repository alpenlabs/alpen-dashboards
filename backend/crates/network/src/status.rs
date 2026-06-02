use std::fmt;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use axum::http::StatusCode;
use axum::Json;
use jsonrpsee::core::client::ClientT;
use jsonrpsee::http_client::HttpClient;
use status_config::NetworkMonitoringConfig;
use status_utils::{create_rpc_client, ExponentialBackoff};
use strata_tasks::ShutdownGuard;
use tokio::time::{interval, sleep, timeout, Duration};
use tracing::{debug, error, info, warn};

use super::types::{
    BlockInfoStatus, EpochCommitmentStatus, EvmChainStatus, NetworkMonitoringContext,
    NetworkStatus, OlChainStatus, Status,
};

const STRATA_CHAIN_STATUS_METHOD: &str = "strata_getChainStatus";
const ETH_BLOCK_NUMBER_METHOD: &str = "eth_blockNumber";

#[derive(Debug, PartialEq)]
enum ChainStatusParseError {
    MissingField(&'static str),
    InvalidField(&'static str),
    InvalidSlotOrder {
        tip: u64,
        confirmed: u64,
        finalized: u64,
    },
}

impl fmt::Display for ChainStatusParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingField(field) => write!(f, "missing field `{field}`"),
            Self::InvalidField(field) => write!(f, "invalid field `{field}`"),
            Self::InvalidSlotOrder {
                tip,
                confirmed,
                finalized,
            } => write!(
                f,
                "invalid slot order: tip={tip}, confirmed={confirmed}, finalized={finalized}"
            ),
        }
    }
}

#[derive(Debug, PartialEq)]
enum EvmStatusParseError {
    InvalidBlockNumber(String),
}

impl fmt::Display for EvmStatusParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBlockNumber(value) => write!(f, "invalid eth block number `{value}`"),
        }
    }
}

#[derive(Debug, Default)]
struct ProgressTracker {
    last_value: Option<u64>,
    last_changed_at: Option<Instant>,
}

impl ProgressTracker {
    fn observe(&mut self, value: u64, now: Instant) -> u64 {
        if self.last_value != Some(value) {
            self.last_value = Some(value);
            self.last_changed_at = Some(now);
            return 0;
        }

        self.last_changed_at
            .map(|changed_at| now.saturating_duration_since(changed_at).as_secs())
            .unwrap_or(0)
    }
}

fn read_u64(json: &serde_json::Value, field: &'static str) -> Result<u64, ChainStatusParseError> {
    json.get(field)
        .and_then(serde_json::Value::as_u64)
        .ok_or(ChainStatusParseError::InvalidField(field))
}

fn read_u32(json: &serde_json::Value, field: &'static str) -> Result<u32, ChainStatusParseError> {
    let value = read_u64(json, field)?;
    u32::try_from(value).map_err(|_| ChainStatusParseError::InvalidField(field))
}

fn read_string(
    json: &serde_json::Value,
    field: &'static str,
) -> Result<String, ChainStatusParseError> {
    json.get(field)
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or(ChainStatusParseError::InvalidField(field))
}

fn read_bool(json: &serde_json::Value, field: &'static str) -> Result<bool, ChainStatusParseError> {
    json.get(field)
        .and_then(serde_json::Value::as_bool)
        .ok_or(ChainStatusParseError::InvalidField(field))
}

fn read_object<'a>(
    json: &'a serde_json::Value,
    field: &'static str,
) -> Result<&'a serde_json::Value, ChainStatusParseError> {
    json.get(field)
        .ok_or(ChainStatusParseError::MissingField(field))
        .and_then(|value| {
            if value.is_object() {
                Ok(value)
            } else {
                Err(ChainStatusParseError::InvalidField(field))
            }
        })
}

fn parse_block_info(json: &serde_json::Value) -> Result<BlockInfoStatus, ChainStatusParseError> {
    Ok(BlockInfoStatus::new(
        read_u64(json, "slot")?,
        read_string(json, "blkid")?,
        read_u32(json, "epoch")?,
        read_bool(json, "is_terminal")?,
    ))
}

fn parse_legacy_block_info(
    json: &serde_json::Value,
) -> Result<BlockInfoStatus, ChainStatusParseError> {
    Ok(BlockInfoStatus::new(
        read_u64(json, "slot")?,
        read_string(json, "blkid")?,
        0,
        false,
    ))
}

fn parse_epoch_commitment(
    json: &serde_json::Value,
) -> Result<EpochCommitmentStatus, ChainStatusParseError> {
    Ok(EpochCommitmentStatus::new(
        read_u32(json, "epoch")?,
        read_u64(json, "last_slot")?,
        read_string(json, "last_blkid")?,
    ))
}

fn parse_ol_chain_status_response(
    json: &serde_json::Value,
) -> Result<OlChainStatus, ChainStatusParseError> {
    let (tip, latest) = match json.get("tip") {
        Some(value) if value.is_object() => {
            let tip = parse_block_info(value)?;
            let latest = parse_epoch_commitment(read_object(json, "latest")?)?;
            (tip, latest)
        }
        Some(_) => return Err(ChainStatusParseError::InvalidField("tip")),
        None => match json.get("latest") {
            Some(value) if value.is_object() => {
                let tip = parse_legacy_block_info(value)?;
                let latest = parse_epoch_commitment(read_object(json, "parent")?)?;
                (tip, latest)
            }
            Some(_) => return Err(ChainStatusParseError::InvalidField("latest")),
            None => return Err(ChainStatusParseError::MissingField("tip")),
        },
    };
    let confirmed = parse_epoch_commitment(read_object(json, "confirmed")?)?;
    let finalized = parse_epoch_commitment(read_object(json, "finalized")?)?;

    if finalized.last_slot() > confirmed.last_slot() || confirmed.last_slot() > tip.slot() {
        return Err(ChainStatusParseError::InvalidSlotOrder {
            tip: tip.slot(),
            confirmed: confirmed.last_slot(),
            finalized: finalized.last_slot(),
        });
    }

    Ok(OlChainStatus::new(tip, latest, confirmed, finalized))
}

fn parse_eth_block_number(raw: &str) -> Result<u64, EvmStatusParseError> {
    let stripped = raw.strip_prefix("0x").unwrap_or(raw);
    u64::from_str_radix(stripped, 16)
        .map_err(|_| EvmStatusParseError::InvalidBlockNumber(raw.to_owned()))
}

/// Calls `strata_getChainStatus` using `jsonrpsee`.
async fn call_ol_chain_status(
    client: &HttpClient,
    retry_policy: ExponentialBackoff,
    endpoint_name: &'static str,
) -> (Status, Option<OlChainStatus>) {
    let mut retry_count: u64 = 0;

    loop {
        let response: Result<serde_json::Value, _> = client
            .request(STRATA_CHAIN_STATUS_METHOD, Vec::<()>::new())
            .await;
        match response {
            Ok(json) => {
                debug!(
                    ?json,
                    method = STRATA_CHAIN_STATUS_METHOD,
                    endpoint = endpoint_name,
                    "rpc response"
                );
                match parse_ol_chain_status_response(&json) {
                    Ok(chain_status) => return (Status::Online, Some(chain_status)),
                    Err(e) => {
                        warn!(
                            error = %e,
                            method = STRATA_CHAIN_STATUS_METHOD,
                            endpoint = endpoint_name,
                            "could not parse OL chain status response"
                        );
                        return (Status::Offline, None);
                    }
                }
            }
            Err(e) => {
                if retry_count < retry_policy.max_retries() {
                    let delay_seconds = retry_policy.get_delay(retry_count);
                    if delay_seconds > 0 {
                        info!(
                            delay_seconds,
                            retry_count,
                            method = STRATA_CHAIN_STATUS_METHOD,
                            endpoint = endpoint_name,
                            "retrying rpc status request"
                        );
                        sleep(Duration::from_secs(delay_seconds)).await;
                    }
                    retry_count += 1;
                } else {
                    error!(
                        error = %e,
                        method = STRATA_CHAIN_STATUS_METHOD,
                        endpoint = endpoint_name,
                        "could not get OL chain status"
                    );
                    return (Status::Offline, None);
                }
            }
        }
    }
}

/// Calls `eth_blockNumber` using `jsonrpsee`.
async fn call_evm_chain_status(
    client: &HttpClient,
    retry_policy: ExponentialBackoff,
) -> (Status, Option<EvmChainStatus>) {
    let mut retry_count: u64 = 0;

    loop {
        let response: Result<String, _> = client
            .request(ETH_BLOCK_NUMBER_METHOD, Vec::<()>::new())
            .await;
        match response {
            Ok(raw) => match parse_eth_block_number(&raw) {
                Ok(block_number) => {
                    return (Status::Online, Some(EvmChainStatus::new(block_number)));
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        method = ETH_BLOCK_NUMBER_METHOD,
                        "could not parse EVM chain status response"
                    );
                    return (Status::Offline, None);
                }
            },
            Err(e) => {
                if retry_count < retry_policy.max_retries() {
                    let delay_seconds = retry_policy.get_delay(retry_count);
                    if delay_seconds > 0 {
                        info!(
                            delay_seconds,
                            retry_count,
                            method = ETH_BLOCK_NUMBER_METHOD,
                            "retrying EVM rpc status request"
                        );
                        sleep(Duration::from_secs(delay_seconds)).await;
                    }
                    retry_count += 1;
                } else {
                    error!(
                        error = %e,
                        method = ETH_BLOCK_NUMBER_METHOD,
                        "could not get EVM chain status"
                    );
                    return (Status::Offline, None);
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
    match client.get(url).send().await {
        Ok(resp) => {
            let status = resp.status();
            match resp.text().await {
                Ok(body) if body.contains("ok") => Status::Online,
                Ok(_) => {
                    warn!(
                        url,
                        %status,
                        "bundler health response did not contain expected status"
                    );
                    Status::Offline
                }
                Err(e) => {
                    error!(url, error = %e, "could not read bundler health response body");
                    Status::Offline
                }
            }
        }
        Err(e) => {
            error!(url, error = %e, "could not query bundler health endpoint");
            Status::Offline
        }
    }
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
    let eth_rpc_client = create_rpc_client(context.config().eth_rpc_url());
    let http_client = reqwest::Client::new();
    let mut sequencer_ol_progress = ProgressTracker::default();
    let mut rpc_ol_progress = ProgressTracker::default();
    let mut ee_progress = ProgressTracker::default();

    loop {
        tokio::select! {
            _ = shutdown.wait_for_shutdown() => break,
            _ = interval.tick() => {}
        }

        let (sequencer, mut sequencer_chain) = call_ol_chain_status(
            &sequencer_client,
            context.config().sequencer_retry_policy(),
            "sequencer",
        )
        .await;
        let (rpc_endpoint, mut rpc_chain) = call_ol_chain_status(
            &rpc_client,
            context.config().rpc_retry_policy(),
            "rpc_endpoint",
        )
        .await;
        let (ee_endpoint, mut ee_chain) =
            call_evm_chain_status(&eth_rpc_client, context.config().rpc_retry_policy()).await;
        let bundler_endpoint = check_bundler_health(&http_client, context.config()).await;

        let now = Instant::now();
        if let Some(chain) = &mut sequencer_chain {
            chain.set_latest_slot_stale_seconds(Some(
                sequencer_ol_progress.observe(chain.latest_slot(), now),
            ));
        }
        if let Some(chain) = &mut rpc_chain {
            chain.set_latest_slot_stale_seconds(Some(
                rpc_ol_progress.observe(chain.latest_slot(), now),
            ));
        }
        if let Some(chain) = &mut ee_chain {
            chain.set_latest_block_stale_seconds(Some(
                ee_progress.observe(chain.latest_block_number(), now),
            ));
        }

        let new_status = NetworkStatus::new(
            sequencer,
            rpc_endpoint,
            ee_endpoint,
            bundler_endpoint,
            sequencer_chain,
            rpc_chain,
            ee_chain,
        );

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
    use std::time::Duration;

    use serde_json::json;

    use super::{
        parse_eth_block_number, parse_ol_chain_status_response, ChainStatusParseError,
        ProgressTracker,
    };

    fn block_id(seed: u8) -> String {
        format!("{seed:02x}").repeat(32)
    }

    fn chain_status_json(latest: u64, confirmed: u64, finalized: u64) -> serde_json::Value {
        json!({
            "tip": {
                "slot": latest,
                "blkid": block_id(1),
                "epoch": 12,
                "is_terminal": false
            },
            "latest": {
                "epoch": 12,
                "last_slot": latest.saturating_sub(1),
                "last_blkid": block_id(2)
            },
            "confirmed": {
                "epoch": 12,
                "last_slot": confirmed,
                "last_blkid": block_id(3)
            },
            "finalized": {
                "epoch": 10,
                "last_slot": finalized,
                "last_blkid": block_id(4)
            }
        })
    }

    #[test]
    fn parses_current_ol_chain_status_response() {
        let parsed = parse_ol_chain_status_response(&chain_status_json(120, 100, 80))
            .expect("current chain status shape should parse");
        let serialized = serde_json::to_value(parsed).expect("status should serialize");

        assert_eq!(serialized["tip"]["slot"], 120);
        assert_eq!(serialized["tip"]["epoch"], 12);
        assert_eq!(serialized["latest"]["last_slot"], 119);
        assert_eq!(serialized["confirmed"]["last_slot"], 100);
        assert_eq!(serialized["finalized"]["last_slot"], 80);
        assert_eq!(serialized["confirmation_lag_slots"], 20);
        assert_eq!(serialized["finality_lag_slots"], 40);
    }

    #[test]
    fn parses_legacy_ol_chain_status_response() {
        let parsed = parse_ol_chain_status_response(&json!({
            "latest": {
                "slot": 120,
                "blkid": block_id(1)
            },
            "parent": {
                "epoch": 12,
                "last_slot": 110,
                "last_blkid": block_id(2)
            },
            "confirmed": {
                "epoch": 12,
                "last_slot": 100,
                "last_blkid": block_id(3)
            },
            "finalized": {
                "epoch": 10,
                "last_slot": 80,
                "last_blkid": block_id(4)
            }
        }))
        .expect("legacy chain status shape should parse");
        let serialized = serde_json::to_value(parsed).expect("status should serialize");

        assert_eq!(serialized["tip"]["slot"], 120);
        assert_eq!(serialized["latest"]["last_slot"], 110);
        assert_eq!(serialized["confirmed"]["last_slot"], 100);
        assert_eq!(serialized["finalized"]["last_slot"], 80);
    }

    #[test]
    fn rejects_ol_chain_status_without_tip_or_latest() {
        let err = parse_ol_chain_status_response(&json!({
            "parent": {},
            "confirmed": {},
            "finalized": {}
        }))
        .expect_err("missing tip/latest status must fail");

        assert_eq!(err, ChainStatusParseError::MissingField("tip"));
    }

    #[test]
    fn rejects_invalid_ol_slot_order() {
        let err = parse_ol_chain_status_response(&chain_status_json(100, 120, 80))
            .expect_err("confirmed slot cannot exceed latest slot");

        assert_eq!(
            err,
            ChainStatusParseError::InvalidSlotOrder {
                tip: 100,
                confirmed: 120,
                finalized: 80,
            }
        );
    }

    #[test]
    fn parses_eth_block_number_hex() {
        assert_eq!(parse_eth_block_number("0x10").unwrap(), 16);
        assert_eq!(parse_eth_block_number("10").unwrap(), 16);
    }

    #[test]
    fn rejects_invalid_eth_block_number_hex() {
        assert!(parse_eth_block_number("nope").is_err());
    }

    #[test]
    fn progress_tracker_resets_when_value_changes() {
        let mut tracker = ProgressTracker::default();
        let start = std::time::Instant::now();

        assert_eq!(tracker.observe(10, start), 0);
        assert_eq!(tracker.observe(10, start + Duration::from_secs(3)), 3);
        assert_eq!(tracker.observe(11, start + Duration::from_secs(4)), 0);
    }
}
