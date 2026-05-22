use std::collections::BTreeSet;

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
        ReimbursementStatus, ReimbursementStatusCursor, WithdrawalInfo, WithdrawalPairingCursor,
        WithdrawalSeq, WithdrawalStatus, WithdrawalStatusCursor,
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

/// Paired withdrawal candidate whose bridge status can be queried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WithdrawalStatusCandidate {
    pub(crate) deposit_idx: DepositIdx,
    pub(crate) withdrawal_seq: WithdrawalSeq,
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

/// Mutable bridge monitoring state shared by the polling task and HTTP handler.
#[derive(Debug, Default)]
pub(crate) struct BridgeMonitoringState {
    cache: RwLock<BridgeStatusCache>,
}

impl BridgeMonitoringState {
    pub(crate) fn from_snapshot(snapshot: DbBridgeStatusSnapshot) -> Self {
        Self {
            cache: RwLock::new(cache_from_status_snapshot(snapshot)),
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

    pub(crate) async fn apply_withdrawal_pairings(
        &self,
        status_db: &impl BridgeStatusDb,
        deposit_indices: &[DepositIdx],
        withdrawal_requests: &[DbWithdrawalRequestRow],
    ) -> DbResult<Vec<(DepositIdx, WithdrawalSeq)>> {
        let withdrawal_seqs = withdrawal_requests
            .iter()
            .map(|row| row.seq)
            .collect::<Vec<_>>();
        let current_cursor = {
            let cache = self.cache.read().await;
            cache.withdrawal_pairing_cursor()
        };
        let (pairings, next_cursor) =
            plan_withdrawal_pairings(current_cursor, deposit_indices, &withdrawal_seqs);
        if pairings.is_empty() {
            return Ok(pairings);
        }

        status_db.put_withdrawal_pairings(&pairings)?;
        status_db.put_withdrawal_pairing_cursor(next_cursor)?;

        let mut cache = self.cache.write().await;
        cache.update_withdrawal_pairings(&pairings, next_cursor);
        Ok(pairings)
    }

    pub(crate) async fn select_withdrawal_status_candidates(
        &self,
    ) -> Vec<WithdrawalStatusCandidate> {
        let cache = self.cache.read().await;
        let cursor = cache.withdrawal_status_cursor().next_deposit_idx;
        cache
            .withdrawal_pairings_from(cursor)
            .into_iter()
            .map(|(deposit_idx, withdrawal_seq)| WithdrawalStatusCandidate {
                deposit_idx,
                withdrawal_seq,
            })
            .collect()
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

        let current_cursor = {
            let cache = self.cache.read().await;
            cache.withdrawal_status_cursor()
        };
        let next_cursor =
            next_withdrawal_status_cursor(current_cursor, &terminal_deposit_indices_to_purge);
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
        let cursor = cache.reimbursement_status_cursor().next_deposit_idx;
        cache
            .filter_withdrawals(|status| matches!(status, WithdrawalStatus::Complete))
            .into_iter()
            .filter_map(|(deposit_idx, _)| (deposit_idx >= cursor).then_some(deposit_idx))
            .collect()
    }

    pub(crate) async fn apply_reimbursement_updates(
        &self,
        updates: Vec<ReimbursementInfoUpdate>,
        max_confirmations: u64,
    ) {
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

        let mut cache = self.cache.write().await;
        let next_cursor = next_reimbursement_status_cursor(
            cache.reimbursement_status_cursor(),
            &terminal_deposit_indices_to_purge,
        );
        cache.apply_reimbursement_updates(cache_updates);
        cache.purge_reimbursements(terminal_deposit_indices_to_purge);
        cache.set_reimbursement_status_cursor(next_cursor);
    }

    pub(crate) async fn bridge_status(&self) -> BridgeStatus {
        let cache = self.cache.read().await;

        BridgeStatus {
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
        }
    }
}

fn cache_from_status_snapshot(snapshot: DbBridgeStatusSnapshot) -> BridgeStatusCache {
    let mut cache = BridgeStatusCache::default();

    cache.apply_withdrawal_updates(
        snapshot
            .withdrawals
            .into_iter()
            .map(|(deposit_idx, info)| (deposit_idx, info, None))
            .collect(),
    );
    cache.update_withdrawal_pairings(
        &snapshot.withdrawal_pairings,
        snapshot.cursors.withdrawal_pairing,
    );
    cache.set_deposit_info_cursor(snapshot.cursors.deposit_info);
    cache.set_withdrawal_status_cursor(snapshot.cursors.withdrawal_status);
    cache.set_reimbursement_status_cursor(snapshot.cursors.reimbursement_status);

    cache
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
) -> WithdrawalStatusCursor {
    let terminal_deposit_indices_to_purge = terminal_deposit_indices_to_purge
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let mut next_cursor = current_cursor.next_deposit_idx;

    while terminal_deposit_indices_to_purge.contains(&next_cursor) {
        let Some(next) = next_cursor.checked_add(1) else {
            break;
        };
        next_cursor = next;
    }

    WithdrawalStatusCursor {
        next_deposit_idx: next_cursor,
    }
}

fn next_reimbursement_status_cursor(
    current_cursor: ReimbursementStatusCursor,
    terminal_deposit_indices_to_purge: &[DepositIdx],
) -> ReimbursementStatusCursor {
    let terminal_deposit_indices_to_purge = terminal_deposit_indices_to_purge
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let mut next_cursor = current_cursor.next_deposit_idx;

    while terminal_deposit_indices_to_purge.contains(&next_cursor) {
        let Some(next) = next_cursor.checked_add(1) else {
            break;
        };
        next_cursor = next;
    }

    ReimbursementStatusCursor {
        next_deposit_idx: next_cursor,
    }
}

fn plan_withdrawal_pairings(
    cursor: WithdrawalPairingCursor,
    discovered_deposit_indices: &[DepositIdx],
    indexed_withdrawal_seqs: &[WithdrawalSeq],
) -> (Vec<(DepositIdx, WithdrawalSeq)>, WithdrawalPairingCursor) {
    let discovered_deposit_indices = discovered_deposit_indices
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let indexed_withdrawal_seqs = indexed_withdrawal_seqs
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let mut next_cursor = cursor;
    let mut pairings = Vec::new();

    while discovered_deposit_indices.contains(&next_cursor.next_deposit_idx)
        && indexed_withdrawal_seqs.contains(&next_cursor.next_withdrawal_seq)
    {
        pairings.push((
            next_cursor.next_deposit_idx,
            next_cursor.next_withdrawal_seq,
        ));

        let Some(next_deposit_idx) = next_cursor.next_deposit_idx.checked_add(1) else {
            break;
        };
        let Some(next_withdrawal_seq) = next_cursor.next_withdrawal_seq.checked_add(1) else {
            break;
        };
        next_cursor = WithdrawalPairingCursor {
            next_deposit_idx,
            next_withdrawal_seq,
        };
    }

    (pairings, next_cursor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::{hashes::Hash, Txid};
    use strata_primitives::buf::Buf32;

    use crate::db::{traits::BridgeStatusDb, BridgeStatusDbSled};

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
                &[2]
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
                &[0, 1]
            ),
            WithdrawalStatusCursor {
                next_deposit_idx: 2
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
                &[2]
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
                &[0, 1]
            ),
            ReimbursementStatusCursor {
                next_deposit_idx: 2
            }
        );
    }

    #[test]
    fn withdrawal_pairing_planner_stops_at_deposit_gap() {
        let (pairings, cursor) =
            plan_withdrawal_pairings(WithdrawalPairingCursor::default(), &[0, 1, 3], &[0, 1, 2]);

        assert_eq!(pairings, vec![(0, 0), (1, 1)]);
        assert_eq!(
            cursor,
            WithdrawalPairingCursor {
                next_deposit_idx: 2,
                next_withdrawal_seq: 2
            }
        );
    }

    #[test]
    fn withdrawal_pairing_planner_stops_without_indexed_wrt() {
        let (pairings, cursor) =
            plan_withdrawal_pairings(WithdrawalPairingCursor::default(), &[0], &[]);

        assert!(pairings.is_empty());
        assert_eq!(cursor, WithdrawalPairingCursor::default());
    }

    #[test]
    fn withdrawal_pairing_planner_does_not_repair_old_indices() {
        let cursor = WithdrawalPairingCursor {
            next_deposit_idx: 2,
            next_withdrawal_seq: 2,
        };
        let (pairings, next_cursor) = plan_withdrawal_pairings(cursor, &[0, 1], &[0, 1]);

        assert!(pairings.is_empty());
        assert_eq!(next_cursor, cursor);
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
            .put_withdrawal_pairings(&[(2, 7)])
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
            Vec::<WithdrawalStatusCandidate>::new()
        );
        assert_eq!(
            state.select_reimbursement_status_candidates().await,
            vec![2]
        );

        let status = state.bridge_status().await;
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

        let pairings = state
            .apply_withdrawal_pairings(&status_db, &[0, 1], &requests)
            .await
            .expect("persist withdrawal pairings");

        assert_eq!(pairings, vec![(0, 0), (1, 1)]);
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
        assert_eq!(snapshot.withdrawal_pairings, vec![(0, 0), (1, 1)]);
        assert_eq!(
            snapshot.cursors.withdrawal_pairing,
            WithdrawalPairingCursor {
                next_deposit_idx: 2,
                next_withdrawal_seq: 2
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

        state
            .apply_withdrawal_pairings(&status_db, &[0, 1], &requests)
            .await
            .expect("persist withdrawal pairings");
        assert_eq!(
            state.select_withdrawal_status_candidates().await,
            vec![
                WithdrawalStatusCandidate {
                    deposit_idx: 0,
                    withdrawal_seq: 0
                },
                WithdrawalStatusCandidate {
                    deposit_idx: 1,
                    withdrawal_seq: 1
                }
            ]
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
            vec![WithdrawalStatusCandidate {
                deposit_idx: 1,
                withdrawal_seq: 1
            }]
        );
        let snapshot = status_db
            .get_status_snapshot()
            .expect("load status snapshot");
        assert_eq!(
            snapshot.withdrawal_pairings,
            vec![(1, 1)],
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
            .put_withdrawal_pairings(&[(0, 0), (1, 1)])
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
        assert_eq!(snapshot.withdrawal_pairings, vec![(1, 1)]);
        assert_eq!(
            state.select_withdrawal_status_candidates().await,
            vec![WithdrawalStatusCandidate {
                deposit_idx: 1,
                withdrawal_seq: 1
            }]
        );
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

        let status = state.bridge_status().await;

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

        state
            .apply_withdrawal_pairings(&status_db, &[0], &requests)
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

        state
            .apply_reimbursement_updates(
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
            .await;

        assert_eq!(
            state.select_reimbursement_status_candidates().await,
            Vec::<DepositIdx>::new()
        );
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
}
