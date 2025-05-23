use anyhow::Result;
use chrono::Utc;
use sqlx::SqlitePool;
use tracing::{error, info, warn};

use crate::db::upsert_deposit;
use crate::rpc::{get_current_deposits, get_deposit_info};
use crate::types::{DepositInfo, DepositToWithdrawal};

/// Polls Strata + Bridge RPCs and updates the indexer DB
pub async fn index_deposits(
    strata_rpc: &jsonrpsee_http_client::HttpClient,
    bridge_rpc: &jsonrpsee_http_client::HttpClient,
    db: &SqlitePool,
) -> Result<()> {
    let current_deposits = match get_current_deposits(strata_rpc).await {
        Ok(ids) => ids,
        Err(e) => {
            error!(%e, "Failed to get current deposits");
            return Err(e.into());
        }
    };

    info!(count = current_deposits.len(), "Fetched current deposit IDs");

    for deposit_id in current_deposits {
        match get_deposit_info(strata_rpc, bridge_rpc, deposit_id).await {
            Ok((Some(deposit_info), Some(deposit_map))) => {
                let record = crate::types::DepositRecord {
                    deposit_outpoint: deposit_map.deposit_outpoint.to_string(),
                    deposit_request_txid: deposit_info.txid.to_string(),
                    deposit_txid: deposit_info.txid.to_string(),
                    deposit_block_height: deposit_info.block_height as i64,
                    current_block_height: deposit_info.block_height as i64,
                    confirmation_depth: 1,
                    status: deposit_info.status.to_string(),
                    last_checked: Utc::now().to_rfc3339(),
                    alpen_address: Some(deposit_info.alpen_address.clone()),
                };
                upsert_deposit(db, &record).await?;
            }
            Ok(_) => {
                warn!(%deposit_id, "Incomplete deposit info or mapping");
            }
            Err(e) => {
                error!(%deposit_id, %e, "Error getting deposit info");
            }
        }
    }

    Ok(())
}
