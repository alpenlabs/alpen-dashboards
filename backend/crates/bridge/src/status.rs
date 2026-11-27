use anyhow::Result;
use axum::http::StatusCode;
use axum::Json;
use bitcoin::Txid;
use std::{collections::BTreeSet, sync::Arc};
use strata_bridge_primitives::types::DepositIdx;
use strata_bridge_rpc::types::RpcOperatorStatus;
use strata_primitives::L1Height;
use strata_tasks::ShutdownGuard;

use super::{
    bridge_rpc,
    context::BridgeMonitoringContext,
    l1::esplora::EsploraClient,
    state::{BridgeMonitoringState, DepositInfoUpdate, ReimbursementInfoUpdate},
    types::{
        BridgeStatus, DepositInfo, DepositStatus, OperatorStatus, ReimbursementInfo,
        ReimbursementStatus, WithdrawalPairing, WithdrawalPairingCursor,
    },
    withdrawal_requests::fetch_withdrawal_requests,
    withdrawal_status::get_withdrawal_updates,
};

use tokio::time::{interval, timeout, Duration};
use tracing::{error, info, warn};

/// Deposit indices selected for status updates and withdrawal pairing.
struct DepositRefreshCandidates {
    status_candidates: BTreeSet<DepositIdx>,
    pairing_candidates: WithdrawalPairingCandidates,
}

impl DepositRefreshCandidates {
    /// Returns the deduplicated indices needed by status refresh and pairing.
    fn deposit_indices(&self) -> Vec<DepositIdx> {
        self.status_candidates
            .union(&self.pairing_candidates.deposit_indices)
            .copied()
            .collect()
    }
}

/// Pairing cursor and candidate deposit indices.
struct WithdrawalPairingCandidates {
    cursor: WithdrawalPairingCursor,
    deposit_indices: BTreeSet<DepositIdx>,
}

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

        refresh_bridge_status(&context).await;

        context.mark_initial_status_poll_completed();
    }

    Ok(())
}

/// Refreshes operator, deposit, withdrawal, and reimbursement statuses.
async fn refresh_bridge_status(context: &BridgeMonitoringContext) {
    refresh_operator_statuses(context).await;

    let tip = fetch_chain_tip(context).await;
    let indices = fetch_deposit_indices(context).await;

    if let Some(indices) = &indices {
        let candidates = select_deposit_refresh_candidates(
            context.state(),
            indices,
            tip,
            context.config().withdrawal_pairing_batch_size(),
        )
        .await;
        let deposit_infos =
            get_deposit_infos(context.bridge_rpc(), &candidates.deposit_indices()).await;

        // Deposit statuses need confirmation counts, which require the current Bitcoin tip.
        if let Some(tip) = tip {
            refresh_deposit_statuses(context, tip, &candidates.status_candidates, &deposit_infos)
                .await;
        }

        // Pairing uses bridge answers and indexed requests, so it can proceed without a tip.
        pair_indexed_withdrawals(
            context,
            indices,
            &candidates.pairing_candidates,
            &deposit_infos,
        )
        .await;
    }

    if let Some(tip) = tip {
        // Refresh withdrawal statuses, including withdrawals paired during this poll.
        refresh_withdrawal_statuses(context, tip).await;

        // Refresh reimbursement statuses, including withdrawals that completed during this poll.
        refresh_reimbursement_statuses(context, tip).await;
    }

    // Without the tip, we skipped deposit, withdrawal and reimbursement status updates.
    // Without deposit indices, we skipped deposit status updates and withdrawal pairing.
    if tip.is_none() || indices.is_none() {
        warn!("bridge status refresh will retry on the next poll");
    }
}

async fn refresh_operator_statuses(context: &BridgeMonitoringContext) {
    let mut operator_statuses = Vec::new();

    for operator in context.config().operators() {
        let operator_pk = *operator.public_key();
        let status = match bridge_rpc::get_operator_status(context.bridge_rpc(), operator_pk).await
        {
            Ok(status) => status,
            Err(error) => {
                warn!(%operator_pk, %error, "failed to fetch bridge operator status");
                // Operator status has no unknown value; failed checks are reported as offline.
                RpcOperatorStatus::Offline
            }
        };
        operator_statuses.push(OperatorStatus::new(
            operator.name().to_owned(),
            operator_pk,
            status,
        ));
    }

    context.state().update_operators(operator_statuses).await;
}

async fn fetch_chain_tip(context: &BridgeMonitoringContext) -> Option<L1Height> {
    match context.esplora().get_tip_height().await {
        Ok(chain_tip_height) => {
            info!(%chain_tip_height, "bitcoin chain tip");
            Some(chain_tip_height)
        }
        Err(error) => {
            error!(%error, "failed to get Bitcoin chain tip");
            None
        }
    }
}

async fn fetch_deposit_indices(context: &BridgeMonitoringContext) -> Option<Vec<DepositIdx>> {
    match bridge_rpc::get_deposit_indices(context.bridge_rpc()).await {
        Ok(indices) => Some(indices),
        Err(error) => {
            error!(%error, "failed to fetch deposit indices");
            None
        }
    }
}

/// Selects deposit-status and pairing candidates from the current monitoring state.
async fn select_deposit_refresh_candidates(
    state: &BridgeMonitoringState,
    deposit_indices: &[DepositIdx],
    tip: Option<L1Height>,
    pairing_batch_size: usize,
) -> DepositRefreshCandidates {
    // Without a tip, only pairing needs deposit info; status-only candidates wait for another poll.
    let status_candidates: BTreeSet<_> = if tip.is_some() {
        state
            .select_deposit_info_candidates(deposit_indices)
            .await
            .into_iter()
            .collect()
    } else {
        BTreeSet::new()
    };
    let pairing_cursor = state.withdrawal_pairing_cursor().await;
    let pairing_candidates: BTreeSet<_> =
        select_pairing_deposit_candidates(deposit_indices, pairing_cursor, pairing_batch_size)
            .into_iter()
            .collect();
    DepositRefreshCandidates {
        status_candidates,
        pairing_candidates: WithdrawalPairingCandidates {
            cursor: pairing_cursor,
            deposit_indices: pairing_candidates,
        },
    }
}

async fn refresh_deposit_statuses(
    context: &BridgeMonitoringContext,
    tip: L1Height,
    candidates: &BTreeSet<DepositIdx>,
    deposit_infos: &[(DepositIdx, DepositInfo)],
) {
    let updates = get_deposit_info_updates(
        context.esplora(),
        tip,
        deposit_infos
            .iter()
            .copied()
            .filter(|(deposit_idx, _)| candidates.contains(deposit_idx))
            .collect(),
    )
    .await;
    if let Err(error) = context
        .state()
        .apply_deposit_info_updates(
            context.status_db(),
            updates,
            context.config().max_tx_confirmations(),
        )
        .await
    {
        warn!(%error, "failed to persist deposit status updates");
    }
}

async fn refresh_withdrawal_statuses(context: &BridgeMonitoringContext, tip: L1Height) {
    let candidates = context.state().select_withdrawal_status_candidates().await;
    let updates = get_withdrawal_updates(
        context.bridge_rpc(),
        context.withdrawal_index_db(),
        context.esplora(),
        tip,
        &candidates,
        context.config().withdrawal_pairing_batch_size(),
    )
    .await;
    if let Err(error) = context
        .state()
        .apply_withdrawal_updates(
            context.status_db(),
            updates,
            context.config().max_tx_confirmations(),
        )
        .await
    {
        warn!(%error, "failed to persist withdrawal status updates");
    }
}

async fn refresh_reimbursement_statuses(context: &BridgeMonitoringContext, tip: L1Height) {
    let candidates = context
        .state()
        .select_reimbursement_status_candidates()
        .await;
    let updates =
        get_reimbursement_updates(context.bridge_rpc(), context.esplora(), tip, &candidates).await;
    if let Err(error) = context
        .state()
        .apply_reimbursement_updates(
            context.status_db(),
            updates,
            context.config().max_tx_confirmations(),
        )
        .await
    {
        warn!(%error, "failed to persist reimbursement status updates");
    }
}

/// Pairs indexed withdrawal requests with candidate deposits and persists the pairings.
async fn pair_indexed_withdrawals(
    context: &BridgeMonitoringContext,
    known_deposit_indices: &[DepositIdx],
    candidates: &WithdrawalPairingCandidates,
    deposit_infos: &[(DepositIdx, DepositInfo)],
) -> Vec<WithdrawalPairing> {
    let pairing_deposit_infos = deposit_infos
        .iter()
        .copied()
        .filter(|(deposit_idx, _)| candidates.deposit_indices.contains(deposit_idx))
        .collect::<Vec<_>>();
    let withdrawal_requests = fetch_withdrawal_requests(
        context.withdrawal_index_db(),
        candidates.cursor.next_withdrawal_seq,
        candidates.deposit_indices.len(),
        context.config().withdrawal_pairing_batch_size(),
    );
    let new_pairings = match context
        .state()
        .advance_withdrawal_pairings(
            context.status_db(),
            known_deposit_indices,
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

/// Fetches bridge deposit info for the supplied indices, omitting failed lookups.
async fn get_deposit_infos(
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

        let current_confirmations = match esplora_client
            .get_tx_confirmations(check_txid, chain_tip_height)
            .await
        {
            Ok(confirmations) => Some(confirmations),
            Err(error) => {
                warn!(deposit_idx, %check_txid, %error, "failed to fetch deposit confirmations");
                // Keep the observed status, but replace the previous count with unknown.
                None
            }
        };
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
                match esplora_client
                    .get_tx_confirmations(txid, chain_tip_height)
                    .await
                {
                    Ok(confirmations) => Some(confirmations),
                    Err(error) => {
                        warn!(deposit_idx, %txid, %error, "failed to fetch reimbursement confirmations");
                        // Keep the observed status, but replace the previous count with unknown.
                        None
                    }
                }
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
    use std::{
        future::Future,
        sync::atomic::{AtomicBool, Ordering},
        task::Poll,
    };

    use axum::{routing::get, Router};
    use bitcoin::{
        hashes::Hash,
        secp256k1::{Secp256k1, SecretKey},
        PublicKey,
    };
    use serde_json::{json, Value};
    use status_config::{
        test_utils::{make_bridge_config, make_bridge_operator},
        BridgeMonitoringConfig,
    };
    use status_utils::test_utils::{make_rpc_error_reply, TestRpcServer};
    use strata_bridge_rpc::types::{
        RpcClaimPhase, RpcDepositInfo, RpcDepositStatus, RpcReimbursementStatus,
        RpcWithdrawalStatus,
    };
    use strata_tasks::TaskManager;
    use tokio::{runtime::Handle, sync::oneshot};

    use crate::{
        test_utils::{make_deposit_info, make_reimbursement_info, BridgeStatusFixture},
        types::{ChallengeStep, WithdrawalStatus},
    };

    const TEST_CHAIN_TIP_HEIGHT: L1Height = 100;
    const TEST_MONITORING_TIMEOUT: Duration = Duration::from_secs(5);

    fn make_monitoring_config(server: &TestRpcServer) -> BridgeMonitoringConfig {
        let secret = SecretKey::from_slice(&[1; 32]).expect("test secret key");
        let key = PublicKey::new(secret.public_key(&Secp256k1::new()));
        let operator = make_bridge_operator("Operator", &key, server.url());
        make_bridge_config(server.url(), &[operator], Some(0))
    }

    fn make_monitoring_fixture(server: &TestRpcServer) -> BridgeStatusFixture {
        BridgeStatusFixture::with_config(make_monitoring_config(server))
    }

    async fn make_monitoring_server(
        replies: Vec<Value>,
        fail_tip: Arc<AtomicBool>,
    ) -> TestRpcServer {
        let router = Router::new()
            .route(
                "/blocks/tip/height",
                get(move || {
                    let fail_tip = Arc::clone(&fail_tip);
                    async move {
                        if fail_tip.load(Ordering::Relaxed) {
                            (StatusCode::SERVICE_UNAVAILABLE, String::new())
                        } else {
                            (StatusCode::OK, TEST_CHAIN_TIP_HEIGHT.to_string())
                        }
                    }
                }),
            )
            .route(
                "/tx/{txid}/status",
                get(|| async {
                    Json(json!({"confirmed": true, "block_height": TEST_CHAIN_TIP_HEIGHT}))
                }),
            );
        TestRpcServer::with_routes(replies, router).await
    }

    fn make_completed_deposit_reply() -> Value {
        let info = make_deposit_info(DepositStatus::Complete);
        json!({"result": RpcDepositInfo {
            status: RpcDepositStatus::Complete { deposit_txid: info.deposit_txid.expect("completed deposit") },
            deposit_idx: 0,
            deposit_request_txid: info.deposit_request_txid,
        }})
    }

    #[tokio::test]
    async fn operator_failure_reports_offline_and_continues_refresh() {
        let server = make_monitoring_server(
            vec![
                make_rpc_error_reply(),
                json!({"result": [0]}),
                make_completed_deposit_reply(),
            ],
            Arc::new(AtomicBool::new(false)),
        )
        .await;
        let fixture = make_monitoring_fixture(&server);

        // The configured operator's uptime call fails, but deposit monitoring still succeeds.
        refresh_bridge_status(fixture.context()).await;
        let status = fixture.bridge_status().await;
        assert_eq!(status.operators.len(), 1);
        assert!(matches!(
            status.operators[0].status,
            RpcOperatorStatus::Offline
        ));
        assert_eq!(status.deposits.len(), 1);
        assert!(matches!(status.deposits[0].status, DepositStatus::Complete));
    }

    // Verifies that RPC failures leave the dashboard's previously known statuses visible.
    #[tokio::test]
    async fn failed_status_lookups_keep_rows_and_cursors() {
        let previous_reimbursement = make_reimbursement_info(4, ReimbursementStatus::InProgress);
        // Keep the tip and deposit indices available so the poll attempts all three status queries.
        let server = make_monitoring_server(
            vec![
                json!({"result": 1}),   // Operator uptime.
                json!({"result": [0]}), // Known deposit indices.
                make_rpc_error_reply(), // Deposit info lookup fails.
                make_rpc_error_reply(), // Withdrawal status lookup fails.
                make_rpc_error_reply(), // Reimbursement status lookup fails.
            ],
            Arc::new(AtomicBool::new(false)),
        )
        .await;
        let fixture = make_monitoring_fixture(&server);

        // Seed what an earlier poll observed: a completed deposit and withdrawal still visible
        // at one confirmation, and a reimbursement whose claim is in progress.
        fixture.index_withdrawal_event(1);
        let deposit = make_deposit_info(DepositStatus::Complete);
        fixture.apply_deposit_update(0, deposit, Some(1)).await;
        let previous_withdrawal = fixture.seed_completed_withdrawal(1).await;
        fixture
            .apply_reimbursement_update(0, previous_reimbursement, None)
            .await;
        let cursors = fixture.snapshot().cursors;

        // Run one poll with failed status queries against the existing state.
        refresh_bridge_status(fixture.context()).await;

        // The previous rows must remain visible with the same transaction IDs and statuses.
        let status = fixture.bridge_status().await;
        assert_eq!(status.deposits.len(), 1);
        assert_eq!(
            status.deposits[0].deposit_request_txid,
            deposit.deposit_request_txid
        );
        assert_eq!(status.deposits[0].deposit_txid, deposit.deposit_txid);
        assert!(matches!(status.deposits[0].status, DepositStatus::Complete));
        assert_eq!(status.withdrawals.len(), 1);
        assert_eq!(
            status.withdrawals[0].withdrawal_request_txid,
            previous_withdrawal.withdrawal_request_txid
        );
        assert_eq!(
            status.withdrawals[0].fulfillment_txid,
            previous_withdrawal.fulfillment_txid
        );
        assert!(matches!(
            status.withdrawals[0].status,
            WithdrawalStatus::Complete
        ));
        assert_eq!(status.reimbursements, vec![previous_reimbursement]);

        // With no new status results, the polling cursors must also remain unchanged.
        assert_eq!(fixture.snapshot().cursors, cursors);
    }

    // Verifies that null RPC responses leave previously cached withdrawal and reimbursement
    // statuses unchanged.
    #[tokio::test]
    async fn absent_status_answers_keep_rows_and_cursors() {
        let previous_reimbursement = make_reimbursement_info(4, ReimbursementStatus::InProgress);
        // Return valid null responses for withdrawal and reimbursement queries, not RPC errors.
        let server = make_monitoring_server(
            vec![
                json!({"result": 1}),    // Operator uptime.
                json!({"result": []}),   // No deposit info queries this poll.
                json!({"result": null}), // No withdrawal status returned.
                json!({"result": null}), // No reimbursement status returned.
            ],
            Arc::new(AtomicBool::new(false)),
        )
        .await;
        let fixture = make_monitoring_fixture(&server);

        // Seed a completed withdrawal still visible at one confirmation and an in-progress claim.
        // These existing records remain query candidates even when no deposit indices are returned.
        fixture.index_withdrawal_event(1);
        let previous_withdrawal = fixture.seed_completed_withdrawal(1).await;
        fixture
            .apply_reimbursement_update(0, previous_reimbursement, None)
            .await;
        let cursors = fixture.snapshot().cursors;

        // Run one poll whose status queries succeed but return no status.
        refresh_bridge_status(fixture.context()).await;

        // Neither null response should remove a row or replace its known transaction IDs or status.
        let status = fixture.bridge_status().await;
        assert_eq!(status.withdrawals.len(), 1);
        assert_eq!(
            status.withdrawals[0].withdrawal_request_txid,
            previous_withdrawal.withdrawal_request_txid
        );
        assert_eq!(
            status.withdrawals[0].fulfillment_txid,
            previous_withdrawal.fulfillment_txid
        );
        assert!(matches!(
            status.withdrawals[0].status,
            WithdrawalStatus::Complete
        ));
        assert_eq!(status.reimbursements, vec![previous_reimbursement]);

        // With no new status results, the polling cursors must also remain unchanged.
        assert_eq!(fixture.snapshot().cursors, cursors);
    }

    #[tokio::test]
    async fn failed_confirmations_keep_completion_without_advancing_cursors() {
        let previous_reimbursement = make_reimbursement_info(4, ReimbursementStatus::InProgress);
        let fulfillment_txid = Txid::from_byte_array([3; 32]);
        let claim_txid = previous_reimbursement.claim_txid;
        let payout_txid = Txid::from_byte_array([5; 32]);
        let withdrawal = json!({"result": RpcWithdrawalStatus::Complete { fulfillment_txid }});
        let reimbursement =
            json!({"result": RpcReimbursementStatus::Complete { claim_txid, payout_txid }});
        let server = TestRpcServer::with_routes(
            // One poll: uptime, indices, then successful deposit, withdrawal and reimbursement statuses.
            vec![
                json!({"result": 1}),
                json!({"result": [0]}),
                make_completed_deposit_reply(),
                withdrawal,
                reimbursement,
            ],
            // The tip is available, but transaction confirmation requests return HTTP 404.
            Router::new().route(
                "/blocks/tip/height",
                get(|| async { TEST_CHAIN_TIP_HEIGHT.to_string() }),
            ),
        )
        .await;
        let fixture = make_monitoring_fixture(&server);
        fixture.index_withdrawal_event(1);
        let deposit = make_deposit_info(DepositStatus::Complete);
        fixture.apply_deposit_update(0, deposit, Some(1)).await;
        fixture.seed_completed_withdrawal(1).await;
        fixture
            .apply_reimbursement_update(0, previous_reimbursement, None)
            .await;
        let cursors = fixture.snapshot().cursors;

        // Successful completion answers with failed confirmations replace the old counts with
        // unknown. Terminal rows are hidden, but remain eligible for the next poll.
        refresh_bridge_status(fixture.context()).await;
        let status = fixture.bridge_status().await;
        assert!(status.deposits.is_empty());
        assert!(status.withdrawals.is_empty());
        assert!(status.reimbursements.is_empty());
        let snapshot = fixture.snapshot();
        assert_eq!(snapshot.cursors, cursors);
        assert_eq!(snapshot.withdrawals.len(), 1);
        assert_eq!(
            snapshot.withdrawals[0].1.fulfillment_txid,
            Some(fulfillment_txid)
        );
        assert!(matches!(
            snapshot.withdrawals[0].1.status,
            WithdrawalStatus::Complete
        ));
        assert_eq!(
            fixture.state().select_deposit_info_candidates(&[0]).await,
            vec![0]
        );
        assert_eq!(
            fixture.state().select_withdrawal_status_candidates().await,
            vec![WithdrawalPairing::new(0, 0)]
        );
        assert_eq!(
            fixture
                .state()
                .select_reimbursement_status_candidates()
                .await,
            vec![0]
        );
    }

    #[tokio::test]
    async fn deposit_index_failure_does_not_skip_withdrawals_or_reimbursements() {
        let fulfillment_txid = Txid::from_byte_array([3; 32]);
        let claim_txid = Txid::from_byte_array([4; 32]);
        let withdrawal_reply =
            json!({"result": RpcWithdrawalStatus::Complete { fulfillment_txid }});
        let reimbursement_reply = json!({"result": RpcReimbursementStatus::InProgress {
            claim_txid, phase: RpcClaimPhase::Claimed,
        }});
        let server = make_monitoring_server(
            vec![
                json!({"result": 1}),
                make_rpc_error_reply(),
                withdrawal_reply.clone(),
                reimbursement_reply.clone(),
                json!({"result": 1}),
                json!({"result": [0]}),
                make_completed_deposit_reply(),
                withdrawal_reply,
                reimbursement_reply,
            ],
            Arc::new(AtomicBool::new(false)),
        )
        .await;
        let fixture = make_monitoring_fixture(&server);
        fixture.index_withdrawal_event(1);
        pair_completed_deposits(&fixture, &[0]).await;
        let deposit = make_deposit_info(DepositStatus::Complete);
        fixture.apply_deposit_update(0, deposit, Some(1)).await;
        let pairing_cursor = fixture.state().withdrawal_pairing_cursor().await;

        // Discovery fails, but the already paired withdrawal can complete and expose its claim.
        refresh_bridge_status(fixture.context()).await;
        let status = fixture.bridge_status().await;
        assert_eq!(status.deposits.len(), 1);
        assert_eq!(
            status.deposits[0].deposit_request_txid,
            deposit.deposit_request_txid
        );
        assert_eq!(status.deposits[0].deposit_txid, deposit.deposit_txid);
        assert!(matches!(status.deposits[0].status, DepositStatus::Complete));
        assert_eq!(status.withdrawals.len(), 1);
        assert_eq!(
            status.withdrawals[0].fulfillment_txid,
            Some(fulfillment_txid)
        );
        assert!(matches!(
            status.withdrawals[0].status,
            WithdrawalStatus::Complete
        ));
        assert_eq!(status.reimbursements.len(), 1);
        assert_eq!(status.reimbursements[0].claim_txid, claim_txid);
        assert_eq!(
            status.reimbursements[0].status,
            ReimbursementStatus::InProgress
        );
        assert_eq!(
            fixture.state().withdrawal_pairing_cursor().await,
            pairing_cursor
        );
        assert!(!server
            .methods()
            .iter()
            .any(|method| method == "stratabridge_depositInfo"));

        // The next successful poll resumes discovery without duplicating the existing pairing.
        refresh_bridge_status(fixture.context()).await;
        assert_eq!(
            fixture.snapshot().withdrawal_pairings,
            vec![WithdrawalPairing::new(0, 0)]
        );
        let recovered = fixture.bridge_status().await;
        assert_eq!(recovered.withdrawals.len(), 1);
        assert_eq!(
            recovered.withdrawals[0].fulfillment_txid,
            Some(fulfillment_txid)
        );
        assert!(server
            .methods()
            .iter()
            .any(|method| method == "stratabridge_depositInfo"));
    }

    #[tokio::test]
    async fn tip_failure_preserves_rows_and_allows_pairing() {
        let fail_tip = Arc::new(AtomicBool::new(true));
        let fulfillment_txid = Txid::from_byte_array([3; 32]);
        let server = make_monitoring_server(
            vec![
                json!({"result": 1}),
                json!({"result": [0]}),
                make_completed_deposit_reply(),
                json!({"result": 1}),
                json!({"result": [0]}),
                make_completed_deposit_reply(),
                json!({"result": RpcWithdrawalStatus::Complete { fulfillment_txid }}),
                json!({"result": null}),
            ],
            Arc::clone(&fail_tip),
        )
        .await;
        let fixture = make_monitoring_fixture(&server);
        fixture.index_withdrawal_event(1);
        let deposit = make_deposit_info(DepositStatus::Complete);
        fixture.apply_deposit_update(0, deposit, Some(1)).await;
        let before = fixture.snapshot();

        // The tip is unavailable: keep cached confirmations, but pair from bridge/indexed data.
        refresh_bridge_status(fixture.context()).await;
        let status = fixture.bridge_status().await;
        assert_eq!(status.deposits.len(), 1);
        assert_eq!(
            status.deposits[0].deposit_request_txid,
            deposit.deposit_request_txid
        );
        assert_eq!(status.deposits[0].deposit_txid, deposit.deposit_txid);
        assert!(matches!(status.deposits[0].status, DepositStatus::Complete));
        assert_eq!(
            fixture.snapshot().withdrawal_pairings,
            vec![WithdrawalPairing::new(0, 0)]
        );
        assert_eq!(
            fixture.state().withdrawal_pairing_cursor().await,
            WithdrawalPairingCursor::new(1, 1)
        );
        assert_eq!(
            fixture.snapshot().cursors.withdrawal_status,
            before.cursors.withdrawal_status
        );
        assert_eq!(
            fixture.snapshot().cursors.reimbursement_status,
            before.cursors.reimbursement_status
        );
        assert!(fixture.bridge_status().await.withdrawals.is_empty());
        let methods = server.methods();
        assert!(methods
            .iter()
            .any(|method| method == "stratabridge_depositInfo"));
        assert!(!methods
            .iter()
            .any(|method| method == "stratabridge_withdrawalStatus"));
        assert!(!methods
            .iter()
            .any(|method| method == "stratabridge_reimbursementStatus"));

        // Once the tip recovers, the existing pairing is used to fetch fulfillment.
        fail_tip.store(false, Ordering::Relaxed);
        refresh_bridge_status(fixture.context()).await;
        let status = fixture.bridge_status().await;
        assert_eq!(status.withdrawals.len(), 1);
        assert_eq!(
            status.withdrawals[0].fulfillment_txid,
            Some(fulfillment_txid)
        );
        assert_eq!(
            fixture.snapshot().withdrawal_pairings,
            vec![WithdrawalPairing::new(0, 0)]
        );
    }

    #[tokio::test]
    async fn tip_failure_fetches_only_pairing_candidates() {
        let server = make_monitoring_server(
            vec![
                json!({"result": 1}),
                json!({"result": [0, 1]}),
                make_completed_deposit_reply(),
            ],
            Arc::new(AtomicBool::new(true)),
        )
        .await;
        let config = make_monitoring_config(&server).with_withdrawal_pairing_batch_size(1);
        let fixture = BridgeStatusFixture::with_config(config);

        // An unfinished deposit keeps both indices eligible for status refresh,
        // while the pairing batch only needs the first deposit.
        fixture.index_withdrawal_event(1);
        fixture
            .apply_deposit_update(0, make_deposit_info(DepositStatus::InProgress), None)
            .await;
        assert_eq!(
            fixture
                .state()
                .select_deposit_info_candidates(&[0, 1])
                .await,
            vec![0, 1]
        );

        // Without a tip, fetch only the pairing batch and preserve cached status.
        refresh_bridge_status(fixture.context()).await;
        assert_eq!(
            server
                .methods()
                .iter()
                .filter(|method| *method == "stratabridge_depositInfo")
                .count(),
            1
        );
        assert_eq!(
            fixture.snapshot().withdrawal_pairings,
            vec![WithdrawalPairing::new(0, 0)]
        );
        let status = fixture.bridge_status().await;
        assert_eq!(status.deposits.len(), 1);
        assert!(matches!(
            status.deposits[0].status,
            DepositStatus::InProgress
        ));
    }

    #[tokio::test]
    async fn withdrawal_failure_preserves_completion_and_allows_reimbursement() {
        let claim_txid = Txid::from_byte_array([4; 32]);
        let payout_txid = Txid::from_byte_array([5; 32]);
        let server = make_monitoring_server(
            vec![
                json!({"result": 1}),
                json!({"result": []}),
                make_rpc_error_reply(),
                json!({"result": RpcReimbursementStatus::Complete { claim_txid, payout_txid }}),
            ],
            Arc::new(AtomicBool::new(false)),
        )
        .await;
        let fixture = make_monitoring_fixture(&server);
        fixture.index_withdrawal_event(1);
        let withdrawal = fixture.seed_completed_withdrawal(1).await;
        let before = fixture.snapshot();

        // A failed fulfillment refresh must neither erase it nor prevent checking its payout.
        refresh_bridge_status(fixture.context()).await;
        let status = fixture.bridge_status().await;
        assert_eq!(status.withdrawals.len(), 1);
        assert_eq!(
            status.withdrawals[0].withdrawal_request_txid,
            withdrawal.withdrawal_request_txid
        );
        assert_eq!(
            status.withdrawals[0].fulfillment_txid,
            withdrawal.fulfillment_txid
        );
        assert!(matches!(
            status.withdrawals[0].status,
            WithdrawalStatus::Complete
        ));
        assert_eq!(
            fixture.snapshot().cursors.withdrawal_status,
            before.cursors.withdrawal_status
        );
        assert_eq!(status.reimbursements.len(), 1);
        assert_eq!(status.reimbursements[0].claim_txid, claim_txid);
        assert_eq!(status.reimbursements[0].payout_txid, Some(payout_txid));
        assert_eq!(
            status.reimbursements[0].status,
            ReimbursementStatus::Complete
        );
    }

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
            .collect::<Vec<_>>();
        let candidates = WithdrawalPairingCandidates {
            cursor: pairing_cursor,
            deposit_indices: candidates.into_iter().collect(),
        };
        pair_indexed_withdrawals(
            fixture.context(),
            deposit_indices,
            &candidates,
            &deposit_infos,
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

    #[tokio::test]
    async fn first_status_poll_serves_cached_status_despite_lookup_failures() {
        for (fail_tip, fail_indices) in [(false, false), (true, false), (false, true), (true, true)]
        {
            let case = format!("fail_tip={fail_tip}, fail_indices={fail_indices}");
            let indices_reply = if fail_indices {
                make_rpc_error_reply()
            } else {
                json!({"result": []})
            };
            let server = make_monitoring_server(
                vec![json!({"result": 1}), indices_reply],
                Arc::new(AtomicBool::new(fail_tip)),
            )
            .await;
            let fixture = make_monitoring_fixture(&server);
            let deposit = make_deposit_info(DepositStatus::Complete);
            fixture.apply_deposit_update(0, deposit, Some(1)).await;

            // Run the real loop: its first status poll must release HTTP waiters even when inputs fail.
            let manager = TaskManager::new(Handle::current());
            let context = Arc::clone(fixture.context());
            let (finished_tx, finished_rx) = oneshot::channel();
            manager
                .create_executor()
                .spawn_critical_async_with_shutdown(
                    "bridge-monitoring",
                    move |shutdown| async move {
                        let result = bridge_monitoring_task(context, shutdown).await;
                        let _ = finished_tx.send(());
                        result
                    },
                );
            let response = timeout(
                TEST_MONITORING_TIMEOUT,
                get_bridge_status(Arc::clone(fixture.context())),
            )
            .await;

            // Stop the loop before checking the result so a failed assertion leaves no running task.
            manager.get_shutdown_signal().send();
            timeout(TEST_MONITORING_TIMEOUT, finished_rx)
                .await
                .unwrap_or_else(|error| panic!("{case}: monitoring task shutdown: {error}"))
                .unwrap_or_else(|error| panic!("{case}: monitoring task finished: {error}"));
            let status = response
                .unwrap_or_else(|error| panic!("{case}: first poll released HTTP waiter: {error}"))
                .unwrap_or_else(|error| panic!("{case}: bridge status available: {error}"))
                .0;
            assert_eq!(status.operators.len(), 1, "{case}");
            assert_eq!(status.deposits.len(), 1, "{case}");
            assert_eq!(
                status.deposits[0].deposit_request_txid, deposit.deposit_request_txid,
                "{case}"
            );
            assert_eq!(
                status.deposits[0].deposit_txid, deposit.deposit_txid,
                "{case}"
            );
            assert!(
                matches!(status.deposits[0].status, DepositStatus::Complete),
                "{case}"
            );

            // Readiness remains available after the first poll; it does not require another update.
            assert!(
                timeout(
                    TEST_MONITORING_TIMEOUT,
                    get_bridge_status(Arc::clone(fixture.context()))
                )
                .await
                .unwrap_or_else(|error| panic!("{case}: latched readiness: {error}"))
                .is_ok(),
                "{case}"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn bridge_status_is_unavailable_before_first_poll() {
        // Paused time lets the readiness timeout elapse without waiting.
        let fixture = BridgeStatusFixture::default();

        let response = get_bridge_status(Arc::clone(fixture.context())).await;

        assert_eq!(response.err(), Some(StatusCode::SERVICE_UNAVAILABLE));
    }

    #[tokio::test]
    async fn bridge_status_is_served_after_first_poll() {
        let fixture = BridgeStatusFixture::default();
        let info = make_deposit_info(DepositStatus::Complete);
        fixture.apply_deposit_update(0, info, Some(1)).await;
        fixture.context().mark_initial_status_poll_completed();

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
            "request must wait for the first status poll to complete"
        );

        let info = make_deposit_info(DepositStatus::Complete);
        fixture.apply_deposit_update(0, info, Some(1)).await;
        fixture.context().mark_initial_status_poll_completed();

        let status = request.await.expect("bridge status").0;
        assert_eq!(status.deposits.len(), 1);
        assert_eq!(
            status.deposits[0].deposit_request_txid,
            info.deposit_request_txid
        );
    }
}
