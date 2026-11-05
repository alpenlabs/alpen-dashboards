use anyhow::Result;
use axum::http::StatusCode;
use axum::Json;
use bitcoin::Txid;
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
        ReimbursementStatus, WithdrawalPairing, WithdrawalPairingCursor,
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

        for operator in context.config().operators() {
            let operator_pk = *operator.public_key();
            let status = bridge_rpc::get_operator_status(context.bridge_rpc(), operator_pk).await;
            operator_statuses.push(OperatorStatus::new(
                operator.name().to_owned(),
                operator_pk,
                status,
            ));
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
        let pairing_cursor = context.state().withdrawal_pairing_cursor().await;
        let withdrawal_pairing_batch_size = context.config().withdrawal_pairing_batch_size();
        let pairing_deposit_candidates = select_pairing_deposit_candidates(
            &deposit_indices,
            pairing_cursor,
            withdrawal_pairing_batch_size,
        );
        let all_deposit_candidates = deposit_candidates
            .iter()
            .chain(pairing_deposit_candidates.iter())
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let deposit_infos = get_deposits(context.bridge_rpc(), &all_deposit_candidates).await;
        let deposit_candidates = deposit_candidates.into_iter().collect::<BTreeSet<_>>();
        let deposit_info_updates = get_deposit_info_updates(
            context.esplora(),
            chain_tip_height,
            deposit_infos
                .iter()
                .copied()
                .filter(|(deposit_idx, _)| deposit_candidates.contains(deposit_idx))
                .collect(),
        )
        .await;
        if let Err(e) = context
            .state()
            .apply_deposit_info_updates(
                context.status_db(),
                deposit_info_updates,
                context.config().max_tx_confirmations(),
            )
            .await
        {
            warn!(error = %e, "failed to persist deposit status updates");
        }

        pair_indexed_withdrawals(
            &context,
            &deposit_indices,
            pairing_cursor,
            pairing_deposit_candidates,
            deposit_infos,
        )
        .await;

        let withdrawal_candidates = context.state().select_withdrawal_status_candidates().await;
        let withdrawal_updates = get_withdrawal_updates(
            context.bridge_rpc(),
            context.withdrawal_index_db(),
            context.esplora(),
            chain_tip_height,
            &withdrawal_candidates,
            withdrawal_pairing_batch_size,
        )
        .await;
        if let Err(e) = context
            .state()
            .apply_withdrawal_updates(
                context.status_db(),
                withdrawal_updates,
                context.config().max_tx_confirmations(),
            )
            .await
        {
            warn!(error = %e, "failed to persist withdrawal status updates");
        }

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
        if let Err(e) = context
            .state()
            .apply_reimbursement_updates(
                context.status_db(),
                reimbursement_updates,
                context.config().max_tx_confirmations(),
            )
            .await
        {
            warn!(error = %e, "failed to persist reimbursement status updates");
        }

        context.mark_status_available();
    }

    Ok(())
}

/// Pairs indexed withdrawal requests with candidate deposits and persists the pairings.
async fn pair_indexed_withdrawals(
    context: &BridgeMonitoringContext,
    deposit_indices: &[DepositIdx],
    pairing_cursor: WithdrawalPairingCursor,
    pairing_deposit_candidates: Vec<DepositIdx>,
    deposit_infos: Vec<(DepositIdx, DepositInfo)>,
) -> Vec<WithdrawalPairing> {
    let pairing_deposit_candidates = pairing_deposit_candidates
        .into_iter()
        .collect::<BTreeSet<_>>();
    let pairing_deposit_infos = deposit_infos
        .into_iter()
        .filter(|(deposit_idx, _)| pairing_deposit_candidates.contains(deposit_idx))
        .collect::<Vec<_>>();
    let withdrawal_requests = fetch_withdrawal_requests(
        context.withdrawal_index_db(),
        pairing_cursor.next_withdrawal_seq,
        pairing_deposit_candidates.len(),
        context.config().withdrawal_pairing_batch_size(),
    );
    let new_pairings = match context
        .state()
        .advance_withdrawal_pairings(
            context.status_db(),
            deposit_indices,
            &pairing_deposit_infos,
            &withdrawal_requests,
        )
        .await
    {
        Ok(pairings) => pairings,
        Err(e) => {
            warn!(error = %e, "failed to persist withdrawal pairings");
            Vec::new()
        }
    };
    if !new_pairings.is_empty() {
        info!(
            pairing_count = new_pairings.len(),
            "paired indexed withdrawals with deposits"
        );
    }
    new_pairings
}

/// Returns up to `batch_size` contiguous deposit indices starting at the pairing cursor.
fn select_pairing_deposit_candidates(
    deposit_indices: &[DepositIdx],
    pairing_cursor: WithdrawalPairingCursor,
    batch_size: usize,
) -> Vec<DepositIdx> {
    // This collects the contiguous deposit-index window known to the bridge.
    // The state pairing planner additionally checks deposit status and skips
    // failed deposits without consuming withdrawal requests.
    let deposit_indices = deposit_indices.iter().copied().collect::<BTreeSet<_>>();
    let mut next_deposit_idx = pairing_cursor.next_deposit_idx;
    let mut indices = Vec::new();

    while indices.len() < batch_size && deposit_indices.contains(&next_deposit_idx) {
        indices.push(next_deposit_idx);
        let Some(next) = next_deposit_idx.checked_add(1) else {
            break;
        };
        next_deposit_idx = next;
    }

    indices
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
    use std::{future::Future, task::Poll};

    use bitcoin::hashes::Hash;

    use crate::{
        test_utils::{make_deposit_info, BridgeStatusFixture},
        types::ChallengeStep,
    };

    /// Runs the pairing step for completed deposits from the current pairing cursor.
    async fn pair_completed_deposits(
        fixture: &BridgeStatusFixture,
        deposit_indices: &[DepositIdx],
    ) -> Vec<WithdrawalPairing> {
        let pairing_cursor = fixture.state().withdrawal_pairing_cursor().await;
        let candidates = select_pairing_deposit_candidates(
            deposit_indices,
            pairing_cursor,
            fixture.context().config().withdrawal_pairing_batch_size(),
        );
        let deposit_infos = candidates
            .iter()
            .map(|deposit_idx| (*deposit_idx, make_deposit_info(DepositStatus::Complete)))
            .collect();
        pair_indexed_withdrawals(
            fixture.context(),
            deposit_indices,
            pairing_cursor,
            candidates,
            deposit_infos,
        )
        .await
    }

    #[tokio::test]
    async fn indexed_withdrawals_pair_with_completed_deposits() {
        let fixture = BridgeStatusFixture::default();
        fixture.index_withdrawal_event(1);
        fixture.index_withdrawal_event(2);

        let pairings = pair_completed_deposits(&fixture, &[0, 1]).await;

        let expected = vec![WithdrawalPairing::new(0, 0), WithdrawalPairing::new(1, 1)];
        assert_eq!(pairings, expected);
        assert_eq!(fixture.snapshot().withdrawal_pairings, expected);
        assert_eq!(
            fixture.state().withdrawal_pairing_cursor().await,
            WithdrawalPairingCursor::new(2, 2)
        );
    }

    #[tokio::test]
    async fn pairing_waits_until_withdrawals_are_indexed() {
        let fixture = BridgeStatusFixture::default();

        let pairings = pair_completed_deposits(&fixture, &[0, 1]).await;

        assert!(pairings.is_empty());
        assert!(fixture.snapshot().withdrawal_pairings.is_empty());
        assert_eq!(
            fixture.state().withdrawal_pairing_cursor().await,
            WithdrawalPairingCursor::default()
        );
    }

    #[tokio::test]
    async fn pairing_resumes_from_cursor_withdrawal_seq() {
        let fixture = BridgeStatusFixture::default();
        fixture.index_withdrawal_event(1);
        assert_eq!(
            pair_completed_deposits(&fixture, &[0, 1]).await,
            vec![WithdrawalPairing::new(0, 0)]
        );

        fixture.index_withdrawal_event(2);
        let pairings = pair_completed_deposits(&fixture, &[0, 1]).await;

        assert_eq!(pairings, vec![WithdrawalPairing::new(1, 1)]);
        assert_eq!(
            fixture.snapshot().withdrawal_pairings,
            vec![WithdrawalPairing::new(0, 0), WithdrawalPairing::new(1, 1)]
        );
    }

    #[tokio::test]
    async fn indexed_withdrawals_survive_restart_and_pair() {
        let mut fixture = BridgeStatusFixture::default();
        fixture.index_withdrawal_event(1);
        fixture.index_withdrawal_event(2);

        fixture.restart();
        let pairings = pair_completed_deposits(&fixture, &[0, 1]).await;

        assert_eq!(
            pairings,
            vec![WithdrawalPairing::new(0, 0), WithdrawalPairing::new(1, 1)]
        );
    }

    #[test]
    fn pairing_deposit_candidates_stop_at_first_gap_or_batch_size() {
        let cases = [
            (&[0, 1, 3][..], 0, 10, vec![0, 1]),
            (&[0, 1, 2, 3][..], 0, 10, vec![0, 1, 2, 3]),
            (&[0, 1, 2, 3][..], 1, 2, vec![1, 2]),
            (&[0, 1, 3][..], 2, 10, vec![]),
        ];

        for (deposit_indices, next_deposit_idx, batch_size, expected) in cases {
            assert_eq!(
                select_pairing_deposit_candidates(
                    deposit_indices,
                    WithdrawalPairingCursor::new(next_deposit_idx, 0),
                    batch_size,
                ),
                expected
            );
        }
    }

    #[test]
    fn terminal_reimbursement_txid_selects_txid_that_retires_the_row() {
        let claim_txid = Txid::from_byte_array([1; 32]);
        let payout_txid = Txid::from_byte_array([2; 32]);
        let cases = [
            (ReimbursementStatus::Slashed, Some(claim_txid)),
            (ReimbursementStatus::Aborted, Some(claim_txid)),
            (ReimbursementStatus::Complete, Some(payout_txid)),
            (ReimbursementStatus::InProgress, None),
            (ReimbursementStatus::NotStarted, None),
        ];

        for (status, expected) in cases {
            let info = ReimbursementInfo {
                claim_txid,
                challenge_step: ChallengeStep::NotApplicable,
                payout_txid: Some(payout_txid),
                status,
            };
            assert_eq!(terminal_reimbursement_txid(&info), expected);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn bridge_status_is_unavailable_before_first_status() {
        // Paused time lets the readiness timeout elapse without waiting.
        let fixture = BridgeStatusFixture::default();

        let response = get_bridge_status(Arc::clone(fixture.context())).await;

        assert_eq!(response.err(), Some(StatusCode::SERVICE_UNAVAILABLE));
    }

    #[tokio::test]
    async fn bridge_status_is_served_after_first_status() {
        let fixture = BridgeStatusFixture::default();
        let info = make_deposit_info(DepositStatus::Complete);
        fixture.apply_deposit_update(0, info, Some(1)).await;
        fixture.context().mark_status_available();

        let status = get_bridge_status(Arc::clone(fixture.context()))
            .await
            .expect("bridge status")
            .0;

        assert_eq!(status.deposits.len(), 1);
        assert_eq!(
            status.deposits[0].deposit_request_txid,
            info.deposit_request_txid
        );
        assert_eq!(status.deposits[0].deposit_txid, info.deposit_txid);
    }

    #[tokio::test]
    async fn waiting_bridge_status_request_returns_status_once_available() {
        let fixture = BridgeStatusFixture::default();
        let mut request = std::pin::pin!(get_bridge_status(Arc::clone(fixture.context())));
        assert!(
            std::future::poll_fn(|cx| Poll::Ready(request.as_mut().poll(cx).is_pending())).await,
            "request must wait for first status"
        );

        let info = make_deposit_info(DepositStatus::Complete);
        fixture.apply_deposit_update(0, info, Some(1)).await;
        fixture.context().mark_status_available();

        let status = request.await.expect("bridge status").0;
        assert_eq!(status.deposits.len(), 1);
        assert_eq!(
            status.deposits[0].deposit_request_txid,
            info.deposit_request_txid
        );
    }
}
