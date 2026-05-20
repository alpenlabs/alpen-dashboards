use std::collections::BTreeSet;

use strata_bridge_primitives::types::DepositIdx;
use strata_primitives::buf::Buf32;
use tokio::sync::RwLock;

use super::{
    cache::BridgeStatusCache,
    types::{
        BridgeStatus, DepositInfo, DepositStatus, OperatorStatus, ReimbursementInfo,
        ReimbursementStatus, WithdrawalInfo,
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

    pub(crate) async fn update_operators(&self, operators: Vec<OperatorStatus>) {
        let mut cache = self.cache.write().await;
        cache.update_operators(operators);
    }

    pub(crate) async fn apply_withdrawal_updates(
        &self,
        updates: Vec<(Buf32, WithdrawalInfo, u64)>,
    ) {
        let mut cache = self.cache.write().await;
        cache.apply_withdrawal_updates(updates);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deposit_info_cursor_advances_over_contiguous_purged_deposits() {
        assert_eq!(next_deposit_info_cursor(0, &[0, 1, 3]), 2);
        assert_eq!(next_deposit_info_cursor(2, &[3, 4]), 2);
        assert_eq!(next_deposit_info_cursor(2, &[2, 3, 4]), 5);
    }
}
