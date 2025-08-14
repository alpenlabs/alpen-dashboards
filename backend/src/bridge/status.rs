use axum::Json;
use bitcoin::{secp256k1::PublicKey, Txid};

use std::sync::{atomic::Ordering, Arc};
use strata_bridge_rpc::traits::{StrataBridgeControlApiClient, StrataBridgeMonitoringApiClient};
use strata_bridge_rpc::types::{
    RpcDepositStatus, RpcOperatorStatus, RpcReimbursementStatus, RpcWithdrawalStatus,
};

use strata_primitives::buf::Buf32;

use super::{
    cache::BridgeStatusCache,
    types::{
        BridgeMonitoringContext, BridgeStatus, DepositInfo, DepositStatus, OperatorStatus,
        ReimbursementInfo, ReimbursementStatus, TxStatus, WithdrawalInfo, WithdrawalStatus,
    },
};

use tokio::time::{interval, Duration};
use tracing::{error, info, warn};

use crate::{config::BridgeMonitoringConfig, utils::rpc_client::create_rpc_client};

/// Get transaction confirmations from esplora
async fn get_tx_confirmations(esplora_url: &str, txid: Txid, chain_tip_height: u64) -> u64 {
    let url = format!("{}/tx/{}/status", esplora_url.trim_end_matches('/'), txid);

    let status_resp = reqwest::get(&url).await;

    let status: TxStatus = match status_resp {
        Ok(resp) => match resp.json().await {
            Ok(status) => status,
            Err(e) => {
                error!(%txid, %e, "Failed to parse tx status JSON from esplora");
                return 0;
            }
        },
        Err(e) => {
            error!(%txid, %e, "Failed to fetch tx status from esplora");
            return 0;
        }
    };

    status
        .block_height
        .filter(|_| status.confirmed)
        .map(|h| chain_tip_height.saturating_sub(h) + 1)
        .unwrap_or(0)
}

/// Determine which cached deposit entries should be purged
async fn determine_deposits_to_purge(
    cache: &BridgeStatusCache,
    config: &BridgeMonitoringConfig,
    chain_tip_height: u64,
) -> Vec<Txid> {
    let max_confirmations = config.max_tx_confirmations();
    let mut deposits_to_purge = Vec::new();

    let final_deposits =
        cache.filter_deposits(|s| matches!(s, DepositStatus::Complete | DepositStatus::Failed));

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

        if current_confirmations >= max_confirmations {
            deposits_to_purge.push(txid);
        }
    }

    deposits_to_purge
}

/// Determine which cached withdrawal entries should be purged
async fn determine_withdrawals_to_purge(
    cache: &BridgeStatusCache,
    config: &BridgeMonitoringConfig,
    chain_tip_height: u64,
) -> Vec<Buf32> {
    let max_confirmations = config.max_tx_confirmations();
    let mut withdrawals_to_purge = Vec::new();

    let final_withdrawals = cache.filter_withdrawals(|s| matches!(s, WithdrawalStatus::Complete));

    for (request_id, withdrawal_info) in final_withdrawals {
        if let Some(fulfillment_txid) = withdrawal_info.fulfillment_txid {
            let current_confirmations =
                get_tx_confirmations(config.esplora_url(), fulfillment_txid, chain_tip_height)
                    .await;

            if current_confirmations >= max_confirmations {
                withdrawals_to_purge.push(request_id);
            }
        }
    }

    withdrawals_to_purge
}

/// Determine which cached reimbursement entries should be purged
async fn determine_reimbursements_to_purge(
    cache: &BridgeStatusCache,
    config: &BridgeMonitoringConfig,
    chain_tip_height: u64,
) -> Vec<Txid> {
    let max_confirmations = config.max_tx_confirmations();
    let mut reimbursements_to_purge = Vec::new();

    let final_reimbursements = cache.filter_reimbursements(|s| {
        matches!(
            s,
            ReimbursementStatus::Complete | ReimbursementStatus::Cancelled
        )
    });

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

        if current_confirmations >= max_confirmations {
            reimbursements_to_purge.push(txid);
        }
    }

    reimbursements_to_purge
}

/// Periodically fetch bridge status and update bridge cache
pub async fn bridge_monitoring_task(context: Arc<BridgeMonitoringContext>) {
    let mut interval = interval(Duration::from_secs(
        context.config.status_refetch_interval(),
    ));

    loop {
        // Fetch all data without holding lock

        // Bridge operator status
        let mut operator_statuses = Vec::new();
        let mut sorted_operators: Vec<_> = context.config.bridge_rpc_urls().iter().collect();
        sorted_operators.sort_by_key(|(_, rpc_url)| *rpc_url);

        for (index, (public_key_string, rpc_url)) in sorted_operators.iter().enumerate() {
            let operator_id = format!("Alpen Labs #{}", index + 1);
            let pk_bytes = hex::decode(public_key_string).expect("decode to succeed");
            let operator_pk = PublicKey::from_slice(&pk_bytes).expect("conversion to succeed");
            let status = get_operator_status(rpc_url).await;

            operator_statuses.push(OperatorStatus::new(operator_id, operator_pk, status));
        }

        // Update operators incrementally
        {
            let mut cache = context.status_cache.write().await;
            cache.update_operators(operator_statuses);
        }

        let chain_tip_height =
            match get_bitcoin_chain_tip_height(context.config.esplora_url()).await {
                Ok(height) => height,
                Err(e) => {
                    error!(error = %e, "Failed to get Bitcoin chain tip");
                    continue;
                }
            };
        info!(%chain_tip_height, "bitcoin chain tip");

        // Get existing active entries from cache to avoid unnecessary RPC calls
        let (active_deposits, active_withdrawals, active_reimbursements) = {
            let cache = context.status_cache.read().await;
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
        };

        // Update deposits incrementally
        let deposit_updates: Vec<(Txid, DepositInfo, u64)> =
            get_deposits(&context.config, chain_tip_height, &active_deposits)
                .await
                .iter()
                .map(|(info, confirmations)| (info.deposit_request_txid, *info, *confirmations))
                .collect();

        {
            let mut cache = context.status_cache.write().await;
            cache.apply_deposit_updates(deposit_updates);

            // Determine deposits to purge after applying updates
            let deposits_to_purge =
                determine_deposits_to_purge(&cache, &context.config, chain_tip_height).await;
            cache.purge_deposits(deposits_to_purge);
        }

        // Update withdrawals incrementally
        let withdrawal_updates: Vec<(Buf32, WithdrawalInfo, u64)> =
            get_withdrawals(&context.config, chain_tip_height, &active_withdrawals)
                .await
                .iter()
                .map(|(info, confirmations)| (info.withdrawal_request_txid, *info, *confirmations))
                .collect();

        {
            let mut cache = context.status_cache.write().await;
            cache.apply_withdrawal_updates(withdrawal_updates);

            // Determine withdrawals to purge after applying updates
            let withdrawals_to_purge =
                determine_withdrawals_to_purge(&cache, &context.config, chain_tip_height).await;
            cache.purge_withdrawals(withdrawals_to_purge);
        }

        // Update reimbursements incrementally
        let reimbursement_updates: Vec<(Txid, ReimbursementInfo, u64)> =
            get_reimbursements(&context.config, chain_tip_height, &active_reimbursements)
                .await
                .iter()
                .map(|(info, confirmations)| (info.claim_txid, *info, *confirmations))
                .collect();

        {
            let mut cache = context.status_cache.write().await;
            cache.apply_reimbursement_updates(reimbursement_updates);

            // Determine reimbursements to purge after applying updates
            let reimbursements_to_purge =
                determine_reimbursements_to_purge(&cache, &context.config, chain_tip_height).await;
            cache.purge_reimbursements(reimbursements_to_purge);
        }

        // Mark initial status query as complete and notify waiters
        if !context.status_available.load(Ordering::Acquire) {
            context.status_available.store(true, Ordering::Release);
            context.initial_status_query_complete.notify_waiters();
        }

        // Wait for next interval
        interval.tick().await;
    }
}

/// Fetch operator status
async fn get_operator_status(rpc_url: &str) -> RpcOperatorStatus {
    let rpc_client = create_rpc_client(rpc_url);

    // Directly use `get_uptime`
    if rpc_client.get_uptime().await.is_ok() {
        RpcOperatorStatus::Online
    } else {
        warn!("Failed to fetch bridge operator uptime");
        RpcOperatorStatus::Offline
    }
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

/// Fetch deposit requests
async fn get_deposit_requests(config: &BridgeMonitoringConfig) -> Vec<Txid> {
    for rpc_url in config.bridge_rpc_urls().values() {
        let rpc_client = create_rpc_client(rpc_url);

        match rpc_client.get_deposit_requests().await {
            Ok(txids) if !txids.is_empty() => return txids,
            Ok(_) | Err(_) => {} // Try next operator
        }
    }

    warn!("No deposit requests found");
    Vec::new()
}

/// Fetch deposit details
async fn get_deposits(
    config: &BridgeMonitoringConfig,
    chain_tip_height: u64,
    active_deposit_txids: &[Txid],
) -> Vec<(DepositInfo, u64)> {
    let mut deposit_requests = get_deposit_requests(config).await;

    // Add existing active deposits that we need to check for status updates
    for txid in active_deposit_txids {
        if !deposit_requests.contains(txid) {
            deposit_requests.push(*txid);
        }
    }

    info!(
        "Checking {} deposit requests ({} new, {} existing active)",
        deposit_requests.len(),
        get_deposit_requests(config).await.len(),
        active_deposit_txids.len()
    );

    let mut deposit_infos: Vec<(DepositInfo, u64)> = Vec::new();
    for deposit_request_txid in deposit_requests.iter() {
        let mut rpc_info = None;
        for rpc_url in config.bridge_rpc_urls().values() {
            let rpc_client = create_rpc_client(rpc_url);
            if let Ok(info) = rpc_client
                .get_deposit_request_info(*deposit_request_txid)
                .await
            {
                rpc_info = Some(info);
                break;
            }
        }

        let Some(dep_info) = rpc_info else {
            error!(%deposit_request_txid, "Failed to fetch deposit info");
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
                if confirmations < config.max_tx_confirmations() {
                    deposit_infos.push((DepositInfo::from(&dep_info), confirmations));
                }
            }
        }
    }

    if deposit_infos.is_empty() {
        warn!("No deposit infos found");
    }
    deposit_infos
}

/// Fetch withdrawal requests
async fn get_withdrawal_requests(config: &BridgeMonitoringConfig) -> Vec<Buf32> {
    for rpc_url in config.bridge_rpc_urls().values() {
        let rpc_client = create_rpc_client(rpc_url);

        match rpc_client.get_withdrawals().await {
            Ok(txids) if !txids.is_empty() => return txids,
            Ok(_) | Err(_) => {} // Try next operator
        }
    }

    warn!("No withdrawal requests found");
    Vec::new()
}

/// Fetch withdrawal/fullfillment details
async fn get_withdrawals(
    config: &BridgeMonitoringConfig,
    chain_tip_height: u64,
    active_withdrawal_request_ids: &[Buf32],
) -> Vec<(WithdrawalInfo, u64)> {
    let mut withdrawal_requests = get_withdrawal_requests(config).await;

    // Add existing active withdrawals that we need to check for status updates
    for request_id in active_withdrawal_request_ids {
        if !withdrawal_requests.contains(request_id) {
            withdrawal_requests.push(*request_id);
        }
    }

    info!(
        "Checking {} withdrawal requests ({} new, {} existing active)",
        withdrawal_requests.len(),
        get_withdrawal_requests(config).await.len(),
        active_withdrawal_request_ids.len()
    );

    let mut withdrawal_infos: Vec<(WithdrawalInfo, u64)> = Vec::new();
    for withdrawal_request_txid in withdrawal_requests.iter() {
        let mut rpc_info = None;
        for rpc_url in config.bridge_rpc_urls().values() {
            let rpc_client = create_rpc_client(rpc_url);
            if let Ok(info) = rpc_client
                .get_withdrawal_info(*withdrawal_request_txid)
                .await
            {
                rpc_info = info;
                break;
            }
        }

        let Some(wd_info) = rpc_info else {
            error!(%withdrawal_request_txid, "Failed to fetch withdrawal info");
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
                if confirmations < config.max_tx_confirmations() {
                    withdrawal_infos.push((WithdrawalInfo::from(&wd_info), confirmations));
                }
            }
        }
    }

    if withdrawal_infos.is_empty() {
        warn!("No withdrawal infos found");
    }
    withdrawal_infos
}

/// Fetch claims
async fn get_claims(config: &BridgeMonitoringConfig) -> Vec<Txid> {
    for rpc_url in config.bridge_rpc_urls().values() {
        let rpc_client = create_rpc_client(rpc_url);

        match rpc_client.get_claims().await {
            Ok(txids) if !txids.is_empty() => return txids,
            Ok(_) | Err(_) => {} // Try next operator
        }
    }

    warn!("No claims found");
    Vec::new()
}

/// Fetch claim/reimbursement details
async fn get_reimbursements(
    config: &BridgeMonitoringConfig,
    chain_tip_height: u64,
    active_reimbursement_txids: &[Txid],
) -> Vec<(ReimbursementInfo, u64)> {
    let mut claims = get_claims(config).await;

    // Add existing active reimbursements that we need to check for status updates
    for txid in active_reimbursement_txids {
        if !claims.contains(txid) {
            claims.push(*txid);
        }
    }

    info!(
        "Checking {} claims ({} new, {} existing active)",
        claims.len(),
        get_claims(config).await.len(),
        active_reimbursement_txids.len()
    );

    let mut reimbursement_infos = Vec::new();
    for claim_txid in claims.iter() {
        let mut rpc_info = None;
        for rpc_url in config.bridge_rpc_urls().values() {
            let rpc_client = create_rpc_client(rpc_url);
            if let Ok(info) = rpc_client.get_claim_info(*claim_txid).await {
                rpc_info = info;
                break;
            }
        }

        let Some(claim_info) = rpc_info else {
            error!(%claim_txid, "Failed to fetch claim info");
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
                if confirmations < config.max_tx_confirmations() {
                    reimbursement_infos.push((ReimbursementInfo::from(&claim_info), confirmations));
                }
            }
        }
    }

    if reimbursement_infos.is_empty() {
        warn!("No reimbursement infos found");
    }
    reimbursement_infos
}

/// Return latest bridge status extracted from cache
pub async fn get_bridge_status(context: Arc<BridgeMonitoringContext>) -> Json<BridgeStatus> {
    // Wait for initial status query to complete if not yet available
    if !context.status_available.load(Ordering::Acquire) {
        info!("Waiting for initial bridge status query to complete");
        context.initial_status_query_complete.notified().await;
    }

    let cache = context.status_cache.read().await;

    let bridge_status = BridgeStatus {
        operators: cache.get_operators(),
        deposits: cache
            .filter_deposits(|_| true)
            .into_iter()
            .map(|(_, info)| info)
            .collect(),
        withdrawals: cache
            .filter_withdrawals(|_| true)
            .into_iter()
            .map(|(_, info)| info)
            .collect(),
        reimbursements: cache
            .filter_reimbursements(|_| true)
            .into_iter()
            .map(|(_, info)| info)
            .collect(),
    };

    Json(bridge_status)
}
