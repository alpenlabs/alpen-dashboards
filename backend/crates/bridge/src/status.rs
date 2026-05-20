use anyhow::Result;
use axum::Json;
use bitcoin::{secp256k1::PublicKey, Txid};
use std::sync::Arc;
use strata_bridge_rpc::types::{RpcDepositStatus, RpcReimbursementStatus, RpcWithdrawalStatus};

use strata_primitives::buf::Buf32;
use strata_tasks::ShutdownGuard;

use super::{
    bridge_rpc::{self, RpcClientManager},
    context::BridgeMonitoringContext,
    types::{
        BridgeStatus, DepositInfo, DepositStatus, OperatorStatus, ReimbursementInfo,
        ReimbursementStatus, TxStatus, WithdrawalInfo, WithdrawalStatus,
    },
};

use tokio::time::{interval, Duration};
use tracing::{error, info, warn};

use status_config::BridgeMonitoringConfig;

/// Get transaction confirmations from esplora
async fn get_tx_confirmations(esplora_url: &str, txid: Txid, chain_tip_height: u64) -> Option<u64> {
    let url = format!("{}/tx/{}/status", esplora_url.trim_end_matches('/'), txid);

    let status_resp = reqwest::get(&url).await;

    let status: TxStatus = match status_resp {
        Ok(resp) => match resp.json().await {
            Ok(status) => status,
            Err(e) => {
                error!(%txid, error = %e, "failed to parse tx status JSON from esplora");
                return None;
            }
        },
        Err(e) => {
            error!(%txid, error = %e, "failed to fetch tx status from esplora");
            return None;
        }
    };

    status
        .block_height
        .filter(|_| status.confirmed)
        .map(|h| chain_tip_height.saturating_sub(h) + 1)
}

/// Determine which cached deposit entries should be purged
async fn determine_deposits_to_purge(
    final_deposits: Vec<(Txid, DepositInfo)>,
    config: &BridgeMonitoringConfig,
    chain_tip_height: u64,
) -> Vec<Txid> {
    let max_confirmations = config.max_tx_confirmations();
    let mut deposits_to_purge = Vec::new();

    for (txid, deposit_info) in final_deposits {
        let check_txid = match deposit_info.status {
            DepositStatus::Failed => deposit_info.deposit_request_txid,
            DepositStatus::Complete => deposit_info
                .deposit_txid
                .unwrap_or(deposit_info.deposit_request_txid),
            DepositStatus::InProgress => unreachable!(),
        };

        let current_confirmations =
            get_tx_confirmations(config.esplora_url(), check_txid, chain_tip_height).await;

        if let Some(confirmations) = current_confirmations {
            if confirmations >= max_confirmations {
                deposits_to_purge.push(txid);
            }
        }
    }

    deposits_to_purge
}

/// Determine which cached withdrawal entries should be purged
async fn determine_withdrawals_to_purge(
    final_withdrawals: Vec<(Buf32, WithdrawalInfo)>,
    config: &BridgeMonitoringConfig,
    chain_tip_height: u64,
) -> Vec<Buf32> {
    let max_confirmations = config.max_tx_confirmations();
    let mut withdrawals_to_purge = Vec::new();

    for (request_id, withdrawal_info) in final_withdrawals {
        if let Some(fulfillment_txid) = withdrawal_info.fulfillment_txid {
            let current_confirmations =
                get_tx_confirmations(config.esplora_url(), fulfillment_txid, chain_tip_height)
                    .await;

            if let Some(confirmations) = current_confirmations {
                if confirmations >= max_confirmations {
                    withdrawals_to_purge.push(request_id);
                }
            }
        }
    }

    withdrawals_to_purge
}

/// Determine which cached reimbursement entries should be purged
async fn determine_reimbursements_to_purge(
    final_reimbursements: Vec<(Txid, ReimbursementInfo)>,
    config: &BridgeMonitoringConfig,
    chain_tip_height: u64,
) -> Vec<Txid> {
    let max_confirmations = config.max_tx_confirmations();
    let mut reimbursements_to_purge = Vec::new();

    for (txid, reimbursement_info) in final_reimbursements {
        let check_txid = match reimbursement_info.status {
            ReimbursementStatus::Cancelled => reimbursement_info.claim_txid,
            ReimbursementStatus::Complete => reimbursement_info
                .payout_txid
                .unwrap_or(reimbursement_info.claim_txid),
            ReimbursementStatus::Challenged
            | ReimbursementStatus::InProgress
            | ReimbursementStatus::NotStarted => unreachable!(),
        };

        let current_confirmations =
            get_tx_confirmations(config.esplora_url(), check_txid, chain_tip_height).await;

        if let Some(confirmations) = current_confirmations {
            if confirmations >= max_confirmations {
                reimbursements_to_purge.push(txid);
            }
        }
    }

    reimbursements_to_purge
}

/// Periodically fetch bridge status and update bridge cache
pub async fn bridge_monitoring_task(
    context: Arc<BridgeMonitoringContext>,
    shutdown: ShutdownGuard,
) -> Result<()> {
    let mut interval = interval(Duration::from_secs(
        context.config().status_refetch_interval(),
    ));

    // Create RPC client manager once and reuse it
    let rpc_manager = RpcClientManager::new(context.config());

    loop {
        tokio::select! {
            _ = shutdown.wait_for_shutdown() => break,
            _ = interval.tick() => {}
        }

        // Fetch all data without holding lock

        // Bridge operator status
        let mut operator_statuses = Vec::new();

        for (index, operator) in context.config().operators().iter().enumerate() {
            let operator_id = format!("Alpen Labs #{}", index + 1);
            let pk_bytes = hex::decode(operator.public_key()).expect("decode to succeed");
            let operator_pk = PublicKey::from_slice(&pk_bytes).expect("conversion to succeed");
            let status = bridge_rpc::get_operator_status(operator.rpc_url()).await;
            operator_statuses.push(OperatorStatus::new(operator_id, operator_pk, status));
        }

        // Update operators incrementally
        context
            .with_state_mut(|cache| {
                cache.update_operators(operator_statuses);
            })
            .await;

        let chain_tip_height =
            match get_bitcoin_chain_tip_height(context.config().esplora_url()).await {
                Ok(height) => height,
                Err(e) => {
                    error!(error = %e, "failed to get Bitcoin chain tip");
                    continue;
                }
            };
        info!(%chain_tip_height, "bitcoin chain tip");

        // Get existing active entries from cache to avoid unnecessary RPC calls
        let (active_deposits, active_withdrawals, active_reimbursements) = context
            .with_state(|cache| {
                let deposits: Vec<Txid> = cache
                    .filter_deposits(|s| matches!(s, DepositStatus::InProgress))
                    .iter()
                    .map(|(txid, _)| *txid)
                    .collect();
                let withdrawals: Vec<Buf32> = cache
                    .filter_withdrawals(|s| matches!(s, WithdrawalStatus::InProgress))
                    .iter()
                    .map(|(request_id, _)| *request_id)
                    .collect();
                let reimbursements: Vec<Txid> = cache
                    .filter_reimbursements(|s| {
                        matches!(
                            s,
                            ReimbursementStatus::InProgress | ReimbursementStatus::Challenged
                        )
                    })
                    .iter()
                    .map(|(txid, _)| *txid)
                    .collect();
                (deposits, withdrawals, reimbursements)
            })
            .await;

        // Update deposits incrementally
        let deposit_updates: Vec<(Txid, DepositInfo, u64)> = get_deposits(
            &rpc_manager,
            context.config(),
            chain_tip_height,
            &active_deposits,
        )
        .await
        .iter()
        .map(|(info, confirmations)| (info.deposit_request_txid, *info, *confirmations))
        .collect();

        let final_deposits = context
            .with_state_mut(|cache| {
                cache.apply_deposit_updates(deposit_updates);

                // Determine deposits to purge after applying updates.
                cache.filter_deposits(|s| {
                    matches!(s, DepositStatus::Complete | DepositStatus::Failed)
                })
            })
            .await;
        let deposits_to_purge =
            determine_deposits_to_purge(final_deposits, context.config(), chain_tip_height).await;
        context
            .with_state_mut(|cache| {
                cache.purge_deposits(deposits_to_purge);
            })
            .await;

        // Update withdrawals incrementally
        let withdrawal_updates: Vec<(Buf32, WithdrawalInfo, u64)> = get_withdrawals(
            &rpc_manager,
            context.config(),
            chain_tip_height,
            &active_withdrawals,
        )
        .await
        .iter()
        .map(|(info, confirmations)| (info.withdrawal_request_txid, *info, *confirmations))
        .collect();

        let final_withdrawals = context
            .with_state_mut(|cache| {
                cache.apply_withdrawal_updates(withdrawal_updates);

                // Determine withdrawals to purge after applying updates.
                cache.filter_withdrawals(|s| matches!(s, WithdrawalStatus::Complete))
            })
            .await;
        let withdrawals_to_purge =
            determine_withdrawals_to_purge(final_withdrawals, context.config(), chain_tip_height)
                .await;
        context
            .with_state_mut(|cache| {
                cache.purge_withdrawals(withdrawals_to_purge);
            })
            .await;

        // Update reimbursements incrementally
        let reimbursement_updates: Vec<(Txid, ReimbursementInfo, u64)> = get_reimbursements(
            &rpc_manager,
            context.config(),
            chain_tip_height,
            &active_reimbursements,
        )
        .await
        .iter()
        .map(|(info, confirmations)| (info.claim_txid, *info, *confirmations))
        .collect();

        let final_reimbursements = context
            .with_state_mut(|cache| {
                cache.apply_reimbursement_updates(reimbursement_updates);

                // Determine reimbursements to purge after applying updates.
                cache.filter_reimbursements(|s| {
                    matches!(
                        s,
                        ReimbursementStatus::Complete | ReimbursementStatus::Cancelled
                    )
                })
            })
            .await;
        let reimbursements_to_purge = determine_reimbursements_to_purge(
            final_reimbursements,
            context.config(),
            chain_tip_height,
        )
        .await;
        context
            .with_state_mut(|cache| {
                cache.purge_reimbursements(reimbursements_to_purge);
            })
            .await;

        // Mark initial status query as complete and notify waiters
        context.mark_status_available();
    }

    Ok(())
}

/// Fetch bitcoin chain tip height
async fn get_bitcoin_chain_tip_height(
    esplora_url: &str,
) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
    let endpoint = format!("{}/blocks/tip/height", esplora_url.trim_end_matches('/'));
    let resp = reqwest::Client::new().get(&endpoint).send().await?;

    let text = resp.text().await?;
    let height = text.trim().parse::<u64>()?;
    Ok(height)
}

/// Fetch detailed information for all deposit requests
///
/// Queries bridge operators for deposit details, including both new deposit requests
/// and existing active deposits that need status updates. Filters results based on
/// confirmation count to only include deposits below the maximum confirmation threshold.
///
/// # Arguments
///
/// * `rpc_manager` - RPC client manager with retry/failover logic
/// * `config` - Bridge monitoring configuration (contains max confirmations threshold)
/// * `chain_tip_height` - Current Bitcoin blockchain height
/// * `active_deposit_txids` - List of deposit txids already being tracked (for status updates)
///
/// # Returns
///
/// Vector of tuples containing:
///
/// - [`DepositInfo`] - Detailed deposit information
/// - [`u64`] - Number of confirmations (0 for in-progress deposits)
///
/// Deposits are filtered to only include those with confirmations < [`BridgeMonitoringConfig::max_tx_confirmations`].
async fn get_deposits(
    rpc_manager: &RpcClientManager,
    config: &BridgeMonitoringConfig,
    chain_tip_height: u64,
    active_deposit_txids: &[Txid],
) -> Vec<(DepositInfo, u64)> {
    let new_deposit_requests = bridge_rpc::get_deposit_requests(rpc_manager).await;
    let new_count = new_deposit_requests.len();
    let mut deposit_requests = new_deposit_requests;

    // Add existing active deposits that we need to check for status updates
    for txid in active_deposit_txids {
        if !deposit_requests.contains(txid) {
            deposit_requests.push(*txid);
        }
    }

    info!(
        deposit_request_count = deposit_requests.len(),
        new_deposit_request_count = new_count,
        active_deposit_count = active_deposit_txids.len(),
        "checking deposit requests"
    );

    let mut deposit_infos: Vec<(DepositInfo, u64)> = Vec::new();
    for deposit_request_txid in deposit_requests.iter() {
        let txid = *deposit_request_txid;
        let rpc_info = bridge_rpc::get_deposit_request_info(rpc_manager, txid).await;

        let Some(dep_info) = rpc_info else {
            error!(%deposit_request_txid, "failed to fetch deposit info after retries");
            continue;
        };

        // Filter based on number of confirmations
        match &dep_info.status {
            RpcDepositStatus::InProgress => {
                // Always include in-progress deposits - no need to check confirmations
                deposit_infos.push((DepositInfo::from(&dep_info), 0));
            }
            RpcDepositStatus::Failed { .. } | RpcDepositStatus::Complete { .. } => {
                let txid = match &dep_info.status {
                    RpcDepositStatus::Failed { .. } => dep_info.deposit_request_txid,
                    RpcDepositStatus::Complete { deposit_txid } => *deposit_txid,
                    RpcDepositStatus::InProgress => unreachable!(), // Already matched
                };

                let confirmations =
                    get_tx_confirmations(config.esplora_url(), txid, chain_tip_height).await;
                if let Some(confirmations) = confirmations {
                    if confirmations < config.max_tx_confirmations() {
                        deposit_infos.push((DepositInfo::from(&dep_info), confirmations));
                    }
                }
            }
        }
    }

    if deposit_infos.is_empty() {
        warn!("no deposit infos found");
    }
    deposit_infos
}

/// Fetch detailed information for all withdrawal requests and fulfillments
///
/// Queries bridge operators for withdrawal details, including both new withdrawal requests
/// and existing active withdrawals that need status updates. Filters results based on
/// confirmation count to only include withdrawals below the maximum confirmation threshold.
///
/// # Arguments
///
/// * `rpc_manager` - RPC client manager with retry/failover logic
/// * `config` - Bridge monitoring configuration (contains max confirmations threshold)
/// * `chain_tip_height` - Current Bitcoin blockchain height
/// * `active_withdrawal_request_ids` - List of withdrawal request IDs already being tracked
///
/// # Returns
///
/// Vector of tuples containing:
/// - `WithdrawalInfo` - Detailed withdrawal information
/// - `u64` - Number of confirmations (0 for in-progress withdrawals)
///
/// Withdrawals are filtered to only include those with confirmations < max_tx_confirmations.
async fn get_withdrawals(
    rpc_manager: &RpcClientManager,
    config: &BridgeMonitoringConfig,
    chain_tip_height: u64,
    active_withdrawal_request_ids: &[Buf32],
) -> Vec<(WithdrawalInfo, u64)> {
    let new_withdrawal_requests = bridge_rpc::get_withdrawal_requests(rpc_manager).await;
    let new_count = new_withdrawal_requests.len();
    let mut withdrawal_requests = new_withdrawal_requests;

    // Add existing active withdrawals that we need to check for status updates
    for request_id in active_withdrawal_request_ids {
        if !withdrawal_requests.contains(request_id) {
            withdrawal_requests.push(*request_id);
        }
    }

    info!(
        withdrawal_request_count = withdrawal_requests.len(),
        new_withdrawal_request_count = new_count,
        active_withdrawal_count = active_withdrawal_request_ids.len(),
        "checking withdrawal requests"
    );

    let mut withdrawal_infos: Vec<(WithdrawalInfo, u64)> = Vec::new();
    for withdrawal_request_txid in withdrawal_requests.iter() {
        let request_id = *withdrawal_request_txid;
        let rpc_info = bridge_rpc::get_withdrawal_info(rpc_manager, request_id).await;

        let Some(wd_info) = rpc_info else {
            error!(%withdrawal_request_txid, "failed to fetch withdrawal info after retries");
            continue;
        };

        // Filter based on number of confirmations
        match &wd_info.status {
            RpcWithdrawalStatus::InProgress => {
                // Always include in-progress withdrawals
                withdrawal_infos.push((WithdrawalInfo::from(&wd_info), 0));
            }
            RpcWithdrawalStatus::Complete { fulfillment_txid } => {
                let confirmations =
                    get_tx_confirmations(config.esplora_url(), *fulfillment_txid, chain_tip_height)
                        .await;
                if let Some(confirmations) = confirmations {
                    if confirmations < config.max_tx_confirmations() {
                        withdrawal_infos.push((WithdrawalInfo::from(&wd_info), confirmations));
                    }
                }
            }
        }
    }

    if withdrawal_infos.is_empty() {
        warn!("no withdrawal infos found");
    }
    withdrawal_infos
}

/// Fetch detailed information for all claim and reimbursement transactions
///
/// Queries bridge operators for claim/reimbursement details, including both new claims
/// and existing active reimbursements that need status updates. Filters results based on
/// confirmation count and status (skips NotStarted claims).
///
/// # Arguments
///
/// * `rpc_manager` - RPC client manager with retry/failover logic
/// * `config` - Bridge monitoring configuration (contains max confirmations threshold)
/// * `chain_tip_height` - Current Bitcoin blockchain height
/// * `active_reimbursement_txids` - List of claim txids already being tracked
///
/// # Returns
///
/// Vector of tuples containing:
/// - `ReimbursementInfo` - Detailed claim/reimbursement information
/// - `u64` - Number of confirmations (0 for in-progress/challenged claims)
///
/// Claims with status `NotStarted` are excluded. Completed/cancelled claims are filtered
/// to only include those with confirmations < max_tx_confirmations.
async fn get_reimbursements(
    rpc_manager: &RpcClientManager,
    config: &BridgeMonitoringConfig,
    chain_tip_height: u64,
    active_reimbursement_txids: &[Txid],
) -> Vec<(ReimbursementInfo, u64)> {
    let new_claims = bridge_rpc::get_claims(rpc_manager).await;
    let new_count = new_claims.len();
    let mut claims = new_claims;

    // Add existing active reimbursements that we need to check for status updates
    for txid in active_reimbursement_txids {
        if !claims.contains(txid) {
            claims.push(*txid);
        }
    }

    info!(
        claim_count = claims.len(),
        new_claim_count = new_count,
        active_reimbursement_count = active_reimbursement_txids.len(),
        "checking claims"
    );

    let mut reimbursement_infos = Vec::new();
    for claim_txid in claims.iter() {
        let txid = *claim_txid;
        let rpc_info = bridge_rpc::get_claim_info(rpc_manager, txid).await;

        let Some(claim_info) = rpc_info else {
            error!(%claim_txid, "failed to fetch claim info after retries");
            continue;
        };

        // Filter based on number of confirmations
        match &claim_info.status {
            // Skip if not started
            RpcReimbursementStatus::NotStarted => {
                continue;
            }
            RpcReimbursementStatus::InProgress { .. }
            | RpcReimbursementStatus::Challenged { .. } => {
                // Always include in-progress or challenged claims - no need to check confirmations
                reimbursement_infos.push((ReimbursementInfo::from(&claim_info), 0));
            }
            RpcReimbursementStatus::Cancelled | RpcReimbursementStatus::Complete { .. } => {
                let txid = match &claim_info.status {
                    RpcReimbursementStatus::Cancelled => claim_info.claim_txid,
                    RpcReimbursementStatus::Complete { payout_txid } => *payout_txid,
                    RpcReimbursementStatus::NotStarted
                    | RpcReimbursementStatus::InProgress { .. }
                    | RpcReimbursementStatus::Challenged { .. } => unreachable!(), // Already matched
                };
                let confirmations =
                    get_tx_confirmations(config.esplora_url(), txid, chain_tip_height).await;
                if let Some(confirmations) = confirmations {
                    if confirmations < config.max_tx_confirmations() {
                        reimbursement_infos
                            .push((ReimbursementInfo::from(&claim_info), confirmations));
                    }
                }
            }
        }
    }

    if reimbursement_infos.is_empty() {
        warn!("no reimbursement infos found");
    }
    reimbursement_infos
}

/// Return latest bridge status extracted from cache
pub async fn get_bridge_status(context: Arc<BridgeMonitoringContext>) -> Json<BridgeStatus> {
    context.wait_until_status_available().await;

    Json(context.bridge_status().await)
}
