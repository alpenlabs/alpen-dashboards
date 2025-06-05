use alloy_primitives::{Address, B256, Log, LogData};
use alloy_primitives::keccak256;
use alloy_sol_types::SolEvent;
use alpen_reth_primitives::{WithdrawalIntentEvent};
use bitcoin_bosd::Descriptor;
use chrono::Utc;
use hex_literal::hex;
use once_cell::sync::Lazy;
use serde_json::json;
use std::time::Duration;
use sqlx::SqlitePool;
use tokio::time::interval;
use tracing::{info, warn};

use crate::config::BridgeMonitoringConfig;
use crate::indexer::db::{insert_withdrawal_request, get_withdrawal_request_by_txid,
    get_indexer_state, update_indexer_state};
use crate::indexer::types::{DecodedWithdrawalIntent, LogEntry, IndexerTaskId,
    DbTxid, DbAmount, DbDescriptor, DbBlockNumber, DbTaskId, DbTimestamp};
use crate::indexer::models::{WithdrawalRequest};

/// BridgeOut contract address (EVM address)
pub static BRIDGEOUT_ADDRESS: Lazy<Address> =
    Lazy::new(|| Address::from(hex!("5400000000000000000000000000000000000001")));

/// WithdrawalIntentEvent topic selector (keccak256 hash of event signature)
pub static WITHDRAWAL_EVENT_SIG: Lazy<B256> = Lazy::new(|| keccak256(WithdrawalIntentEvent::SIGNATURE.as_bytes()));

/// Withdrawal requests indexer task
pub async fn run_withdrawal_requests_task(pool: &SqlitePool, config: &BridgeMonitoringConfig) {
    info!("Running withdrawal requests indexer...");
    let mut interval = interval(Duration::from_secs(10));
    let rpc_client = reqwest::Client::new();

    loop {
        interval.tick().await;

        let task_id = IndexerTaskId::WithdrawalRequests;
        // Step 1: Load the current indexer state from the DB
        let mut state = match get_indexer_state(pool, &DbTaskId::from(task_id)).await {
            Ok(s) => s,
            Err(e) => {
                warn!(?e, "Indexer state not found for task_id={task_id}");
                return;
            }
        };

        let from_block = *state.last_scanned_block as u64;
        let to_block = from_block + config.eth_logs_batch_size();

        let logs = fetch_withdrawal_logs(
            &rpc_client,
            &config.strata_rpc_url(),
            from_block,
            to_block,
            *BRIDGEOUT_ADDRESS,
            *WITHDRAWAL_EVENT_SIG,
        )
        .await.unwrap();

        // Step 3: Parse logs, decode, and insert into DB
        let mut inserted = 0;
        for entry in logs {
            if entry.topics.get(0) != Some(&WITHDRAWAL_EVENT_SIG) {
                continue;
            }

            match decode_log_entry(&entry) {
                Ok(decoded) => {
                    let record = WithdrawalRequest {
                        txid: DbTxid::from(decoded.txid),
                        amount: DbAmount::from(decoded.amount),
                        destination: DbDescriptor::from(decoded.destination),
                        block_number: DbBlockNumber::try_from(decoded.block_number).unwrap(),
                        timestamp: DbTimestamp::from(Utc::now().naive_utc()),
                    };
                    info!(?record, "record to insert in indexer db");
                    if let Err(e) = insert_withdrawal_request(pool, &record).await{
                        warn!(?e, "Failed to insert withdrawal request into database");
                        continue;
                    };
                    inserted += 1;

                    // Check inserted record exists
                    let fetched = get_withdrawal_request_by_txid(pool, &record.txid).await;
                    info!(?fetched, "record to fetched from indexer db");
                }
                Err(err) => {
                    warn!(?err, "Failed to decode withdrawal log");
                }
            }
        }

        state.last_scanned_block = DbBlockNumber::try_from(to_block).unwrap();
        // Step 4: Update indexer state
        if let Err(e) = update_indexer_state(pool, &state).await{
            warn!(?e, "Failed to update indexer state");
            continue;
        };

        info!(
            task = format!("{task_id:?}"),
            from_block,
            to_block,
            inserted,
            "Withdrawal request indexer completed"
        );
    }
}


/// Fetch logs from Reth RPC
pub async fn fetch_withdrawal_logs(
    client: &reqwest::Client,
    rpc_url: &str,
    from_block: u64,
    to_block: u64,
    contract_address: Address,
    topic0: B256,
) -> anyhow::Result<Vec<LogEntry>> {
    let params = json!([{
        "fromBlock": format!("{:#x}", from_block),
        "toBlock": format!("{:#x}", to_block),
        "address": format!("{:#x}", contract_address),
        "topics": [format!("{:#x}", topic0)]
    }]);

    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "eth_getLogs",
        "params": params,
    });


    let resp = client.post(rpc_url).json(&body).send().await?.error_for_status()?;
    let json: serde_json::Value = resp.json().await?;
    info!(?json, "JSON response from RPC");
    let logs: Vec<LogEntry> = serde_json::from_value(json["result"].clone())?;
    info!(?logs, "Logs from RPC");
    Ok(logs)
}

fn decode_log_entry(log: &LogEntry) -> anyhow::Result<DecodedWithdrawalIntent> {
    let log_data = LogData::new(
        log.topics.clone(),
        log.data.clone(),
    ).unwrap();

    let event_log = Log {
        address: log.address,
        data: log_data,
    };

    let event = WithdrawalIntentEvent::decode_log(&event_log, true)
        .map_err(|e| anyhow::anyhow!("Failed to decode WithdrawalIntentEvent: {:?}", e))?;

    Ok(DecodedWithdrawalIntent {
        txid: log.transaction_hash,
        amount: event.amount,
        destination: Descriptor::from_bytes(&event.destination)?,
        block_number: log.block_number,
    })
}
