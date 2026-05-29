use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use tokio::sync::Notify;
use tokio::time::Duration;

use super::{
    bridge_rpc::RpcClientManager,
    db::{traits::BridgeStatusDb, BridgeStatusDbSled, WithdrawalIndexerDbSled},
    esplora::EsploraClient,
    state::BridgeMonitoringState,
    types::BridgeStatus,
};
use status_config::BridgeMonitoringConfig;

/// Bridge monitoring task context.
pub struct BridgeMonitoringContext {
    config: BridgeMonitoringConfig,
    bridge_rpc: RpcClientManager,
    esplora_client: EsploraClient,
    withdrawal_index: Arc<WithdrawalIndexerDbSled>,
    status_db: Arc<BridgeStatusDbSled>,
    state: BridgeMonitoringState,
    status_available: AtomicBool,
    initial_status_query_complete: Notify,
}

impl BridgeMonitoringContext {
    pub fn new(
        config: BridgeMonitoringConfig,
        withdrawal_index: Arc<WithdrawalIndexerDbSled>,
        status_db: Arc<BridgeStatusDbSled>,
    ) -> anyhow::Result<Self> {
        let bridge_rpc = RpcClientManager::new(&config);
        let esplora_client =
            EsploraClient::new(config.esplora_url(), config.esplora_request_timeout_s());
        let snapshot = status_db
            .get_status_snapshot()
            .map_err(|e| anyhow::anyhow!("hydrate bridge status state: {e}"))?;
        let state = BridgeMonitoringState::from_snapshot(snapshot);

        Ok(Self {
            config,
            bridge_rpc,
            esplora_client,
            withdrawal_index,
            status_db,
            state,
            status_available: AtomicBool::new(false),
            initial_status_query_complete: Notify::new(),
        })
    }

    pub(crate) fn config(&self) -> &BridgeMonitoringConfig {
        &self.config
    }

    pub(crate) fn bridge_rpc(&self) -> &RpcClientManager {
        &self.bridge_rpc
    }

    pub(crate) fn esplora(&self) -> &EsploraClient {
        &self.esplora_client
    }

    pub(crate) fn withdrawal_index(&self) -> &WithdrawalIndexerDbSled {
        self.withdrawal_index.as_ref()
    }

    pub(crate) fn status_db(&self) -> &BridgeStatusDbSled {
        self.status_db.as_ref()
    }

    pub(crate) fn state(&self) -> &BridgeMonitoringState {
        &self.state
    }

    pub(crate) fn mark_status_available(&self) {
        if self
            .status_available
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            self.initial_status_query_complete.notify_waiters();
        }
    }

    pub(crate) fn initial_status_wait_timeout(&self) -> Duration {
        Duration::from_secs(self.config.initial_status_wait_timeout_s().max(1))
    }

    pub(crate) async fn wait_until_initial_status(&self) {
        if self.status_available.load(Ordering::Acquire) {
            return;
        }

        let notified = self.initial_status_query_complete.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();

        if self.status_available.load(Ordering::Acquire) {
            return;
        }

        notified.await;
    }

    pub(crate) async fn bridge_status(&self) -> BridgeStatus {
        self.state
            .bridge_status(self.config.max_tx_confirmations())
            .await
    }
}

#[cfg(test)]
mod tests {
    use bitcoin::Txid;
    use std::str::FromStr;

    use super::*;
    use crate::{
        state::DepositInfoUpdate,
        types::{DepositInfo, DepositStatus},
    };

    fn test_config() -> BridgeMonitoringConfig {
        toml::from_str(
            r#"
            esplora_url = "http://localhost:3000"
            max_tx_confirmations = 6
            status_refetch_interval_s = 1
            operators = []
            "#,
        )
        .expect("test config should deserialize")
    }

    fn test_context() -> BridgeMonitoringContext {
        let withdrawal_index =
            Arc::new(WithdrawalIndexerDbSled::open_temporary().expect("open db"));
        let status_db = Arc::new(BridgeStatusDbSled::open_temporary().expect("open status db"));
        BridgeMonitoringContext::new(test_config(), withdrawal_index, status_db)
            .expect("create bridge monitoring context")
    }

    #[tokio::test]
    async fn bridge_status_reflects_cached_state() {
        let context = test_context();
        let deposit_request_txid =
            Txid::from_str("0101010101010101010101010101010101010101010101010101010101010101")
                .expect("valid txid");
        let deposit_txid =
            Txid::from_str("0202020202020202020202020202020202020202020202020202020202020202")
                .expect("valid txid");

        context
            .state()
            .apply_deposit_info_updates(
                context.status_db(),
                vec![DepositInfoUpdate {
                    deposit_idx: 0,
                    info: DepositInfo {
                        deposit_request_txid,
                        deposit_txid: Some(deposit_txid),
                        status: DepositStatus::Complete,
                    },
                    confirmations: Some(1),
                }],
                6,
            )
            .await
            .expect("apply deposit update");

        let status = context.bridge_status().await;

        assert_eq!(status.deposits.len(), 1);
        assert_eq!(
            status.deposits[0].deposit_request_txid,
            deposit_request_txid
        );
        assert_eq!(status.deposits[0].deposit_txid, Some(deposit_txid));
    }

    #[tokio::test]
    async fn wait_for_initial_status_times_out_when_unavailable() {
        let context = test_context();

        assert!(tokio::time::timeout(
            Duration::from_millis(1),
            context.wait_until_initial_status()
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn wait_for_initial_status_returns_when_available() {
        let context = test_context();

        context.mark_status_available();

        assert!(tokio::time::timeout(
            Duration::from_millis(1),
            context.wait_until_initial_status()
        )
        .await
        .is_ok());
    }
}
