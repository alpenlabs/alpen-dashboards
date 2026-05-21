use std::collections::BTreeSet;

use strata_bridge_primitives::types::DepositIdx;
use tokio::sync::RwLock;

use super::{
    cache::{BridgeStatusCache, WithdrawalPairingCursor, WithdrawalStatusCursor},
    db::types::DbWithdrawalRequestRow,
    types::{
        BridgeStatus, DepositInfo, DepositStatus, OperatorStatus, ReimbursementInfo,
        ReimbursementStatus, WithdrawalInfo, WithdrawalStatus,
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
    pub(crate) withdrawal_seq: u64,
}

/// Withdrawal status update collected during one monitoring tick.
#[derive(Debug)]
pub(crate) struct WithdrawalInfoUpdate {
    pub(crate) deposit_idx: DepositIdx,
    pub(crate) info: WithdrawalInfo,
    pub(crate) confirmations: Option<u64>,
}

/// Mutable bridge monitoring state shared by the polling task and HTTP handler.
#[derive(Debug, Default)]
pub(crate) struct BridgeMonitoringState {
    cache: RwLock<BridgeStatusCache>,
}

impl BridgeMonitoringState {
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
        updates: Vec<DepositInfoUpdate>,
        max_confirmations: u64,
    ) {
        let mut cache_updates = Vec::new();
        let mut terminal_deposit_indices_to_purge = Vec::new();

        for update in updates {
            match update.info.status {
                DepositStatus::InProgress => {
                    cache_updates.push((update.deposit_idx, update.info, 0));
                }
                DepositStatus::Failed | DepositStatus::Complete => {
                    let Some(confirmations) = update.confirmations else {
                        continue;
                    };

                    if confirmations >= max_confirmations {
                        terminal_deposit_indices_to_purge.push(update.deposit_idx);
                    } else {
                        cache_updates.push((update.deposit_idx, update.info, confirmations));
                    }
                }
            }
        }

        let mut cache = self.cache.write().await;
        let next_cursor = next_deposit_info_cursor(
            cache.deposit_info_cursor(),
            &terminal_deposit_indices_to_purge,
        );
        cache.apply_deposit_updates(cache_updates);
        cache.purge_deposits(terminal_deposit_indices_to_purge);
        cache.set_deposit_info_cursor(next_cursor);
    }

    pub(crate) async fn withdrawal_pairing_cursor(&self) -> WithdrawalPairingCursor {
        let cache = self.cache.read().await;
        cache.withdrawal_pairing_cursor()
    }

    pub(crate) async fn apply_withdrawal_pairings(
        &self,
        deposit_indices: &[DepositIdx],
        withdrawal_requests: &[DbWithdrawalRequestRow],
    ) -> Vec<(DepositIdx, u64)> {
        let withdrawal_seqs = withdrawal_requests
            .iter()
            .map(|row| row.seq)
            .collect::<Vec<_>>();
        let mut cache = self.cache.write().await;
        let (pairings, next_cursor) = plan_withdrawal_pairings(
            cache.withdrawal_pairing_cursor(),
            deposit_indices,
            &withdrawal_seqs,
        );

        cache.update_withdrawal_pairings(&pairings, next_cursor);
        pairings
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
        updates: Vec<WithdrawalInfoUpdate>,
        max_confirmations: u64,
    ) {
        let mut cache_updates = Vec::new();
        let mut terminal_deposit_indices_to_purge = Vec::new();
        let mut withdrawal_deposit_indices_to_purge = Vec::new();

        for update in updates {
            match update.info.status {
                WithdrawalStatus::InProgress => {
                    cache_updates.push((update.deposit_idx, update.info, 0));
                }
                WithdrawalStatus::Complete => {
                    let Some(confirmations) = update.confirmations else {
                        continue;
                    };

                    if confirmations >= max_confirmations {
                        terminal_deposit_indices_to_purge.push(update.deposit_idx);
                        withdrawal_deposit_indices_to_purge.push(update.deposit_idx);
                    } else {
                        cache_updates.push((update.deposit_idx, update.info, confirmations));
                    }
                }
            }
        }

        let mut cache = self.cache.write().await;
        let next_cursor = next_withdrawal_status_cursor(
            cache.withdrawal_status_cursor(),
            &terminal_deposit_indices_to_purge,
        );
        cache.apply_withdrawal_updates(cache_updates);
        cache.purge_withdrawals(withdrawal_deposit_indices_to_purge);
        cache.set_withdrawal_status_cursor(next_cursor);
    }

    pub(crate) async fn apply_reimbursement_updates(
        &self,
        updates: Vec<(DepositIdx, ReimbursementInfo, u64)>,
    ) -> Vec<(DepositIdx, ReimbursementInfo)> {
        let mut cache = self.cache.write().await;
        cache.apply_reimbursement_updates(updates);
        cache.filter_reimbursements(|s| {
            matches!(
                s,
                ReimbursementStatus::Complete
                    | ReimbursementStatus::Slashed
                    | ReimbursementStatus::Aborted
            )
        })
    }

    pub(crate) async fn purge_reimbursements(&self, reimbursements_to_purge: Vec<DepositIdx>) {
        let mut cache = self.cache.write().await;
        cache.purge_reimbursements(reimbursements_to_purge);
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

fn plan_withdrawal_pairings(
    cursor: WithdrawalPairingCursor,
    discovered_deposit_indices: &[DepositIdx],
    indexed_withdrawal_seqs: &[u64],
) -> (Vec<(DepositIdx, u64)>, WithdrawalPairingCursor) {
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
    async fn withdrawal_pairings_apply_atomically_with_cursor() {
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

        let pairings = state.apply_withdrawal_pairings(&[0, 1], &requests).await;

        assert_eq!(pairings, vec![(0, 0), (1, 1)]);
        assert_eq!(
            state.withdrawal_pairing_cursor().await,
            WithdrawalPairingCursor {
                next_deposit_idx: 2,
                next_withdrawal_seq: 2
            }
        );
    }

    #[tokio::test]
    async fn withdrawal_status_candidates_follow_cursor() {
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

        state.apply_withdrawal_pairings(&[0, 1], &requests).await;
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
            .await;

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
        let state = BridgeMonitoringState::default();
        let withdrawal_request_txid = Buf32([9; 32]);

        state
            .apply_withdrawal_updates(
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
            .await;

        let status = state.bridge_status().await;

        assert_eq!(status.withdrawals.len(), 2);
        assert!(status
            .withdrawals
            .iter()
            .all(|info| info.withdrawal_request_txid == withdrawal_request_txid));
    }
}
