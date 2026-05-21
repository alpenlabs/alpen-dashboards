use anyhow::Result;
use axum::http::StatusCode;
use axum::Json;
use bitcoin::{secp256k1::PublicKey, Txid};
use std::{collections::BTreeSet, sync::Arc};
use strata_bridge_primitives::types::DepositIdx;
use strata_primitives::L1Height;
use strata_tasks::ShutdownGuard;

use super::{
    bridge_rpc,
    context::BridgeMonitoringContext,
    esplora::{self, get_bitcoin_chain_tip_height, EsploraClient},
    state::{DepositInfoUpdate, ReimbursementInfoUpdate},
    types::{
        BridgeStatus, DepositInfo, DepositStatus, OperatorStatus, ReimbursementInfo,
        ReimbursementStatus,
    },
    withdrawal_requests::fetch_withdrawal_requests,
    withdrawal_status::get_withdrawal_updates,
};

use tokio::time::{interval, timeout, Duration};
use tracing::{error, info, warn};

/// Periodically fetch bridge status and update bridge cache.
pub async fn bridge_monitoring_task(
    context: Arc<BridgeMonitoringContext>,
    shutdown: ShutdownGuard,
) -> Result<()> {
    let mut interval = interval(Duration::from_secs(
        context.config().status_refetch_interval(),
    ));

    loop {
        tokio::select! {
            _ = shutdown.wait_for_shutdown() => break,
            _ = interval.tick() => {}
        }

        let mut operator_statuses = Vec::new();

        for (index, operator) in context.config().operators().iter().enumerate() {
            let operator_id = format!("Alpen Labs #{}", index + 1);
            let pk_bytes = hex::decode(operator.public_key()).expect("decode to succeed");
            let operator_pk = PublicKey::from_slice(&pk_bytes).expect("conversion to succeed");
            let status =
                bridge_rpc::get_operator_status(context.bridge_rpc(), operator.public_key()).await;
            operator_statuses.push(OperatorStatus::new(operator_id, operator_pk, status));
        }

        context.state().update_operators(operator_statuses).await;

        let chain_tip_height = match get_bitcoin_chain_tip_height(context.esplora()).await {
            Ok(height) => height,
            Err(e) => {
                error!(error = %e, "failed to get Bitcoin chain tip");
                continue;
            }
        };
        info!(%chain_tip_height, "bitcoin chain tip");

        let deposit_indices = match bridge_rpc::get_deposit_indices(context.bridge_rpc()).await {
            Ok(indices) => indices,
            Err(e) => {
                error!(error = %e, "failed to fetch bridge deposit indices");
                continue;
            }
        };

        let deposit_candidates = context
            .state()
            .select_deposit_info_candidates(&deposit_indices)
            .await;
        let deposit_infos = get_deposits(context.bridge_rpc(), &deposit_candidates).await;
        let deposit_info_updates =
            get_deposit_info_updates(context.esplora(), chain_tip_height, deposit_infos).await;
        context
            .state()
            .apply_deposit_info_updates(
                deposit_info_updates,
                context.config().max_tx_confirmations(),
            )
            .await;

        let pairing_cursor = context.state().withdrawal_pairing_cursor().await;
        let new_deposit_indices_count =
            count_deposit_indices_from(&deposit_indices, pairing_cursor.next_deposit_idx);
        let withdrawal_requests = fetch_withdrawal_requests(
            context.withdrawal_index(),
            pairing_cursor.next_withdrawal_seq,
            new_deposit_indices_count,
            context.config().withdrawal_pairing_batch_size(),
        );
        let new_pairings = context
            .state()
            .apply_withdrawal_pairings(&deposit_indices, &withdrawal_requests)
            .await;
        if !new_pairings.is_empty() {
            info!(
                pairing_count = new_pairings.len(),
                "paired indexed withdrawals with deposits"
            );
        }

        let withdrawal_candidates = context.state().select_withdrawal_status_candidates().await;
        let withdrawal_updates = get_withdrawal_updates(
            context.bridge_rpc(),
            context.withdrawal_index(),
            context.esplora(),
            chain_tip_height,
            &withdrawal_candidates,
            context.config().withdrawal_pairing_batch_size(),
        )
        .await;
        context
            .state()
            .apply_withdrawal_updates(withdrawal_updates, context.config().max_tx_confirmations())
            .await;

        let reimbursement_candidates = context
            .state()
            .select_reimbursement_status_candidates()
            .await;
        let reimbursement_updates = get_reimbursement_updates(
            context.bridge_rpc(),
            context.esplora(),
            chain_tip_height,
            &reimbursement_candidates,
        )
        .await;
        context
            .state()
            .apply_reimbursement_updates(
                reimbursement_updates,
                context.config().max_tx_confirmations(),
            )
            .await;

        context.mark_status_available();
    }

    Ok(())
}

fn count_deposit_indices_from(
    deposit_indices: &[DepositIdx],
    next_deposit_idx: DepositIdx,
) -> usize {
    // This mirrors the state pairing planner's contiguous-prefix rule. The
    // status tick uses this count to size DB reads; state still enforces the
    // pairing invariant when it applies the fetched rows.
    let deposit_indices = deposit_indices.iter().copied().collect::<BTreeSet<_>>();
    let mut next_deposit_idx = next_deposit_idx;
    let mut count = 0;

    while deposit_indices.contains(&next_deposit_idx) {
        count += 1;
        let Some(next) = next_deposit_idx.checked_add(1) else {
            break;
        };
        next_deposit_idx = next;
    }

    count
}

/// Fetch detailed information for all deposits.
async fn get_deposits(
    rpc_manager: &bridge_rpc::RpcClientManager,
    deposit_indices: &[DepositIdx],
) -> Vec<(DepositIdx, DepositInfo)> {
    info!(
        deposit_count = deposit_indices.len(),
        "fetching deposit details"
    );

    let mut deposit_infos = Vec::new();
    for deposit_idx in deposit_indices.iter().copied() {
        let dep_info = match bridge_rpc::get_deposit_info(rpc_manager, deposit_idx).await {
            Ok(info) => info,
            Err(e) => {
                error!(deposit_idx, error = %e, "failed to fetch deposit info");
                continue;
            }
        };

        deposit_infos.push((dep_info.deposit_idx, DepositInfo::from(&dep_info)));
    }

    if deposit_infos.is_empty() {
        warn!("no deposit infos found");
    }
    deposit_infos
}

async fn get_deposit_info_updates(
    esplora_client: &EsploraClient,
    chain_tip_height: L1Height,
    deposit_infos: Vec<(DepositIdx, DepositInfo)>,
) -> Vec<DepositInfoUpdate> {
    let mut updates = Vec::new();

    for (deposit_idx, deposit_info) in deposit_infos {
        let check_txid = match deposit_info.status {
            DepositStatus::InProgress => {
                updates.push(DepositInfoUpdate {
                    deposit_idx,
                    info: deposit_info,
                    confirmations: None,
                });
                continue;
            }
            DepositStatus::Failed => deposit_info.deposit_request_txid,
            DepositStatus::Complete => deposit_info
                .deposit_txid
                .unwrap_or(deposit_info.deposit_request_txid),
        };

        let current_confirmations =
            esplora::get_tx_confirmations(esplora_client, check_txid, chain_tip_height).await;
        updates.push(DepositInfoUpdate {
            deposit_idx,
            info: deposit_info,
            confirmations: current_confirmations,
        });
    }

    updates
}

/// Fetch reimbursement cache updates for paired deposits.
async fn get_reimbursement_updates(
    rpc_manager: &bridge_rpc::RpcClientManager,
    esplora_client: &EsploraClient,
    chain_tip_height: L1Height,
    candidates: &[DepositIdx],
) -> Vec<ReimbursementInfoUpdate> {
    let mut updates = Vec::new();

    for deposit_idx in candidates.iter().copied() {
        let status = match bridge_rpc::get_reimbursement_status(rpc_manager, deposit_idx).await {
            Ok(Some(status)) => status,
            Ok(None) => continue,
            Err(e) => {
                warn!(deposit_idx, error = %e, "failed to fetch reimbursement status");
                continue;
            }
        };

        let Some(info) = ReimbursementInfo::from_status(&status) else {
            continue;
        };

        let confirmations = match info.status {
            ReimbursementStatus::NotStarted => continue,
            ReimbursementStatus::InProgress => None,
            ReimbursementStatus::Slashed
            | ReimbursementStatus::Aborted
            | ReimbursementStatus::Complete => {
                let Some(txid) = terminal_reimbursement_txid(&info) else {
                    continue;
                };
                esplora::get_tx_confirmations(esplora_client, txid, chain_tip_height).await
            }
        };

        updates.push(ReimbursementInfoUpdate {
            deposit_idx,
            info,
            confirmations,
        });
    }

    updates
}

fn terminal_reimbursement_txid(info: &ReimbursementInfo) -> Option<Txid> {
    match info.status {
        ReimbursementStatus::Slashed | ReimbursementStatus::Aborted => Some(info.claim_txid),
        ReimbursementStatus::Complete => info.payout_txid,
        ReimbursementStatus::InProgress | ReimbursementStatus::NotStarted => None,
    }
}

/// Return latest bridge status extracted from cache.
pub async fn get_bridge_status(
    context: Arc<BridgeMonitoringContext>,
) -> std::result::Result<Json<BridgeStatus>, StatusCode> {
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

    Ok(Json(context.bridge_status().await))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_deposit_indices_from_cursor() {
        let cases = [
            (&[0, 1, 3][..], 0, 2),
            (&[0, 1, 2, 3][..], 0, 4),
            (&[0, 1, 3][..], 2, 0),
        ];

        for (deposit_indices, next_deposit_idx, expected) in cases {
            assert_eq!(
                count_deposit_indices_from(deposit_indices, next_deposit_idx),
                expected
            );
        }
    }
}
