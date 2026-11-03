use std::collections::{BTreeMap, BTreeSet};

use strata_bridge_primitives::types::DepositIdx;
use tokio::sync::RwLock;
use tracing::warn;

use super::{
    cache::BridgeStatusCache,
    db::{
        error::DbResult,
        traits::BridgeStatusDb,
        types::{BridgeStatusDbSnapshot, DbWithdrawalRequestRow},
    },
    types::{
        BridgeStatus, DepositInfo, DepositStatus, OperatorStatus, ReimbursementInfo,
        ReimbursementStatus, ReimbursementStatusCursor, WithdrawalInfo, WithdrawalPairing,
        WithdrawalPairingCursor, WithdrawalSeq, WithdrawalStatus, WithdrawalStatusCursor,
    },
};

/// Deposit status update collected during one monitoring tick.
///
/// The update carries the cache key separately because [`DepositInfo`] is the
/// dashboard row value and does not include its bridge deposit index.
#[derive(Debug)]
pub(crate) struct DepositInfoUpdate {
    pub(crate) deposit_idx: DepositIdx,
    pub(crate) info: DepositInfo,
    pub(crate) confirmations: Option<u64>,
}

/// Withdrawal status update collected during one monitoring tick.
#[derive(Debug)]
pub(crate) struct WithdrawalInfoUpdate {
    pub(crate) deposit_idx: DepositIdx,
    pub(crate) info: WithdrawalInfo,
    pub(crate) confirmations: Option<u64>,
}

/// Reimbursement status update collected during one monitoring tick.
#[derive(Debug)]
pub(crate) struct ReimbursementInfoUpdate {
    pub(crate) deposit_idx: DepositIdx,
    pub(crate) info: ReimbursementInfo,
    pub(crate) confirmations: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WithdrawalPairingUpdate {
    pairings: Vec<WithdrawalPairing>,
    cursor: WithdrawalPairingCursor,
}

/// Mutable bridge monitoring state shared by the polling task and HTTP handler.
#[derive(Debug, Default)]
pub(crate) struct BridgeMonitoringState {
    cache: RwLock<BridgeStatusCache>,
}

impl BridgeMonitoringState {
    pub(crate) fn from_snapshot(snapshot: BridgeStatusDbSnapshot) -> Self {
        Self {
            cache: RwLock::new(BridgeStatusCache::from_status_snapshot(snapshot)),
        }
    }

    pub(crate) async fn select_deposit_info_candidates(
        &self,
        deposit_indices: &[DepositIdx],
    ) -> Vec<DepositIdx> {
        let cache = self.cache.read().await;
        let deposit_info_cursor = cache.deposit_info_cursor();
        deposit_indices
            .iter()
            .copied()
            .filter(|deposit_idx| *deposit_idx >= deposit_info_cursor)
            .collect()
    }

    pub(crate) async fn apply_deposit_info_updates(
        &self,
        status_db: &(impl BridgeStatusDb + ?Sized),
        updates: Vec<DepositInfoUpdate>,
        max_confirmations: u64,
    ) -> DbResult<()> {
        let mut cache_updates = Vec::new();
        let mut terminal_deposit_indices_to_purge = Vec::new();

        for update in updates {
            match update.info.status {
                DepositStatus::InProgress => {
                    cache_updates.push((update.deposit_idx, update.info, None));
                }
                DepositStatus::Failed | DepositStatus::Complete => {
                    if has_reached_confirmation_cutoff(update.confirmations, max_confirmations) {
                        terminal_deposit_indices_to_purge.push(update.deposit_idx);
                    } else {
                        cache_updates.push((update.deposit_idx, update.info, update.confirmations));
                    }
                }
            }
        }

        let current_cursor = {
            let cache = self.cache.read().await;
            cache.deposit_info_cursor()
        };
        let next_cursor =
            next_deposit_info_cursor(current_cursor, &terminal_deposit_indices_to_purge);
        status_db.put_deposit_info_cursor(next_cursor)?;

        let mut cache = self.cache.write().await;
        cache.apply_deposit_updates(cache_updates);
        cache.purge_deposits(terminal_deposit_indices_to_purge);
        cache.set_deposit_info_cursor(next_cursor);
        Ok(())
    }

    pub(crate) async fn withdrawal_pairing_cursor(&self) -> WithdrawalPairingCursor {
        let cache = self.cache.read().await;
        cache.withdrawal_pairing_cursor()
    }

    pub(crate) async fn advance_withdrawal_pairings(
        &self,
        status_db: &(impl BridgeStatusDb + ?Sized),
        deposit_indices: &[DepositIdx],
        deposit_infos: &[(DepositIdx, DepositInfo)],
        withdrawal_requests: &[DbWithdrawalRequestRow],
    ) -> DbResult<Vec<WithdrawalPairing>> {
        let withdrawal_seqs = withdrawal_requests
            .iter()
            .map(|row| row.seq)
            .collect::<Vec<_>>();
        let current_cursor = {
            let cache = self.cache.read().await;
            cache.withdrawal_pairing_cursor()
        };
        let update = plan_withdrawal_pairings(
            current_cursor,
            deposit_indices,
            deposit_infos,
            &withdrawal_seqs,
        );
        if update.pairings.is_empty() && update.cursor == current_cursor {
            return Ok(update.pairings);
        }

        if !update.pairings.is_empty() {
            status_db.put_withdrawal_pairings(&update.pairings)?;
        }
        if update.cursor != current_cursor {
            status_db.put_withdrawal_pairing_cursor(update.cursor)?;
        }

        let mut cache = self.cache.write().await;
        cache.update_withdrawal_pairings(&update.pairings, update.cursor);
        Ok(update.pairings)
    }

    pub(crate) async fn select_withdrawal_status_candidates(&self) -> Vec<WithdrawalPairing> {
        let cache = self.cache.read().await;
        let cursor = cache.withdrawal_status_cursor().next_deposit_idx;
        cache.withdrawal_pairings_from(cursor)
    }

    pub(crate) async fn update_operators(&self, operators: Vec<OperatorStatus>) {
        let mut cache = self.cache.write().await;
        cache.update_operators(operators);
    }

    pub(crate) async fn apply_withdrawal_updates(
        &self,
        status_db: &(impl BridgeStatusDb + ?Sized),
        updates: Vec<WithdrawalInfoUpdate>,
        max_confirmations: u64,
    ) -> DbResult<()> {
        let mut cache_updates = Vec::new();
        let mut withdrawal_infos_to_persist = Vec::new();
        let mut terminal_deposit_indices = Vec::new();

        for update in updates {
            match update.info.status {
                WithdrawalStatus::InProgress => {
                    cache_updates.push((update.deposit_idx, update.info, None));
                }
                WithdrawalStatus::Complete => {
                    if has_reached_confirmation_cutoff(update.confirmations, max_confirmations) {
                        terminal_deposit_indices.push(update.deposit_idx);
                    }
                    withdrawal_infos_to_persist.push((update.deposit_idx, update.info));
                    cache_updates.push((update.deposit_idx, update.info, update.confirmations));
                }
            }
        }

        let (current_cursor, pairing_frontier, unconfirmed_fulfillment_indices) = {
            let cache = self.cache.read().await;
            let current_cursor = cache.withdrawal_status_cursor();
            let mut unconfirmed_fulfillment_indices = cache
                .withdrawal_pairings_from(current_cursor.next_deposit_idx)
                .into_iter()
                .map(|pairing| pairing.deposit_idx)
                .collect::<BTreeSet<_>>();
            for deposit_idx in &terminal_deposit_indices {
                unconfirmed_fulfillment_indices.remove(deposit_idx);
            }
            (
                current_cursor,
                cache.withdrawal_pairing_cursor().next_deposit_idx,
                unconfirmed_fulfillment_indices,
            )
        };
        let next_cursor = next_withdrawal_status_cursor(
            current_cursor,
            pairing_frontier,
            &unconfirmed_fulfillment_indices,
        );
        for (deposit_idx, info) in &withdrawal_infos_to_persist {
            status_db.put_withdrawal_info(*deposit_idx, info)?;
        }
        status_db.put_withdrawal_status_cursor(next_cursor)?;

        let pairing_purge_frontier = next_cursor.next_deposit_idx;
        let pairings_purged =
            match status_db.del_withdrawal_pairings_range(0, pairing_purge_frontier) {
                Ok(()) => true,
                Err(e) => {
                    warn!(error = %e, "failed to purge old withdrawal pairings");
                    false
                }
            };

        let mut cache = self.cache.write().await;
        cache.apply_withdrawal_updates(cache_updates);
        if pairings_purged {
            cache.purge_withdrawal_pairings_range(0, pairing_purge_frontier);
        }
        cache.set_withdrawal_status_cursor(next_cursor);
        Ok(())
    }

    pub(crate) async fn select_reimbursement_status_candidates(&self) -> Vec<DepositIdx> {
        let cache = self.cache.read().await;
        let reimbursement_cursor = cache.reimbursement_status_cursor().next_deposit_idx;
        cache
            .filter_withdrawals(|deposit_idx, info, _| {
                deposit_idx >= reimbursement_cursor
                    && matches!(info.status, WithdrawalStatus::Complete)
            })
            .into_iter()
            .map(|(deposit_idx, _)| deposit_idx)
            .collect()
    }

    pub(crate) async fn apply_reimbursement_updates(
        &self,
        status_db: &(impl BridgeStatusDb + ?Sized),
        updates: Vec<ReimbursementInfoUpdate>,
        max_confirmations: u64,
    ) -> DbResult<()> {
        let mut cache_updates = Vec::new();
        let mut terminal_deposit_indices_to_purge = Vec::new();

        for update in updates {
            match update.info.status {
                ReimbursementStatus::NotStarted => continue,
                ReimbursementStatus::InProgress => {
                    cache_updates.push((update.deposit_idx, update.info, None));
                }
                ReimbursementStatus::Slashed
                | ReimbursementStatus::Aborted
                | ReimbursementStatus::Complete => {
                    if has_reached_confirmation_cutoff(update.confirmations, max_confirmations) {
                        terminal_deposit_indices_to_purge.push(update.deposit_idx);
                    } else {
                        cache_updates.push((update.deposit_idx, update.info, update.confirmations));
                    }
                }
            }
        }

        let (next_cursor, withdrawal_deposit_indices_to_purge) = {
            let cache = self.cache.read().await;
            let current_cursor = cache.reimbursement_status_cursor();
            let withdrawal_frontier = cache.withdrawal_status_cursor().next_deposit_idx;
            let mut unconfirmed_reimbursement_indices = cache
                .filter_withdrawals(|deposit_idx, info, _| {
                    deposit_idx >= current_cursor.next_deposit_idx
                        && deposit_idx < withdrawal_frontier
                        && matches!(info.status, WithdrawalStatus::Complete)
                })
                .into_iter()
                .map(|(deposit_idx, _)| deposit_idx)
                .collect::<BTreeSet<_>>();
            for deposit_idx in &terminal_deposit_indices_to_purge {
                unconfirmed_reimbursement_indices.remove(deposit_idx);
            }
            let next_cursor = next_reimbursement_status_cursor(
                current_cursor,
                withdrawal_frontier,
                &unconfirmed_reimbursement_indices,
            );
            (
                next_cursor,
                cache
                    .filter_withdrawals(|deposit_idx, _, _| {
                        deposit_idx < next_cursor.next_deposit_idx
                    })
                    .into_iter()
                    .map(|(deposit_idx, _)| deposit_idx)
                    .collect::<Vec<_>>(),
            )
        };

        status_db.put_reimbursement_status_cursor(next_cursor)?;

        let mut purged_withdrawal_deposit_indices = Vec::new();
        for deposit_idx in withdrawal_deposit_indices_to_purge {
            match status_db.del_withdrawal_info(deposit_idx) {
                Ok(_) => purged_withdrawal_deposit_indices.push(deposit_idx),
                Err(e) => {
                    warn!(deposit_idx, error = %e, "failed to purge old withdrawal row");
                }
            }
        }

        let mut cache = self.cache.write().await;
        cache.apply_reimbursement_updates(cache_updates);
        cache.purge_reimbursements(terminal_deposit_indices_to_purge);
        cache.purge_withdrawals(purged_withdrawal_deposit_indices);
        cache.set_reimbursement_status_cursor(next_cursor);
        Ok(())
    }

    pub(crate) async fn bridge_status(&self, max_confirmations: u64) -> BridgeStatus {
        let cache = self.cache.read().await;

        BridgeStatus {
            operators: cache.get_operators(),
            deposits: cache
                // Omit terminal rows whose confirmations reached `max_confirmations` or are unknown.
                .filter_deposits(|_, info, confirmations| {
                    !matches!(info.status, DepositStatus::Complete | DepositStatus::Failed)
                        || is_below_confirmation_cutoff(confirmations, max_confirmations)
                })
                .into_iter()
                .map(|(_, info)| info)
                .collect(),
            withdrawals: cache
                // Complete withdrawals may be retained as reimbursement handoff state; omit
                // them from this response at `max_confirmations` or when confirmations are unknown.
                .filter_withdrawals(|_, info, confirmations| {
                    !matches!(info.status, WithdrawalStatus::Complete)
                        || is_below_confirmation_cutoff(confirmations, max_confirmations)
                })
                .into_iter()
                .map(|(_, info)| info)
                .collect(),
            reimbursements: cache
                // Omit terminal rows whose confirmations reached `max_confirmations` or are unknown.
                .filter_reimbursements(|_, info, confirmations| {
                    !matches!(
                        info.status,
                        ReimbursementStatus::Complete
                            | ReimbursementStatus::Slashed
                            | ReimbursementStatus::Aborted
                    ) || is_below_confirmation_cutoff(confirmations, max_confirmations)
                })
                .into_iter()
                .map(|(_, info)| info)
                .collect(),
        }
    }
}

/// Whether confirmations are known and below `max_confirmations`.
fn is_below_confirmation_cutoff(confirmations: Option<u64>, max_confirmations: u64) -> bool {
    confirmations.is_some_and(|confirmations| confirmations < max_confirmations)
}

/// Whether confirmations are known and at least `max_confirmations`.
fn has_reached_confirmation_cutoff(confirmations: Option<u64>, max_confirmations: u64) -> bool {
    confirmations.is_some_and(|confirmations| confirmations >= max_confirmations)
}

fn next_deposit_info_cursor(
    current_cursor: DepositIdx,
    terminal_deposit_indices_to_purge: &[DepositIdx],
) -> DepositIdx {
    let terminal_deposit_indices_to_purge = terminal_deposit_indices_to_purge
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let mut next_cursor = current_cursor;

    while terminal_deposit_indices_to_purge.contains(&next_cursor) {
        let Some(next) = next_cursor.checked_add(1) else {
            break;
        };
        next_cursor = next;
    }

    next_cursor
}

/// Returns the first paired index from `current_cursor` whose fulfillment has not reached
/// `max_tx_confirmations`, capped at `pairing_frontier` since later indices are not paired yet.
///
/// Snapshot restore relies on this cursor never passing an unconfirmed fulfillment.
fn next_withdrawal_status_cursor(
    current_cursor: WithdrawalStatusCursor,
    pairing_frontier: DepositIdx,
    unconfirmed_fulfillment_indices: &BTreeSet<DepositIdx>,
) -> WithdrawalStatusCursor {
    let start = current_cursor.next_deposit_idx;
    let end = pairing_frontier.max(start);
    WithdrawalStatusCursor {
        next_deposit_idx: unconfirmed_fulfillment_indices
            .range(start..end)
            .next()
            .copied()
            .unwrap_or(end),
    }
}

/// Returns the first index from `current_cursor` whose reimbursement has not reached
/// `max_tx_confirmations`, capped at `withdrawal_frontier` since later
/// fulfillments are not confirmed yet.
fn next_reimbursement_status_cursor(
    current_cursor: ReimbursementStatusCursor,
    withdrawal_frontier: DepositIdx,
    unconfirmed_reimbursement_indices: &BTreeSet<DepositIdx>,
) -> ReimbursementStatusCursor {
    let start = current_cursor.next_deposit_idx;
    let end = withdrawal_frontier.max(start);
    ReimbursementStatusCursor {
        next_deposit_idx: unconfirmed_reimbursement_indices
            .range(start..end)
            .next()
            .copied()
            .unwrap_or(end),
    }
}

fn plan_withdrawal_pairings(
    cursor: WithdrawalPairingCursor,
    discovered_deposit_indices: &[DepositIdx],
    deposit_infos: &[(DepositIdx, DepositInfo)],
    indexed_withdrawal_seqs: &[WithdrawalSeq],
) -> WithdrawalPairingUpdate {
    let discovered_deposit_indices = discovered_deposit_indices
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let deposit_infos = deposit_infos
        .iter()
        .map(|(deposit_idx, info)| (*deposit_idx, info.status))
        .collect::<BTreeMap<_, _>>();
    let indexed_withdrawal_seqs = indexed_withdrawal_seqs
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let mut next_cursor = cursor;
    let mut pairings = Vec::new();

    while discovered_deposit_indices.contains(&next_cursor.next_deposit_idx) {
        match deposit_infos.get(&next_cursor.next_deposit_idx) {
            Some(DepositStatus::Complete) => {
                if !indexed_withdrawal_seqs.contains(&next_cursor.next_withdrawal_seq) {
                    break;
                }
                pairings.push(WithdrawalPairing::new(
                    next_cursor.next_deposit_idx,
                    next_cursor.next_withdrawal_seq,
                ));

                let Some(next_deposit_idx) = next_cursor.next_deposit_idx.checked_add(1) else {
                    break;
                };
                let Some(next_withdrawal_seq) = next_cursor.next_withdrawal_seq.checked_add(1)
                else {
                    break;
                };
                next_cursor = WithdrawalPairingCursor {
                    next_deposit_idx,
                    next_withdrawal_seq,
                };
            }
            Some(DepositStatus::Failed) => {
                let Some(next_deposit_idx) = next_cursor.next_deposit_idx.checked_add(1) else {
                    break;
                };
                next_cursor = WithdrawalPairingCursor {
                    next_deposit_idx,
                    next_withdrawal_seq: next_cursor.next_withdrawal_seq,
                };
            }
            Some(DepositStatus::InProgress) | None => break,
        }
    }

    WithdrawalPairingUpdate {
        pairings,
        cursor: next_cursor,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use bitcoin::{hashes::Hash, Txid};

    use crate::{
        test_utils::{
            make_deposit_info, make_withdrawal_info, make_withdrawal_request_row,
            BridgeStatusFixture, TEST_MAX_CONFIRMATIONS,
        },
        types::ChallengeStep,
    };

    fn make_reimbursement_info(seed: u8, status: ReimbursementStatus) -> ReimbursementInfo {
        ReimbursementInfo {
            claim_txid: Txid::from_byte_array([seed; 32]),
            challenge_step: if status == ReimbursementStatus::InProgress {
                ChallengeStep::Claimed
            } else {
                ChallengeStep::NotApplicable
            },
            payout_txid: (status == ReimbursementStatus::Complete)
                .then_some(Txid::from_byte_array([seed.wrapping_add(1); 32])),
            status,
        }
    }

    fn make_deposit_infos(
        statuses: &[(DepositIdx, DepositStatus)],
    ) -> Vec<(DepositIdx, DepositInfo)> {
        statuses
            .iter()
            .map(|(deposit_idx, status)| (*deposit_idx, make_deposit_info(*status)))
            .collect()
    }

    fn make_pairing(deposit_idx: DepositIdx, withdrawal_seq: WithdrawalSeq) -> WithdrawalPairing {
        WithdrawalPairing::new(deposit_idx, withdrawal_seq)
    }

    #[test]
    fn deposit_info_cursor_advances_over_contiguous_purged_deposits() {
        // Only contiguous terminal deposits advance this cursor; a gap must block it.
        assert_eq!(next_deposit_info_cursor(0, &[0, 1, 3]), 2);
        assert_eq!(next_deposit_info_cursor(2, &[3, 4]), 2);
        assert_eq!(next_deposit_info_cursor(2, &[2, 3, 4]), 5);
    }

    #[test]
    fn withdrawal_status_cursor_stops_at_first_unconfirmed_or_pairing_frontier() {
        let next = |pairing_frontier, cursor, unconfirmed: &BTreeSet<DepositIdx>| {
            next_withdrawal_status_cursor(
                WithdrawalStatusCursor {
                    next_deposit_idx: cursor,
                },
                pairing_frontier,
                unconfirmed,
            )
            .next_deposit_idx
        };
        let unconfirmed = BTreeSet::from([1, 4, 8]);

        // A pending index at the cursor blocks advancement; gaps before it are skipped.
        assert_eq!(next(3, 0, &BTreeSet::from([0])), 0);
        assert_eq!(next(3, 0, &BTreeSet::from([2])), 2);
        assert_eq!(next(2, 0, &BTreeSet::new()), 2);
        assert_eq!(next(3, 0, &BTreeSet::new()), 3);

        // Ignore indices before the cursor and stop at the first unconfirmed one in range.
        assert_eq!(next(6, 2, &unconfirmed), 4);
        assert_eq!(next(3, 2, &unconfirmed), 3);

        // Never move backward, even if the pairing cursor is behind.
        assert_eq!(next(4, 4, &unconfirmed), 4);
        assert_eq!(next(4, 5, &unconfirmed), 5);

        // With nothing unconfirmed, advance directly to the pairing cursor.
        assert_eq!(next(DepositIdx::MAX, 0, &BTreeSet::new()), DepositIdx::MAX);
    }

    #[test]
    fn reimbursement_status_cursor_stops_at_first_unconfirmed_or_withdrawal_frontier() {
        let next = |withdrawal_frontier, cursor, unconfirmed: &BTreeSet<DepositIdx>| {
            next_reimbursement_status_cursor(
                ReimbursementStatusCursor {
                    next_deposit_idx: cursor,
                },
                withdrawal_frontier,
                unconfirmed,
            )
            .next_deposit_idx
        };
        let unconfirmed = BTreeSet::from([1, 4, 8]);

        // A pending index at the cursor blocks advancement; gaps before it are skipped.
        assert_eq!(next(3, 0, &BTreeSet::from([0])), 0);
        assert_eq!(next(3, 0, &BTreeSet::from([2])), 2);
        assert_eq!(next(2, 0, &BTreeSet::new()), 2);
        assert_eq!(next(3, 0, &BTreeSet::new()), 3);

        // Ignore indices before the cursor and stop at the first unconfirmed one in range.
        assert_eq!(next(6, 2, &unconfirmed), 4);
        assert_eq!(next(3, 2, &unconfirmed), 3);

        // Never move backward, even if the withdrawal-status cursor is behind.
        assert_eq!(next(4, 4, &unconfirmed), 4);
        assert_eq!(next(4, 5, &unconfirmed), 5);

        // With nothing unconfirmed, advance directly to the withdrawal-status cursor.
        assert_eq!(next(DepositIdx::MAX, 0, &BTreeSet::new()), DepositIdx::MAX);
    }

    #[test]
    fn withdrawal_pairing_planner_stops_at_deposit_gap() {
        // Deposit 2 is missing, so later complete deposits cannot consume a withdrawal request.
        let deposit_infos = make_deposit_infos(&[
            (0, DepositStatus::Complete),
            (1, DepositStatus::Complete),
            (3, DepositStatus::Complete),
        ]);
        let update = plan_withdrawal_pairings(
            WithdrawalPairingCursor::default(),
            &[0, 1, 3],
            &deposit_infos,
            &[0, 1, 2],
        );

        assert_eq!(
            update.pairings,
            vec![make_pairing(0, 0), make_pairing(1, 1)]
        );
        assert_eq!(
            update.cursor,
            WithdrawalPairingCursor {
                next_deposit_idx: 2,
                next_withdrawal_seq: 2
            }
        );
    }

    #[test]
    fn withdrawal_pairing_planner_stops_without_indexed_wrt() {
        // A complete deposit cannot be paired before its withdrawal request is indexed.
        let deposit_infos = make_deposit_infos(&[(0, DepositStatus::Complete)]);
        let update = plan_withdrawal_pairings(
            WithdrawalPairingCursor::default(),
            &[0],
            &deposit_infos,
            &[],
        );

        assert!(update.pairings.is_empty());
        assert_eq!(update.cursor, WithdrawalPairingCursor::default());
    }

    #[test]
    fn withdrawal_pairing_planner_skips_failed_deposits() {
        // A failed deposit advances the deposit cursor without consuming a withdrawal request.
        let deposit_infos = make_deposit_infos(&[
            (0, DepositStatus::Failed),
            (1, DepositStatus::Complete),
            (2, DepositStatus::Complete),
        ]);
        let update = plan_withdrawal_pairings(
            WithdrawalPairingCursor::default(),
            &[0, 1, 2],
            &deposit_infos,
            &[0, 1],
        );

        assert_eq!(
            update.pairings,
            vec![make_pairing(1, 0), make_pairing(2, 1)]
        );
        assert_eq!(
            update.cursor,
            WithdrawalPairingCursor {
                next_deposit_idx: 3,
                next_withdrawal_seq: 2
            }
        );
    }

    #[test]
    fn withdrawal_pairing_planner_stops_at_unresolved_deposit() {
        // An in-progress deposit blocks later pairings to preserve FIFO order.
        let deposit_infos =
            make_deposit_infos(&[(0, DepositStatus::InProgress), (1, DepositStatus::Complete)]);
        let update = plan_withdrawal_pairings(
            WithdrawalPairingCursor::default(),
            &[0, 1],
            &deposit_infos,
            &[0],
        );

        assert!(update.pairings.is_empty());
        assert_eq!(update.cursor, WithdrawalPairingCursor::default());
    }

    #[test]
    fn withdrawal_pairing_planner_does_not_repair_old_indices() {
        // Previously processed indices must not be paired again.
        let cursor = WithdrawalPairingCursor {
            next_deposit_idx: 2,
            next_withdrawal_seq: 2,
        };
        let deposit_infos =
            make_deposit_infos(&[(0, DepositStatus::Complete), (1, DepositStatus::Complete)]);
        let update = plan_withdrawal_pairings(cursor, &[0, 1], &deposit_infos, &[0, 1]);

        assert!(update.pairings.is_empty());
        assert_eq!(update.cursor, cursor);
    }

    #[tokio::test]
    async fn status_snapshot_restores_withdrawals_and_cursors() {
        // Seed persisted records directly to exercise restoration and retained storage.
        let mut fixture = BridgeStatusFixture::default();
        let withdrawal_info = make_withdrawal_info(2, WithdrawalStatus::Complete);

        // Deposit 2 passed withdrawal processing but still awaits reimbursement.
        fixture
            .status_db()
            .put_withdrawal_info(2, &withdrawal_info)
            .expect("put withdrawal info");
        fixture
            .status_db()
            .put_withdrawal_pairings(&[make_pairing(2, 7)])
            .expect("put withdrawal pairing");
        fixture
            .status_db()
            .put_deposit_info_cursor(2)
            .expect("put deposit cursor");
        fixture
            .status_db()
            .put_withdrawal_pairing_cursor(WithdrawalPairingCursor {
                next_deposit_idx: 3,
                next_withdrawal_seq: 8,
            })
            .expect("put pairing cursor");
        fixture
            .status_db()
            .put_withdrawal_status_cursor(WithdrawalStatusCursor {
                next_deposit_idx: 3,
            })
            .expect("put withdrawal cursor");
        fixture
            .status_db()
            .put_reimbursement_status_cursor(ReimbursementStatusCursor {
                next_deposit_idx: 2,
            })
            .expect("put reimbursement cursor");

        // Restore persisted cursors even when storage retains an old pairing.
        fixture.restart();

        assert_eq!(
            fixture.state().withdrawal_pairing_cursor().await,
            WithdrawalPairingCursor {
                next_deposit_idx: 3,
                next_withdrawal_seq: 8,
            }
        );
        assert_eq!(
            fixture
                .state()
                .select_deposit_info_candidates(&[0, 1, 2])
                .await,
            vec![2]
        );
        assert_eq!(
            fixture.state().select_withdrawal_status_candidates().await,
            Vec::<WithdrawalPairing>::new()
        );
        assert_eq!(
            fixture
                .state()
                .select_reimbursement_status_candidates()
                .await,
            vec![2]
        );

        // Retention for reimbursement must not expose the completed withdrawal.
        let status = fixture.bridge_status().await;
        assert!(status.deposits.is_empty());
        assert!(status.withdrawals.is_empty());
        assert!(status.reimbursements.is_empty());
    }

    #[tokio::test]
    async fn withdrawal_at_confirmation_cutoff_remains_excluded_after_restart() {
        let mut fixture = BridgeStatusFixture::default();

        // Reach the cutoff while retaining the withdrawal for reimbursement.
        let info = fixture
            .seed_completed_withdrawal(TEST_MAX_CONFIRMATIONS)
            .await;
        assert!(fixture.bridge_status().await.withdrawals.is_empty());

        // Cursor advancement and pairing removal stop further withdrawal polling.
        let snapshot = fixture.snapshot();
        assert_eq!(snapshot.cursors.withdrawal_status.next_deposit_idx, 1);
        assert!(snapshot.withdrawal_pairings.is_empty());
        assert_eq!(snapshot.withdrawals.len(), 1);
        assert_eq!(snapshot.withdrawals[0].0, 0);
        assert_eq!(
            snapshot.withdrawals[0].1.fulfillment_txid,
            info.fulfillment_txid
        );

        // Losing cached counts on restart must not include the retained row.
        fixture.restart();
        assert!(fixture
            .state()
            .select_withdrawal_status_candidates()
            .await
            .is_empty());
        assert!(fixture.bridge_status().await.withdrawals.is_empty());

        // With no candidates, the next monitoring cycle cannot refresh this row.
        fixture.apply_withdrawal_updates([]).await;
        assert!(fixture.bridge_status().await.withdrawals.is_empty());

        // Retain the completed withdrawal for reimbursement processing.
        assert_eq!(
            fixture
                .state()
                .select_reimbursement_status_candidates()
                .await,
            vec![0]
        );
        let retained = fixture.snapshot();
        assert_eq!(retained.withdrawals.len(), 1);
        assert_eq!(retained.withdrawals[0].0, 0);
        assert_eq!(
            retained.withdrawals[0].1.fulfillment_txid,
            info.fulfillment_txid
        );
    }

    #[tokio::test]
    async fn terminal_rows_with_unknown_confirmations_stay_polled_and_excluded() {
        let fixture = BridgeStatusFixture::default();
        let requests = vec![make_withdrawal_request_row(0, 1)];
        fixture
            .advance_withdrawal_pairings(
                &[0],
                &make_deposit_infos(&[(0, DepositStatus::Complete)]),
                &requests,
            )
            .await;

        // A newly completed withdrawal whose lookup fails has no count and is left out.
        let withdrawal_info = make_withdrawal_info(1, WithdrawalStatus::Complete);
        fixture
            .apply_withdrawal_update(0, withdrawal_info, None)
            .await;
        // The same holds for terminal deposits.
        fixture
            .apply_deposit_update(1, make_deposit_info(DepositStatus::Complete), None)
            .await;

        let status = fixture.bridge_status().await;
        assert!(status.withdrawals.is_empty());
        assert!(status.deposits.is_empty());
        // The completed row is saved and its pairing stays a polling candidate.
        let snapshot = fixture.snapshot();
        assert_eq!(
            snapshot.cursors.withdrawal_status,
            WithdrawalStatusCursor::default()
        );
        assert_eq!(snapshot.withdrawals.len(), 1);
        let (deposit_idx, persisted) = &snapshot.withdrawals[0];
        assert_eq!(*deposit_idx, 0);
        assert_eq!(
            persisted.withdrawal_request_txid,
            withdrawal_info.withdrawal_request_txid
        );
        assert_eq!(persisted.fulfillment_txid, withdrawal_info.fulfillment_txid);
        assert!(matches!(persisted.status, WithdrawalStatus::Complete));
        assert_eq!(snapshot.withdrawal_pairings, vec![make_pairing(0, 0)]);
        assert_eq!(
            fixture.state().select_withdrawal_status_candidates().await,
            vec![make_pairing(0, 0)]
        );
    }

    #[tokio::test]
    async fn restored_withdrawal_remains_excluded_until_confirmations_are_known() {
        let mut fixture = BridgeStatusFixture::default();

        // Below the cutoff, the row is included and its pairing remains active.
        let info = fixture
            .seed_completed_withdrawal(TEST_MAX_CONFIRMATIONS - 1)
            .await;
        assert_eq!(fixture.bridge_status().await.withdrawals.len(), 1);

        // After restart, the row has no count and is left out until its confirmations are refreshed.
        fixture.restart();
        assert_eq!(
            fixture.state().select_withdrawal_status_candidates().await,
            vec![make_pairing(0, 0)]
        );
        assert!(fixture.bridge_status().await.withdrawals.is_empty());

        // Only a known count below the cutoff includes the row; a failed lookup or a count at
        // the cutoff leaves it out.
        for (confirmations, included) in [
            (None, false),
            (Some(TEST_MAX_CONFIRMATIONS - 1), true),
            (Some(TEST_MAX_CONFIRMATIONS), false),
        ] {
            fixture
                .apply_withdrawal_update(0, info, confirmations)
                .await;
            let withdrawals = fixture.bridge_status().await.withdrawals;
            assert_eq!(withdrawals.len(), usize::from(included));
            if included {
                assert_eq!(withdrawals[0].fulfillment_txid, info.fulfillment_txid);
                assert_eq!(
                    withdrawals[0].withdrawal_request_txid,
                    info.withdrawal_request_txid
                );
            }
        }

        // Reaching the cutoff affects inclusion, not reimbursement eligibility.
        assert_eq!(
            fixture
                .state()
                .select_reimbursement_status_candidates()
                .await,
            vec![0]
        );
    }

    #[tokio::test]
    async fn withdrawal_ahead_of_blocked_cursor_never_reuses_previous_confirmations() {
        let mut fixture = BridgeStatusFixture::default();
        let requests = [
            make_withdrawal_request_row(0, 1),
            make_withdrawal_request_row(1, 2),
        ];
        let first = make_withdrawal_info(1, WithdrawalStatus::InProgress);
        let second = make_withdrawal_info(2, WithdrawalStatus::Complete);

        // The in-progress first withdrawal holds the cursor before one at the cutoff.
        fixture
            .advance_withdrawal_pairings(
                &[0, 1],
                &make_deposit_infos(&[(0, DepositStatus::Complete), (1, DepositStatus::Complete)]),
                &requests,
            )
            .await;
        fixture
            .apply_withdrawal_updates([
                WithdrawalInfoUpdate {
                    deposit_idx: 0,
                    info: first,
                    confirmations: None,
                },
                WithdrawalInfoUpdate {
                    deposit_idx: 1,
                    info: second,
                    confirmations: Some(TEST_MAX_CONFIRMATIONS),
                },
            ])
            .await;
        let included = fixture.bridge_status().await.withdrawals;
        assert_eq!(included.len(), 1);
        assert_eq!(
            included[0].withdrawal_request_txid,
            first.withdrawal_request_txid
        );
        assert_eq!(
            fixture
                .snapshot()
                .cursors
                .withdrawal_status
                .next_deposit_idx,
            0
        );

        // Above the saved cursor, even a row past the cutoff restores without a count.
        fixture.restart();
        assert!(fixture.bridge_status().await.withdrawals.is_empty());
        assert_eq!(
            fixture.state().select_withdrawal_status_candidates().await,
            vec![make_pairing(0, 0), make_pairing(1, 1)]
        );

        // Only a known count below the cutoff includes the row; a failed lookup never
        // reuses an earlier count.
        for (confirmations, included) in [
            (None, false),
            (Some(TEST_MAX_CONFIRMATIONS), false),
            (None, false),
            (Some(TEST_MAX_CONFIRMATIONS - 1), true),
            (None, false),
        ] {
            fixture
                .apply_withdrawal_update(1, second, confirmations)
                .await;
            let withdrawals = fixture.bridge_status().await.withdrawals;
            assert_eq!(withdrawals.len(), usize::from(included));
            if included {
                assert_eq!(withdrawals[0].fulfillment_txid, second.fulfillment_txid);
            }
        }
        assert_eq!(
            fixture
                .snapshot()
                .cursors
                .withdrawal_status
                .next_deposit_idx,
            0
        );
        assert_eq!(
            fixture
                .state()
                .select_reimbursement_status_candidates()
                .await,
            vec![1]
        );
    }

    #[tokio::test]
    async fn deposit_updates_persist_cursor_only() {
        // Drive deposit transitions; rows stay in memory while only the cursor is persisted.
        let fixture = BridgeStatusFixture::default();
        let in_progress = make_deposit_info(DepositStatus::InProgress);
        let complete = make_deposit_info(DepositStatus::Complete);

        fixture.apply_deposit_update(0, in_progress, None).await;

        let snapshot = fixture.snapshot();
        assert_eq!(snapshot.cursors.deposit_info, 0);
        assert!(snapshot.withdrawals.is_empty());
        let deposits = fixture.bridge_status().await.deposits;
        assert_eq!(deposits.len(), 1);
        assert!(matches!(deposits[0].status, DepositStatus::InProgress));

        // Terminal deposits remain included below the cutoff and disappear at it.
        for (confirmations, included) in [
            (TEST_MAX_CONFIRMATIONS - 1, true),
            (TEST_MAX_CONFIRMATIONS, false),
        ] {
            fixture
                .apply_deposit_update(0, complete, Some(confirmations))
                .await;
            let deposits = fixture.bridge_status().await.deposits;
            assert_eq!(deposits.len(), usize::from(included));
            if included {
                assert_eq!(
                    deposits[0].deposit_request_txid,
                    complete.deposit_request_txid
                );
            }
        }

        let snapshot = fixture.snapshot();
        assert_eq!(snapshot.cursors.deposit_info, 1);
        assert!(snapshot.withdrawals.is_empty());
        assert_eq!(
            fixture
                .state()
                .select_deposit_info_candidates(&[0, 1])
                .await,
            vec![1]
        );
    }

    #[tokio::test]
    async fn withdrawal_pairings_persist_rows_and_cursor() {
        // Pair two complete deposits through the production transition.
        let fixture = BridgeStatusFixture::default();
        let requests = vec![
            make_withdrawal_request_row(0, 1),
            make_withdrawal_request_row(1, 2),
        ];

        let complete_deposit_infos =
            make_deposit_infos(&[(0, DepositStatus::Complete), (1, DepositStatus::Complete)]);
        let pairings = fixture
            .advance_withdrawal_pairings(&[0, 1], &complete_deposit_infos, &requests)
            .await;

        // The returned pairings and persisted cursor must describe the same FIFO progress.
        assert_eq!(pairings, vec![make_pairing(0, 0), make_pairing(1, 1)]);
        assert_eq!(
            fixture.state().withdrawal_pairing_cursor().await,
            WithdrawalPairingCursor {
                next_deposit_idx: 2,
                next_withdrawal_seq: 2
            }
        );
        let snapshot = fixture.snapshot();
        assert_eq!(
            snapshot.withdrawal_pairings,
            vec![make_pairing(0, 0), make_pairing(1, 1)]
        );
        assert_eq!(
            snapshot.cursors.withdrawal_pairing,
            WithdrawalPairingCursor {
                next_deposit_idx: 2,
                next_withdrawal_seq: 2
            }
        );
    }

    #[tokio::test]
    async fn withdrawal_pairings_persist_cursor_when_skipping_failed_deposits() {
        // Skip a failed deposit without consuming or persisting a withdrawal pairing.
        let fixture = BridgeStatusFixture::default();
        let failed_deposit_infos = make_deposit_infos(&[(0, DepositStatus::Failed)]);

        let pairings = fixture
            .advance_withdrawal_pairings(&[0], &failed_deposit_infos, &[])
            .await;

        // Persist deposit progress even when no pairing row is produced.
        assert!(pairings.is_empty());
        assert_eq!(
            fixture.state().withdrawal_pairing_cursor().await,
            WithdrawalPairingCursor {
                next_deposit_idx: 1,
                next_withdrawal_seq: 0
            }
        );
        let snapshot = fixture.snapshot();
        assert!(snapshot.withdrawal_pairings.is_empty());
        assert_eq!(
            snapshot.cursors.withdrawal_pairing,
            WithdrawalPairingCursor {
                next_deposit_idx: 1,
                next_withdrawal_seq: 0
            }
        );
    }

    #[tokio::test]
    async fn withdrawal_status_candidates_exclude_indices_below_cursor() {
        // Only pairings at or above the status cursor remain polling candidates.
        let fixture = BridgeStatusFixture::default();
        let requests = vec![
            make_withdrawal_request_row(0, 1),
            make_withdrawal_request_row(1, 2),
        ];

        let complete_deposit_infos =
            make_deposit_infos(&[(0, DepositStatus::Complete), (1, DepositStatus::Complete)]);
        fixture
            .advance_withdrawal_pairings(&[0, 1], &complete_deposit_infos, &requests)
            .await;
        assert_eq!(
            fixture.state().select_withdrawal_status_candidates().await,
            vec![make_pairing(0, 0), make_pairing(1, 1)]
        );

        // Reaching the cutoff removes only the first pairing from future polling.
        fixture
            .apply_withdrawal_update(
                0,
                make_withdrawal_info(1, WithdrawalStatus::Complete),
                Some(TEST_MAX_CONFIRMATIONS),
            )
            .await;

        assert_eq!(
            fixture.state().select_withdrawal_status_candidates().await,
            vec![make_pairing(1, 1)]
        );
        let snapshot = fixture.snapshot();
        assert_eq!(
            snapshot.withdrawal_pairings,
            vec![make_pairing(1, 1)],
            "pairings at the cutoff below the cursor are removed"
        );
        assert_eq!(
            snapshot.cursors.withdrawal_status,
            WithdrawalStatusCursor {
                next_deposit_idx: 1
            }
        );
        assert_eq!(snapshot.withdrawals.len(), 1);
        assert_eq!(snapshot.withdrawals[0].0, 0);
    }

    #[tokio::test]
    async fn withdrawal_pairing_cleanup_removes_rows_below_status_cursor() {
        // Seed persisted records directly to exercise restoration and retained storage.
        let mut fixture = BridgeStatusFixture::default();
        fixture
            .status_db()
            .put_withdrawal_pairings(&[make_pairing(0, 0), make_pairing(1, 1)])
            .expect("put withdrawal pairings");
        fixture
            .status_db()
            .put_withdrawal_status_cursor(WithdrawalStatusCursor {
                next_deposit_idx: 1,
            })
            .expect("put withdrawal status cursor");

        fixture.restart();

        // A monitoring cycle removes pairings left below the saved status cursor.
        fixture.apply_withdrawal_updates([]).await;

        // Cleanup preserves the pairing that is still an active polling candidate.
        let snapshot = fixture.snapshot();
        assert_eq!(snapshot.withdrawal_pairings, vec![make_pairing(1, 1)]);
        assert_eq!(
            fixture.state().select_withdrawal_status_candidates().await,
            vec![make_pairing(1, 1)]
        );
    }

    #[tokio::test]
    async fn withdrawal_pairing_cleanup_skips_gaps_below_pairing_cursor() {
        // Seed persisted records directly to exercise restoration and retained storage.
        let mut fixture = BridgeStatusFixture::default();
        fixture
            .status_db()
            .put_withdrawal_pairings(&[make_pairing(0, 0), make_pairing(2, 1)])
            .expect("put withdrawal pairings");
        fixture
            .status_db()
            .put_withdrawal_pairing_cursor(WithdrawalPairingCursor {
                next_deposit_idx: 3,
                next_withdrawal_seq: 2,
            })
            .expect("put withdrawal pairing cursor");

        fixture.restart();

        // Complete the first pairing; the absent deposit 1 must not block the cursor at 2.
        fixture
            .apply_withdrawal_update(
                0,
                make_withdrawal_info(20, WithdrawalStatus::Complete),
                Some(TEST_MAX_CONFIRMATIONS),
            )
            .await;

        let snapshot = fixture.snapshot();
        assert_eq!(
            snapshot.cursors.withdrawal_status,
            WithdrawalStatusCursor {
                next_deposit_idx: 2
            }
        );
        assert_eq!(snapshot.withdrawal_pairings, vec![make_pairing(2, 1)]);

        // Completing the last pairing advances to the pairing cursor and removes all pairings.
        fixture
            .apply_withdrawal_update(
                2,
                make_withdrawal_info(22, WithdrawalStatus::Complete),
                Some(TEST_MAX_CONFIRMATIONS),
            )
            .await;

        let snapshot = fixture.snapshot();
        assert_eq!(
            snapshot.cursors.withdrawal_status,
            WithdrawalStatusCursor {
                next_deposit_idx: 3
            }
        );
        assert!(snapshot.withdrawal_pairings.is_empty());
    }

    #[tokio::test]
    async fn withdrawal_cache_allows_shared_request_tx_hash() {
        // Two deposits can share one request transaction; cache identity is the deposit index.
        let fixture = BridgeStatusFixture::default();
        let info = make_withdrawal_info(1, WithdrawalStatus::InProgress);
        let withdrawal_request_txid = info.withdrawal_request_txid;
        fixture.apply_withdrawal_update(0, info, None).await;
        fixture.apply_withdrawal_update(1, info, None).await;

        let status = fixture.bridge_status().await;

        assert_eq!(status.withdrawals.len(), 2);
        assert!(status
            .withdrawals
            .iter()
            .all(|info| info.withdrawal_request_txid == withdrawal_request_txid));
        assert!(
            fixture.snapshot().withdrawals.is_empty(),
            "in-progress withdrawal rows are cache-only"
        );
    }

    #[tokio::test]
    async fn reimbursement_candidates_include_complete_withdrawals_before_and_after_cutoff() {
        let fixture = BridgeStatusFixture::default();

        // Neither an empty state nor a pairing without a completed withdrawal is eligible.
        assert!(fixture
            .state()
            .select_reimbursement_status_candidates()
            .await
            .is_empty());
        fixture
            .advance_withdrawal_pairings(
                &[0],
                &make_deposit_infos(&[(0, DepositStatus::Complete)]),
                &[make_withdrawal_request_row(0, 1)],
            )
            .await;
        assert!(fixture
            .state()
            .select_reimbursement_status_candidates()
            .await
            .is_empty());

        // Completion enables reimbursement polling even before fulfillment reaches the cutoff.
        let info = make_withdrawal_info(1, WithdrawalStatus::Complete);
        fixture.apply_withdrawal_update(0, info, Some(1)).await;
        assert_eq!(
            fixture.snapshot().cursors.withdrawal_status,
            WithdrawalStatusCursor::default()
        );
        assert_eq!(
            fixture
                .state()
                .select_reimbursement_status_candidates()
                .await,
            vec![0]
        );
        assert_eq!(fixture.bridge_status().await.withdrawals.len(), 1);

        // Reaching the cutoff leaves the withdrawal out but preserves reimbursement eligibility.
        fixture
            .apply_withdrawal_update(0, info, Some(TEST_MAX_CONFIRMATIONS))
            .await;
        assert_eq!(
            fixture
                .state()
                .select_reimbursement_status_candidates()
                .await,
            vec![0]
        );
        assert!(fixture.bridge_status().await.withdrawals.is_empty());
    }

    #[tokio::test]
    async fn reimbursement_in_progress_retains_completed_withdrawal() {
        let fixture = BridgeStatusFixture::default();
        let withdrawal = fixture
            .seed_completed_withdrawal(TEST_MAX_CONFIRMATIONS)
            .await;

        // An active reimbursement remains included without a terminal confirmation count.
        let reimbursement = make_reimbursement_info(1, ReimbursementStatus::InProgress);
        fixture
            .apply_reimbursement_update(0, reimbursement, None)
            .await;
        let reimbursements = fixture.bridge_status().await.reimbursements;
        assert_eq!(reimbursements, vec![reimbursement]);

        // Keep the completed withdrawal and cursor until its reimbursement reaches the cutoff.
        let snapshot = fixture.snapshot();
        assert_eq!(
            snapshot.cursors.reimbursement_status,
            ReimbursementStatusCursor::default()
        );
        assert_eq!(snapshot.withdrawals.len(), 1);
        assert_eq!(snapshot.withdrawals[0].0, 0);
        assert_eq!(
            snapshot.withdrawals[0].1.fulfillment_txid,
            withdrawal.fulfillment_txid
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
    async fn reimbursement_cleanup_removes_completed_withdrawal_at_cutoff() {
        let fixture = BridgeStatusFixture::default();
        fixture
            .seed_completed_withdrawal(TEST_MAX_CONFIRMATIONS)
            .await;
        let reimbursement = make_reimbursement_info(1, ReimbursementStatus::Complete);

        // Confirmed reimbursements remain included and retain the withdrawal below the cutoff.
        fixture
            .apply_reimbursement_update(0, reimbursement, Some(TEST_MAX_CONFIRMATIONS - 1))
            .await;
        assert_eq!(
            fixture.bridge_status().await.reimbursements,
            vec![reimbursement]
        );
        assert_eq!(
            fixture
                .state()
                .select_reimbursement_status_candidates()
                .await,
            vec![0]
        );
        let snapshot = fixture.snapshot();
        assert_eq!(
            snapshot.cursors.reimbursement_status,
            ReimbursementStatusCursor::default()
        );
        assert_eq!(snapshot.withdrawals.len(), 1);

        // Reaching the cutoff removes both rows and advances past the completed handoff.
        fixture
            .apply_reimbursement_update(0, reimbursement, Some(TEST_MAX_CONFIRMATIONS))
            .await;
        let status = fixture.bridge_status().await;
        assert!(status.reimbursements.is_empty());
        assert!(status.withdrawals.is_empty());
        assert!(fixture
            .state()
            .select_reimbursement_status_candidates()
            .await
            .is_empty());
        let snapshot = fixture.snapshot();
        assert_eq!(
            snapshot.cursors.reimbursement_status,
            ReimbursementStatusCursor {
                next_deposit_idx: 1
            }
        );
        assert!(snapshot.withdrawals.is_empty());
    }

    #[tokio::test]
    async fn reimbursement_cleanup_skips_gaps_below_withdrawal_status_cursor() {
        // Seed persisted records directly to exercise restoration and retained storage.
        let mut fixture = BridgeStatusFixture::default();
        fixture
            .status_db()
            .put_withdrawal_info(0, &make_withdrawal_info(30, WithdrawalStatus::Complete))
            .expect("put withdrawal info");
        fixture
            .status_db()
            .put_withdrawal_info(2, &make_withdrawal_info(32, WithdrawalStatus::Complete))
            .expect("put withdrawal info");
        fixture
            .status_db()
            .put_withdrawal_status_cursor(WithdrawalStatusCursor {
                next_deposit_idx: 3,
            })
            .expect("put withdrawal cursor");

        fixture.restart();

        // Complete reimbursement 0; the missing withdrawal 1 must not block the cursor at 2.
        fixture
            .apply_reimbursement_update(
                0,
                make_reimbursement_info(34, ReimbursementStatus::Complete),
                Some(TEST_MAX_CONFIRMATIONS),
            )
            .await;

        let snapshot = fixture.snapshot();
        assert_eq!(
            snapshot.cursors.reimbursement_status,
            ReimbursementStatusCursor {
                next_deposit_idx: 2
            }
        );
        assert_eq!(snapshot.withdrawals.len(), 1);
        assert_eq!(snapshot.withdrawals[0].0, 2);

        // Completing the remaining reimbursement removes the last retained withdrawal.
        fixture
            .apply_reimbursement_update(
                2,
                make_reimbursement_info(36, ReimbursementStatus::Complete),
                Some(TEST_MAX_CONFIRMATIONS),
            )
            .await;

        let snapshot = fixture.snapshot();
        assert_eq!(
            snapshot.cursors.reimbursement_status,
            ReimbursementStatusCursor {
                next_deposit_idx: 3
            }
        );
        assert!(snapshot.withdrawals.is_empty());
    }

    #[tokio::test]
    async fn reimbursement_candidates_skip_incomplete_withdrawals() {
        // Drive mixed withdrawal states; only a Complete row can start reimbursement polling.
        let fixture = BridgeStatusFixture::default();

        fixture
            .apply_withdrawal_updates([
                WithdrawalInfoUpdate {
                    deposit_idx: 0,
                    info: make_withdrawal_info(10, WithdrawalStatus::InProgress),
                    confirmations: None,
                },
                WithdrawalInfoUpdate {
                    deposit_idx: 1,
                    info: make_withdrawal_info(11, WithdrawalStatus::Complete),
                    confirmations: Some(1),
                },
            ])
            .await;

        // The in-progress withdrawal is excluded even though it is present in the cache.
        assert_eq!(
            fixture
                .state()
                .select_reimbursement_status_candidates()
                .await,
            vec![1]
        );
    }

    #[tokio::test]
    async fn reimbursement_candidates_use_retained_complete_rows_when_cursor_lags() {
        // Seed persisted records directly to exercise restoration and retained storage.
        let mut fixture = BridgeStatusFixture::default();
        fixture
            .status_db()
            .put_withdrawal_info(2, &make_withdrawal_info(13, WithdrawalStatus::Complete))
            .expect("put withdrawal info");
        fixture
            .status_db()
            .put_withdrawal_status_cursor(WithdrawalStatusCursor {
                next_deposit_idx: 3,
            })
            .expect("put withdrawal status cursor");
        fixture
            .status_db()
            .put_reimbursement_status_cursor(ReimbursementStatusCursor {
                next_deposit_idx: 1,
            })
            .expect("put reimbursement status cursor");

        fixture.restart();

        // A gap at index 1 does not exclude the retained complete withdrawal at index 2.
        assert_eq!(
            fixture
                .state()
                .select_reimbursement_status_candidates()
                .await,
            vec![2]
        );
    }
}
