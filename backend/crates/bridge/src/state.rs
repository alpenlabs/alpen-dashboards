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

/// Mutable bridge monitoring state shared by the polling task and HTTP handler.
#[derive(Debug, Default)]
pub(crate) struct BridgeMonitoringState {
    cache: RwLock<BridgeStatusCache>,
}

impl BridgeMonitoringState {
    pub(crate) async fn update_operators(&self, operators: Vec<OperatorStatus>) {
        let mut cache = self.cache.write().await;
        cache.update_operators(operators);
    }

    pub(crate) async fn apply_deposit_updates(
        &self,
        updates: Vec<(DepositIdx, DepositInfo, u64)>,
    ) -> Vec<(DepositIdx, DepositInfo)> {
        let mut cache = self.cache.write().await;
        cache.apply_deposit_updates(updates);
        cache.filter_deposits(|s| matches!(s, DepositStatus::Complete | DepositStatus::Failed))
    }

    pub(crate) async fn purge_deposits(&self, deposits_to_purge: Vec<DepositIdx>) {
        let mut cache = self.cache.write().await;
        cache.purge_deposits(deposits_to_purge);
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
