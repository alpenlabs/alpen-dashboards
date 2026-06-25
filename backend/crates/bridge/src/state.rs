use std::collections::{BTreeMap, BTreeSet};

use strata_bridge_primitives::types::DepositIdx;
use tokio::sync::RwLock;
use tracing::warn;

use super::{
    cache::BridgeStatusCache,
    db::{
        error::DbResult,
        traits::BridgeStatusDb,
        types::{DbBridgeStatusSnapshot, DbWithdrawalRequestRow},
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
    pub(crate) fn from_snapshot(snapshot: DbBridgeStatusSnapshot) -> Self {
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
        status_db: &impl BridgeStatusDb,
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
                    let Some(confirmations) = update.confirmations else {
                        continue;
                    };

                    if confirmations >= max_confirmations {
                        terminal_deposit_indices_to_purge.push(update.deposit_idx);
                    } else {
                        cache_updates.push((update.deposit_idx, update.info, Some(confirmations)));
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
        status_db: &impl BridgeStatusDb,
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
        status_db: &impl BridgeStatusDb,
        updates: Vec<WithdrawalInfoUpdate>,
        max_confirmations: u64,
    ) -> DbResult<()> {
        let mut cache_updates = Vec::new();
        let mut withdrawal_infos_to_persist = Vec::new();
        let mut terminal_deposit_indices_to_purge = Vec::new();

        for update in updates {
            match update.info.status {
                WithdrawalStatus::InProgress => {
                    cache_updates.push((update.deposit_idx, update.info, None));
                }
                WithdrawalStatus::Complete => {
                    let Some(confirmations) = update.confirmations else {
                        continue;
                    };

                    if confirmations >= max_confirmations {
                        terminal_deposit_indices_to_purge.push(update.deposit_idx);
                    }
                    withdrawal_infos_to_persist.push((update.deposit_idx, update.info));
                    cache_updates.push((update.deposit_idx, update.info, Some(confirmations)));
                }
            }
        }

        let (current_cursor, pairing_frontier, paired_deposit_indices) = {
            let cache = self.cache.read().await;
            let current_cursor = cache.withdrawal_status_cursor();
            (
                current_cursor,
                cache.withdrawal_pairing_cursor().next_deposit_idx,
                cache
                    .withdrawal_pairings_from(current_cursor.next_deposit_idx)
                    .into_iter()
                    .map(|pairing| pairing.deposit_idx)
                    .collect::<Vec<_>>(),
            )
        };
        let next_cursor = next_withdrawal_status_cursor(
            current_cursor,
            &terminal_deposit_indices_to_purge,
            pairing_frontier,
            &paired_deposit_indices,
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
        status_db: &impl BridgeStatusDb,
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
                    let Some(confirmations) = update.confirmations else {
                        continue;
                    };

                    if confirmations >= max_confirmations {
                        terminal_deposit_indices_to_purge.push(update.deposit_idx);
                    } else {
                        cache_updates.push((update.deposit_idx, update.info, Some(confirmations)));
                    }
                }
            }
        }

        let (next_cursor, withdrawal_deposit_indices_to_purge) = {
            let cache = self.cache.read().await;
            let current_cursor = cache.reimbursement_status_cursor();
            let withdrawal_frontier = cache.withdrawal_status_cursor().next_deposit_idx;
            let complete_withdrawal_deposit_indices = cache
                .filter_withdrawals(|deposit_idx, info, _| {
                    deposit_idx >= current_cursor.next_deposit_idx
                        && deposit_idx < withdrawal_frontier
                        && matches!(info.status, WithdrawalStatus::Complete)
                })
                .into_iter()
                .map(|(deposit_idx, _)| deposit_idx)
                .collect::<Vec<_>>();
            let next_cursor = next_reimbursement_status_cursor(
                current_cursor,
                &terminal_deposit_indices_to_purge,
                withdrawal_frontier,
                &complete_withdrawal_deposit_indices,
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
                // Omit terminal rows whose confirmations reached `max_confirmations`.
                .filter_deposits(|_, info, confirmations| {
                    !matches!(info.status, DepositStatus::Complete | DepositStatus::Failed)
                        || confirmations
                            .is_none_or(|confirmations| confirmations < max_confirmations)
                })
                .into_iter()
                .map(|(_, info)| info)
                .collect(),
            withdrawals: cache
                // Complete withdrawals may be retained as reimbursement
                // handoff state; omit them from this response at `max_confirmations`.
                .filter_withdrawals(|_, info, confirmations| {
                    !matches!(info.status, WithdrawalStatus::Complete)
                        || confirmations
                            .is_none_or(|confirmations| confirmations < max_confirmations)
                })
                .into_iter()
                .map(|(_, info)| info)
                .collect(),
            reimbursements: cache
                // Omit terminal rows whose confirmations reached `max_confirmations`.
                .filter_reimbursements(|_, info, confirmations| {
                    !matches!(
                        info.status,
                        ReimbursementStatus::Complete
                            | ReimbursementStatus::Slashed
                            | ReimbursementStatus::Aborted
                    ) || confirmations.is_none_or(|confirmations| confirmations < max_confirmations)
                })
                .into_iter()
                .map(|(_, info)| info)
                .collect(),
        }
    }
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

fn next_withdrawal_status_cursor(
    current_cursor: WithdrawalStatusCursor,
    terminal_deposit_indices_to_purge: &[DepositIdx],
    pairing_frontier: DepositIdx,
    paired_deposit_indices: &[DepositIdx],
) -> WithdrawalStatusCursor {
    WithdrawalStatusCursor {
        next_deposit_idx: next_sparse_status_cursor(
            current_cursor.next_deposit_idx,
            terminal_deposit_indices_to_purge,
            pairing_frontier,
            paired_deposit_indices,
        ),
    }
}

fn next_reimbursement_status_cursor(
    current_cursor: ReimbursementStatusCursor,
    terminal_deposit_indices_to_purge: &[DepositIdx],
    withdrawal_frontier: DepositIdx,
    complete_withdrawal_deposit_indices: &[DepositIdx],
) -> ReimbursementStatusCursor {
    ReimbursementStatusCursor {
        next_deposit_idx: next_sparse_status_cursor(
            current_cursor.next_deposit_idx,
            terminal_deposit_indices_to_purge,
            withdrawal_frontier,
            complete_withdrawal_deposit_indices,
        ),
    }
}

fn next_sparse_status_cursor(
    current_cursor: DepositIdx,
    terminal_deposit_indices_to_purge: &[DepositIdx],
    domain_frontier: DepositIdx,
    pending_domain_indices: &[DepositIdx],
) -> DepositIdx {
    let terminal_deposit_indices_to_purge = terminal_deposit_indices_to_purge
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let pending_domain_indices = pending_domain_indices
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let mut next_cursor = current_cursor;

    while next_cursor < domain_frontier
        && (terminal_deposit_indices_to_purge.contains(&next_cursor)
            || !pending_domain_indices.contains(&next_cursor))
    {
        let Some(next) = next_cursor.checked_add(1) else {
            break;
        };
        next_cursor = next;
    }

    next_cursor
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
    use strata_primitives::buf::Buf32;

    use crate::db::{traits::BridgeStatusDb, BridgeStatusDbSled};

    fn deposit_info(status: DepositStatus) -> DepositInfo {
        DepositInfo {
            deposit_request_txid: Txid::from_byte_array([1; 32]),
            deposit_txid: matches!(status, DepositStatus::Complete)
                .then_some(Txid::from_byte_array([2; 32])),
            status,
        }
    }

    fn deposit_infos(statuses: &[(DepositIdx, DepositStatus)]) -> Vec<(DepositIdx, DepositInfo)> {
        statuses
            .iter()
            .map(|(deposit_idx, status)| (*deposit_idx, deposit_info(*status)))
            .collect()
    }

    fn pairing(deposit_idx: DepositIdx, withdrawal_seq: WithdrawalSeq) -> WithdrawalPairing {
        WithdrawalPairing::new(deposit_idx, withdrawal_seq)
    }

    #[test]
    fn deposit_info_cursor_advances_over_contiguous_purged_deposits() {
        assert_eq!(next_deposit_info_cursor(0, &[0, 1, 3]), 2);
        assert_eq!(next_deposit_info_cursor(2, &[3, 4]), 2);
        assert_eq!(next_deposit_info_cursor(2, &[2, 3, 4]), 5);
    }

    #[test]
    fn withdrawal_status_cursor_advances_over_contiguous_purged_deposits() {
        assert_eq!(
            next_withdrawal_status_cursor(
                WithdrawalStatusCursor {
                    next_deposit_idx: 0
                },
                &[2],
                3,
                &[0, 2]
            ),
            WithdrawalStatusCursor {
                next_deposit_idx: 0
            }
        );
        assert_eq!(
            next_withdrawal_status_cursor(
                WithdrawalStatusCursor {
                    next_deposit_idx: 0
                },
                &[0, 1],
                2,
                &[0, 1]
            ),
            WithdrawalStatusCursor {
                next_deposit_idx: 2
            }
        );
    }

    #[test]
    fn withdrawal_status_cursor_skips_missing_pairings_below_pairing_frontier() {
        assert_eq!(
            next_withdrawal_status_cursor(
                WithdrawalStatusCursor {
                    next_deposit_idx: 0
                },
                &[0],
                3,
                &[0, 2]
            ),
            WithdrawalStatusCursor {
                next_deposit_idx: 2
            }
        );
        assert_eq!(
            next_withdrawal_status_cursor(
                WithdrawalStatusCursor {
                    next_deposit_idx: 0
                },
                &[0, 2],
                3,
                &[0, 2]
            ),
            WithdrawalStatusCursor {
                next_deposit_idx: 3
            }
        );
    }

    #[test]
    fn reimbursement_status_cursor_advances_over_contiguous_purged_deposits() {
        assert_eq!(
            next_reimbursement_status_cursor(
                ReimbursementStatusCursor {
                    next_deposit_idx: 0
                },
                &[2],
                3,
                &[0, 2]
            ),
            ReimbursementStatusCursor {
                next_deposit_idx: 0
            }
        );
        assert_eq!(
            next_reimbursement_status_cursor(
                ReimbursementStatusCursor {
                    next_deposit_idx: 0
                },
                &[0, 1],
                2,
                &[0, 1]
            ),
            ReimbursementStatusCursor {
                next_deposit_idx: 2
            }
        );
    }

    #[test]
    fn reimbursement_status_cursor_skips_missing_withdrawals_below_withdrawal_frontier() {
        assert_eq!(
            next_reimbursement_status_cursor(
                ReimbursementStatusCursor {
                    next_deposit_idx: 0
                },
                &[0],
                3,
                &[0, 2]
            ),
            ReimbursementStatusCursor {
                next_deposit_idx: 2
            }
        );
        assert_eq!(
            next_reimbursement_status_cursor(
                ReimbursementStatusCursor {
                    next_deposit_idx: 0
                },
                &[0, 2],
                3,
                &[0, 2]
            ),
            ReimbursementStatusCursor {
                next_deposit_idx: 3
            }
        );
    }

    #[test]
    fn withdrawal_pairing_planner_stops_at_deposit_gap() {
        let deposit_infos = deposit_infos(&[
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

        assert_eq!(update.pairings, vec![pairing(0, 0), pairing(1, 1)]);
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
        let deposit_infos = deposit_infos(&[(0, DepositStatus::Complete)]);
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
        let deposit_infos = deposit_infos(&[
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

        assert_eq!(update.pairings, vec![pairing(1, 0), pairing(2, 1)]);
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
        let deposit_infos =
            deposit_infos(&[(0, DepositStatus::InProgress), (1, DepositStatus::Complete)]);
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
        let cursor = WithdrawalPairingCursor {
            next_deposit_idx: 2,
            next_withdrawal_seq: 2,
        };
        let deposit_infos =
            deposit_infos(&[(0, DepositStatus::Complete), (1, DepositStatus::Complete)]);
        let update = plan_withdrawal_pairings(cursor, &[0, 1], &deposit_infos, &[0, 1]);

        assert!(update.pairings.is_empty());
        assert_eq!(update.cursor, cursor);
    }

    #[tokio::test]
    async fn hydrates_persisted_status_snapshot() {
        let status_db = BridgeStatusDbSled::open_temporary().expect("open status db");
        let withdrawal_info = WithdrawalInfo {
            withdrawal_request_txid: Buf32([2; 32]),
            fulfillment_txid: Some(Txid::from_byte_array([4; 32])),
            status: WithdrawalStatus::Complete,
        };

        status_db
            .put_withdrawal_info(2, &withdrawal_info)
            .expect("put withdrawal info");
        status_db
            .put_withdrawal_pairings(&[pairing(2, 7)])
            .expect("put withdrawal pairing");
        status_db
            .put_deposit_info_cursor(2)
            .expect("put deposit cursor");
        status_db
            .put_withdrawal_pairing_cursor(WithdrawalPairingCursor {
                next_deposit_idx: 3,
                next_withdrawal_seq: 8,
            })
            .expect("put pairing cursor");
        status_db
            .put_withdrawal_status_cursor(WithdrawalStatusCursor {
                next_deposit_idx: 3,
            })
            .expect("put withdrawal cursor");
        status_db
            .put_reimbursement_status_cursor(ReimbursementStatusCursor {
                next_deposit_idx: 2,
            })
            .expect("put reimbursement cursor");

        let snapshot = status_db
            .get_status_snapshot()
            .expect("load status snapshot");
        let state = BridgeMonitoringState::from_snapshot(snapshot);

        assert_eq!(
            state.withdrawal_pairing_cursor().await,
            WithdrawalPairingCursor {
                next_deposit_idx: 3,
                next_withdrawal_seq: 8,
            }
        );
        assert_eq!(
            state.select_deposit_info_candidates(&[0, 1, 2]).await,
            vec![2]
        );
        assert_eq!(
            state.select_withdrawal_status_candidates().await,
            Vec::<WithdrawalPairing>::new()
        );
        assert_eq!(
            state.select_reimbursement_status_candidates().await,
            vec![2]
        );

        let status = state.bridge_status(6).await;
        assert!(status.deposits.is_empty());
        assert_eq!(status.withdrawals.len(), 1);
        assert!(status.reimbursements.is_empty());
    }

    #[tokio::test]
    async fn deposit_updates_persist_cursor_only() {
        let status_db = BridgeStatusDbSled::open_temporary().expect("open status db");
        let state = BridgeMonitoringState::default();
        let deposit_request_txid = Txid::from_byte_array([6; 32]);
        let deposit_txid = Txid::from_byte_array([7; 32]);

        state
            .apply_deposit_info_updates(
                &status_db,
                vec![DepositInfoUpdate {
                    deposit_idx: 0,
                    info: DepositInfo {
                        deposit_request_txid,
                        deposit_txid: None,
                        status: DepositStatus::InProgress,
                    },
                    confirmations: None,
                }],
                6,
            )
            .await
            .expect("persist deposit cursor");

        let snapshot = status_db
            .get_status_snapshot()
            .expect("load status snapshot");
        assert_eq!(snapshot.cursors.deposit_info, 0);
        assert!(snapshot.withdrawals.is_empty());

        state
            .apply_deposit_info_updates(
                &status_db,
                vec![DepositInfoUpdate {
                    deposit_idx: 0,
                    info: DepositInfo {
                        deposit_request_txid,
                        deposit_txid: Some(deposit_txid),
                        status: DepositStatus::Complete,
                    },
                    confirmations: Some(6),
                }],
                6,
            )
            .await
            .expect("persist deposit cursor");

        let snapshot = status_db
            .get_status_snapshot()
            .expect("load status snapshot");
        assert_eq!(snapshot.cursors.deposit_info, 1);
        assert!(snapshot.withdrawals.is_empty());
        assert_eq!(state.select_deposit_info_candidates(&[0, 1]).await, vec![1]);
    }

    #[tokio::test]
    async fn withdrawal_pairings_persist_rows_and_cursor() {
        let status_db = BridgeStatusDbSled::open_temporary().expect("open status db");
        let state = BridgeMonitoringState::default();
        let requests = vec![
            DbWithdrawalRequestRow {
                seq: 0,
                request: crate::db::withdrawal_index::test_utils::make_withdrawal_request(1),
            },
            DbWithdrawalRequestRow {
                seq: 1,
                request: crate::db::withdrawal_index::test_utils::make_withdrawal_request(2),
            },
        ];

        let complete_deposit_infos =
            deposit_infos(&[(0, DepositStatus::Complete), (1, DepositStatus::Complete)]);
        let pairings = state
            .advance_withdrawal_pairings(&status_db, &[0, 1], &complete_deposit_infos, &requests)
            .await
            .expect("persist withdrawal pairings");

        assert_eq!(pairings, vec![pairing(0, 0), pairing(1, 1)]);
        assert_eq!(
            state.withdrawal_pairing_cursor().await,
            WithdrawalPairingCursor {
                next_deposit_idx: 2,
                next_withdrawal_seq: 2
            }
        );
        let snapshot = status_db
            .get_status_snapshot()
            .expect("load status snapshot");
        assert_eq!(
            snapshot.withdrawal_pairings,
            vec![pairing(0, 0), pairing(1, 1)]
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
        let status_db = BridgeStatusDbSled::open_temporary().expect("open status db");
        let state = BridgeMonitoringState::default();
        let failed_deposit_infos = deposit_infos(&[(0, DepositStatus::Failed)]);

        let pairings = state
            .advance_withdrawal_pairings(&status_db, &[0], &failed_deposit_infos, &[])
            .await
            .expect("persist skipped deposit cursor");

        assert!(pairings.is_empty());
        assert_eq!(
            state.withdrawal_pairing_cursor().await,
            WithdrawalPairingCursor {
                next_deposit_idx: 1,
                next_withdrawal_seq: 0
            }
        );
        let snapshot = status_db
            .get_status_snapshot()
            .expect("load status snapshot");
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
    async fn withdrawal_status_candidates_follow_cursor() {
        let status_db = BridgeStatusDbSled::open_temporary().expect("open status db");
        let state = BridgeMonitoringState::default();
        let requests = vec![
            DbWithdrawalRequestRow {
                seq: 0,
                request: crate::db::withdrawal_index::test_utils::make_withdrawal_request(1),
            },
            DbWithdrawalRequestRow {
                seq: 1,
                request: crate::db::withdrawal_index::test_utils::make_withdrawal_request(2),
            },
        ];

        let complete_deposit_infos =
            deposit_infos(&[(0, DepositStatus::Complete), (1, DepositStatus::Complete)]);
        state
            .advance_withdrawal_pairings(&status_db, &[0, 1], &complete_deposit_infos, &requests)
            .await
            .expect("persist withdrawal pairings");
        assert_eq!(
            state.select_withdrawal_status_candidates().await,
            vec![pairing(0, 0), pairing(1, 1)]
        );

        state
            .apply_withdrawal_updates(
                &status_db,
                vec![WithdrawalInfoUpdate {
                    deposit_idx: 0,
                    info: WithdrawalInfo {
                        withdrawal_request_txid: requests[0].request.tx_hash,
                        fulfillment_txid: Some(Txid::from_byte_array([3; 32])),
                        status: WithdrawalStatus::Complete,
                    },
                    confirmations: Some(6),
                }],
                6,
            )
            .await
            .expect("persist withdrawal status");

        assert_eq!(
            state.select_withdrawal_status_candidates().await,
            vec![pairing(1, 1)]
        );
        let snapshot = status_db
            .get_status_snapshot()
            .expect("load status snapshot");
        assert_eq!(
            snapshot.withdrawal_pairings,
            vec![pairing(1, 1)],
            "confirmed withdrawal pairings below the cursor are purged"
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
    async fn pairing_gc_retries_below_cursor() {
        let status_db = BridgeStatusDbSled::open_temporary().expect("open status db");
        status_db
            .put_withdrawal_pairings(&[pairing(0, 0), pairing(1, 1)])
            .expect("put withdrawal pairings");
        status_db
            .put_withdrawal_status_cursor(WithdrawalStatusCursor {
                next_deposit_idx: 1,
            })
            .expect("put withdrawal status cursor");

        let snapshot = status_db
            .get_status_snapshot()
            .expect("load status snapshot");
        let state = BridgeMonitoringState::from_snapshot(snapshot);

        state
            .apply_withdrawal_updates(&status_db, Vec::new(), 6)
            .await
            .expect("persist withdrawal status");

        let snapshot = status_db
            .get_status_snapshot()
            .expect("load status snapshot");
        assert_eq!(snapshot.withdrawal_pairings, vec![pairing(1, 1)]);
        assert_eq!(
            state.select_withdrawal_status_candidates().await,
            vec![pairing(1, 1)]
        );
    }

    #[tokio::test]
    async fn pairing_gc_skips_missing_pairings_below_pairing_frontier() {
        let status_db = BridgeStatusDbSled::open_temporary().expect("open status db");
        status_db
            .put_withdrawal_pairings(&[pairing(0, 0), pairing(2, 1)])
            .expect("put withdrawal pairings");
        status_db
            .put_withdrawal_pairing_cursor(WithdrawalPairingCursor {
                next_deposit_idx: 3,
                next_withdrawal_seq: 2,
            })
            .expect("put withdrawal pairing cursor");

        let snapshot = status_db
            .get_status_snapshot()
            .expect("load status snapshot");
        let state = BridgeMonitoringState::from_snapshot(snapshot);

        state
            .apply_withdrawal_updates(
                &status_db,
                vec![WithdrawalInfoUpdate {
                    deposit_idx: 0,
                    info: WithdrawalInfo {
                        withdrawal_request_txid: Buf32([20; 32]),
                        fulfillment_txid: Some(Txid::from_byte_array([21; 32])),
                        status: WithdrawalStatus::Complete,
                    },
                    confirmations: Some(6),
                }],
                6,
            )
            .await
            .expect("persist withdrawal status");

        let snapshot = status_db
            .get_status_snapshot()
            .expect("load status snapshot");
        assert_eq!(
            snapshot.cursors.withdrawal_status,
            WithdrawalStatusCursor {
                next_deposit_idx: 2
            }
        );
        assert_eq!(snapshot.withdrawal_pairings, vec![pairing(2, 1)]);

        state
            .apply_withdrawal_updates(
                &status_db,
                vec![WithdrawalInfoUpdate {
                    deposit_idx: 2,
                    info: WithdrawalInfo {
                        withdrawal_request_txid: Buf32([22; 32]),
                        fulfillment_txid: Some(Txid::from_byte_array([23; 32])),
                        status: WithdrawalStatus::Complete,
                    },
                    confirmations: Some(6),
                }],
                6,
            )
            .await
            .expect("persist withdrawal status");

        let snapshot = status_db
            .get_status_snapshot()
            .expect("load status snapshot");
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
        let status_db = BridgeStatusDbSled::open_temporary().expect("open status db");
        let state = BridgeMonitoringState::default();
        let withdrawal_request_txid = Buf32([9; 32]);

        state
            .apply_withdrawal_updates(
                &status_db,
                vec![
                    WithdrawalInfoUpdate {
                        deposit_idx: 0,
                        info: WithdrawalInfo {
                            withdrawal_request_txid,
                            fulfillment_txid: None,
                            status: WithdrawalStatus::InProgress,
                        },
                        confirmations: None,
                    },
                    WithdrawalInfoUpdate {
                        deposit_idx: 1,
                        info: WithdrawalInfo {
                            withdrawal_request_txid,
                            fulfillment_txid: None,
                            status: WithdrawalStatus::InProgress,
                        },
                        confirmations: None,
                    },
                ],
                6,
            )
            .await
            .expect("persist withdrawal status");

        let status = state.bridge_status(6).await;

        assert_eq!(status.withdrawals.len(), 2);
        assert!(status
            .withdrawals
            .iter()
            .all(|info| info.withdrawal_request_txid == withdrawal_request_txid));
        assert!(
            status_db
                .get_status_snapshot()
                .expect("load status snapshot")
                .withdrawals
                .is_empty(),
            "in-progress withdrawal rows are cache-only"
        );
    }

    #[tokio::test]
    async fn reimbursement_candidates_use_complete_withdrawals() {
        let status_db = BridgeStatusDbSled::open_temporary().expect("open status db");
        let state = BridgeMonitoringState::default();
        let requests = vec![DbWithdrawalRequestRow {
            seq: 0,
            request: crate::db::withdrawal_index::test_utils::make_withdrawal_request(1),
        }];

        assert_eq!(
            state.select_reimbursement_status_candidates().await,
            Vec::<DepositIdx>::new()
        );

        let complete_deposit_infos = deposit_infos(&[(0, DepositStatus::Complete)]);
        state
            .advance_withdrawal_pairings(&status_db, &[0], &complete_deposit_infos, &requests)
            .await
            .expect("persist withdrawal pairings");
        assert_eq!(
            state.select_reimbursement_status_candidates().await,
            Vec::<DepositIdx>::new()
        );

        state
            .apply_withdrawal_updates(
                &status_db,
                vec![WithdrawalInfoUpdate {
                    deposit_idx: 0,
                    info: WithdrawalInfo {
                        withdrawal_request_txid: requests[0].request.tx_hash,
                        fulfillment_txid: Some(Txid::from_byte_array([3; 32])),
                        status: WithdrawalStatus::Complete,
                    },
                    confirmations: Some(1),
                }],
                6,
            )
            .await
            .expect("persist withdrawal status");
        assert_eq!(
            status_db
                .get_status_snapshot()
                .expect("load status snapshot")
                .cursors
                .withdrawal_status,
            WithdrawalStatusCursor {
                next_deposit_idx: 0
            }
        );
        assert_eq!(
            state.select_reimbursement_status_candidates().await,
            vec![0]
        );
        assert_eq!(state.bridge_status(6).await.withdrawals.len(), 1);

        state
            .apply_withdrawal_updates(
                &status_db,
                vec![WithdrawalInfoUpdate {
                    deposit_idx: 0,
                    info: WithdrawalInfo {
                        withdrawal_request_txid: requests[0].request.tx_hash,
                        fulfillment_txid: Some(Txid::from_byte_array([3; 32])),
                        status: WithdrawalStatus::Complete,
                    },
                    confirmations: Some(6),
                }],
                6,
            )
            .await
            .expect("persist confirmed withdrawal status");
        assert_eq!(
            state.select_reimbursement_status_candidates().await,
            vec![0]
        );
        assert!(state.bridge_status(6).await.withdrawals.is_empty());

        state
            .apply_reimbursement_updates(
                &status_db,
                vec![ReimbursementInfoUpdate {
                    deposit_idx: 0,
                    info: ReimbursementInfo {
                        claim_txid: Txid::from_byte_array([4; 32]),
                        challenge_step: crate::types::ChallengeStep::Claimed,
                        payout_txid: None,
                        status: ReimbursementStatus::InProgress,
                    },
                    confirmations: None,
                }],
                6,
            )
            .await
            .expect("persist active reimbursement status");
        let snapshot = status_db
            .get_status_snapshot()
            .expect("load status snapshot");
        assert_eq!(
            snapshot.cursors.reimbursement_status,
            ReimbursementStatusCursor::default()
        );
        assert_eq!(snapshot.withdrawals.len(), 1);

        state
            .apply_reimbursement_updates(
                &status_db,
                vec![ReimbursementInfoUpdate {
                    deposit_idx: 0,
                    info: ReimbursementInfo {
                        claim_txid: Txid::from_byte_array([4; 32]),
                        challenge_step: crate::types::ChallengeStep::NotApplicable,
                        payout_txid: Some(Txid::from_byte_array([5; 32])),
                        status: ReimbursementStatus::Complete,
                    },
                    confirmations: Some(6),
                }],
                6,
            )
            .await
            .expect("persist reimbursement status");

        assert_eq!(
            state.select_reimbursement_status_candidates().await,
            Vec::<DepositIdx>::new()
        );
        let snapshot = status_db
            .get_status_snapshot()
            .expect("load status snapshot");
        assert_eq!(
            snapshot.cursors.reimbursement_status,
            ReimbursementStatusCursor {
                next_deposit_idx: 1
            }
        );
        assert!(snapshot.withdrawals.is_empty());
        assert!(state.bridge_status(6).await.withdrawals.is_empty());
    }

    #[tokio::test]
    async fn reimbursement_gc_skips_missing_withdrawals_below_withdrawal_frontier() {
        let status_db = BridgeStatusDbSled::open_temporary().expect("open status db");
        status_db
            .put_withdrawal_info(
                0,
                &WithdrawalInfo {
                    withdrawal_request_txid: Buf32([30; 32]),
                    fulfillment_txid: Some(Txid::from_byte_array([31; 32])),
                    status: WithdrawalStatus::Complete,
                },
            )
            .expect("put withdrawal info");
        status_db
            .put_withdrawal_info(
                2,
                &WithdrawalInfo {
                    withdrawal_request_txid: Buf32([32; 32]),
                    fulfillment_txid: Some(Txid::from_byte_array([33; 32])),
                    status: WithdrawalStatus::Complete,
                },
            )
            .expect("put withdrawal info");
        status_db
            .put_withdrawal_status_cursor(WithdrawalStatusCursor {
                next_deposit_idx: 3,
            })
            .expect("put withdrawal cursor");

        let snapshot = status_db
            .get_status_snapshot()
            .expect("load status snapshot");
        let state = BridgeMonitoringState::from_snapshot(snapshot);

        state
            .apply_reimbursement_updates(
                &status_db,
                vec![ReimbursementInfoUpdate {
                    deposit_idx: 0,
                    info: ReimbursementInfo {
                        claim_txid: Txid::from_byte_array([34; 32]),
                        challenge_step: crate::types::ChallengeStep::NotApplicable,
                        payout_txid: Some(Txid::from_byte_array([35; 32])),
                        status: ReimbursementStatus::Complete,
                    },
                    confirmations: Some(6),
                }],
                6,
            )
            .await
            .expect("persist reimbursement status");

        let snapshot = status_db
            .get_status_snapshot()
            .expect("load status snapshot");
        assert_eq!(
            snapshot.cursors.reimbursement_status,
            ReimbursementStatusCursor {
                next_deposit_idx: 2
            }
        );
        assert_eq!(snapshot.withdrawals.len(), 1);
        assert_eq!(snapshot.withdrawals[0].0, 2);

        state
            .apply_reimbursement_updates(
                &status_db,
                vec![ReimbursementInfoUpdate {
                    deposit_idx: 2,
                    info: ReimbursementInfo {
                        claim_txid: Txid::from_byte_array([36; 32]),
                        challenge_step: crate::types::ChallengeStep::NotApplicable,
                        payout_txid: Some(Txid::from_byte_array([37; 32])),
                        status: ReimbursementStatus::Complete,
                    },
                    confirmations: Some(6),
                }],
                6,
            )
            .await
            .expect("persist reimbursement status");

        let snapshot = status_db
            .get_status_snapshot()
            .expect("load status snapshot");
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
        let status_db = BridgeStatusDbSled::open_temporary().expect("open status db");
        let state = BridgeMonitoringState::default();

        state
            .apply_withdrawal_updates(
                &status_db,
                vec![
                    WithdrawalInfoUpdate {
                        deposit_idx: 0,
                        info: WithdrawalInfo {
                            withdrawal_request_txid: Buf32([10; 32]),
                            fulfillment_txid: None,
                            status: WithdrawalStatus::InProgress,
                        },
                        confirmations: None,
                    },
                    WithdrawalInfoUpdate {
                        deposit_idx: 1,
                        info: WithdrawalInfo {
                            withdrawal_request_txid: Buf32([11; 32]),
                            fulfillment_txid: Some(Txid::from_byte_array([12; 32])),
                            status: WithdrawalStatus::Complete,
                        },
                        confirmations: Some(1),
                    },
                ],
                6,
            )
            .await
            .expect("persist withdrawal status");

        assert_eq!(
            state.select_reimbursement_status_candidates().await,
            vec![1]
        );
    }

    #[tokio::test]
    async fn reimbursement_candidates_use_retained_complete_rows_when_cursor_lags() {
        let status_db = BridgeStatusDbSled::open_temporary().expect("open status db");
        status_db
            .put_withdrawal_info(
                2,
                &WithdrawalInfo {
                    withdrawal_request_txid: Buf32([13; 32]),
                    fulfillment_txid: Some(Txid::from_byte_array([14; 32])),
                    status: WithdrawalStatus::Complete,
                },
            )
            .expect("put withdrawal info");
        status_db
            .put_withdrawal_status_cursor(WithdrawalStatusCursor {
                next_deposit_idx: 3,
            })
            .expect("put withdrawal status cursor");
        status_db
            .put_reimbursement_status_cursor(ReimbursementStatusCursor {
                next_deposit_idx: 1,
            })
            .expect("put reimbursement status cursor");

        let snapshot = status_db
            .get_status_snapshot()
            .expect("load status snapshot");
        let state = BridgeMonitoringState::from_snapshot(snapshot);

        assert_eq!(
            state.select_reimbursement_status_candidates().await,
            vec![2]
        );
    }
}
