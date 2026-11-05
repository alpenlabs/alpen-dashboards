//! Shared constants, config, row factories and monitoring fixture for bridge tests.

use std::sync::Arc;

use bitcoin::{hashes::Hash, Txid};
pub(crate) use status_config::test_utils::TEST_MAX_CONFIRMATIONS;
use status_config::test_utils::{make_bridge_config, LOCAL_ESPLORA_URL};
use strata_bridge_primitives::types::DepositIdx;
use strata_retry::RetryConfig;

use crate::{
    context::BridgeMonitoringContext,
    db::{
        status::mock::MockBridgeStatusDb,
        traits::{BridgeStatusDb, WithdrawalIndexerDb},
        types::{BridgeStatusDbSnapshot, DbWithdrawalRequestRow},
        withdrawal_index::{mock::MockWithdrawalIndexerDb, test_utils::make_withdrawal_request},
    },
    state::{
        BridgeMonitoringState, DepositInfoUpdate, ReimbursementInfoUpdate, WithdrawalInfoUpdate,
    },
    types::{
        BridgeStatus, DepositInfo, DepositStatus, ReimbursementInfo, WithdrawalInfo,
        WithdrawalPairing, WithdrawalSeq, WithdrawalStatus,
    },
    withdrawal_indexer::TASK_NAME,
};

/// Monitoring context over mock databases, with setup through production transitions.
pub(crate) struct BridgeStatusFixture {
    context: Arc<BridgeMonitoringContext>,
}

impl Default for BridgeStatusFixture {
    fn default() -> Self {
        Self::new(MockBridgeStatusDb::default())
    }
}

impl BridgeStatusFixture {
    pub(crate) fn new(status_db: impl BridgeStatusDb) -> Self {
        Self::with_dbs(MockWithdrawalIndexerDb::default(), status_db)
    }

    fn with_dbs(
        withdrawal_index_db: impl WithdrawalIndexerDb,
        status_db: impl BridgeStatusDb,
    ) -> Self {
        Self {
            context: Arc::new(
                BridgeMonitoringContext::new(
                    make_bridge_config(LOCAL_ESPLORA_URL, &[], None),
                    Arc::new(withdrawal_index_db),
                    Arc::new(status_db),
                )
                .expect("create bridge monitoring context"),
            ),
        }
    }

    pub(crate) fn context(&self) -> &Arc<BridgeMonitoringContext> {
        &self.context
    }

    pub(crate) fn state(&self) -> &BridgeMonitoringState {
        self.context.state()
    }

    pub(crate) fn status_db(&self) -> &dyn BridgeStatusDb {
        self.context.status_db()
    }

    pub(crate) fn snapshot(&self) -> BridgeStatusDbSnapshot {
        self.status_db()
            .get_status_snapshot()
            .expect("load status snapshot")
    }

    /// Rebuilds the context from a copy of both databases' persisted records.
    pub(crate) fn restart(&mut self) {
        let withdrawal_index_db = self.context.withdrawal_index_db();
        let rows = withdrawal_index_db
            .fetch_withdrawal_requests_from(0, usize::MAX)
            .expect("read indexed withdrawal requests");
        let state = withdrawal_index_db
            .get_indexer_state(TASK_NAME)
            .expect("read indexer state");
        *self = Self::with_dbs(
            MockWithdrawalIndexerDb::from_snapshot(rows, state),
            MockBridgeStatusDb::from_snapshot(self.snapshot()),
        );
    }

    /// Bridge status at the configured confirmation cutoff.
    pub(crate) async fn bridge_status(&self) -> BridgeStatus {
        self.context.bridge_status().await
    }

    pub(crate) fn index_withdrawal_event(&self, seed: u8) {
        self.context
            .withdrawal_index_db()
            .insert_withdrawal_event(&[make_withdrawal_request(seed)])
            .expect("index withdrawal event");
    }

    pub(crate) async fn apply_deposit_update(
        &self,
        deposit_idx: DepositIdx,
        info: DepositInfo,
        confirmations: Option<u64>,
    ) {
        self.state()
            .apply_deposit_info_updates(
                self.status_db(),
                vec![DepositInfoUpdate {
                    deposit_idx,
                    info,
                    confirmations,
                }],
                self.context.config().max_tx_confirmations(),
            )
            .await
            .expect("apply deposit update");
    }

    pub(crate) async fn apply_withdrawal_update(
        &self,
        deposit_idx: DepositIdx,
        info: WithdrawalInfo,
        confirmations: Option<u64>,
    ) {
        self.apply_withdrawal_updates([WithdrawalInfoUpdate {
            deposit_idx,
            info,
            confirmations,
        }])
        .await;
    }

    pub(crate) async fn apply_withdrawal_updates(
        &self,
        updates: impl IntoIterator<Item = WithdrawalInfoUpdate>,
    ) {
        self.state()
            .apply_withdrawal_updates(
                self.status_db(),
                updates.into_iter().collect(),
                self.context.config().max_tx_confirmations(),
            )
            .await
            .expect("apply withdrawal updates");
    }

    pub(crate) async fn apply_reimbursement_update(
        &self,
        deposit_idx: DepositIdx,
        info: ReimbursementInfo,
        confirmations: Option<u64>,
    ) {
        self.state()
            .apply_reimbursement_updates(
                self.status_db(),
                vec![ReimbursementInfoUpdate {
                    deposit_idx,
                    info,
                    confirmations,
                }],
                self.context.config().max_tx_confirmations(),
            )
            .await
            .expect("apply reimbursement update");
    }

    pub(crate) async fn advance_withdrawal_pairings(
        &self,
        deposit_indices: &[DepositIdx],
        deposit_infos: &[(DepositIdx, DepositInfo)],
        requests: &[DbWithdrawalRequestRow],
    ) -> Vec<WithdrawalPairing> {
        self.state()
            .advance_withdrawal_pairings(self.status_db(), deposit_indices, deposit_infos, requests)
            .await
            .expect("advance withdrawal pairings")
    }

    /// Seeds deposit 0 and its completed withdrawal through production transitions.
    pub(crate) async fn seed_completed_withdrawal(&self, confirmations: u64) -> WithdrawalInfo {
        let request = make_withdrawal_request_row(0, 1);
        let info = make_withdrawal_info(1, WithdrawalStatus::Complete);
        self.advance_withdrawal_pairings(
            &[0],
            &[(0, make_deposit_info(DepositStatus::Complete))],
            &[request],
        )
        .await;
        self.apply_withdrawal_update(0, info, Some(confirmations))
            .await;
        info
    }
}

pub(crate) fn make_retry_config() -> RetryConfig {
    make_bridge_config(LOCAL_ESPLORA_URL, &[], Some(3)).retry_policy()
}

pub(crate) fn make_deposit_info(status: DepositStatus) -> DepositInfo {
    DepositInfo {
        deposit_request_txid: Txid::from_byte_array([1; 32]),
        deposit_txid: matches!(status, DepositStatus::Complete)
            .then_some(Txid::from_byte_array([2; 32])),
        status,
    }
}

pub(crate) fn make_withdrawal_info(seed: u8, status: WithdrawalStatus) -> WithdrawalInfo {
    WithdrawalInfo {
        withdrawal_request_txid: make_withdrawal_request(seed).tx_hash,
        fulfillment_txid: matches!(status, WithdrawalStatus::Complete)
            .then_some(Txid::from_byte_array([seed.wrapping_add(1); 32])),
        status,
    }
}

pub(crate) fn make_withdrawal_request_row(seq: WithdrawalSeq, seed: u8) -> DbWithdrawalRequestRow {
    DbWithdrawalRequestRow {
        seq,
        request: make_withdrawal_request(seed),
    }
}
