use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use tokio::sync::Notify;
use tracing::debug;

use super::{
    bridge_rpc::RpcClientManager, db::WithdrawalIndexerDbSled, esplora::EsploraClient,
    state::BridgeMonitoringState, types::BridgeStatus,
};
use status_config::BridgeMonitoringConfig;

/// Bridge monitoring task context.
pub struct BridgeMonitoringContext {
    config: BridgeMonitoringConfig,
    bridge_rpc: RpcClientManager,
    esplora_client: EsploraClient,
    withdrawal_index: Arc<WithdrawalIndexerDbSled>,
    state: BridgeMonitoringState,
    status_available: AtomicBool,
    initial_status_query_complete: Notify,
}

impl BridgeMonitoringContext {
    pub fn new(
        config: BridgeMonitoringConfig,
        withdrawal_index: Arc<WithdrawalIndexerDbSled>,
    ) -> Self {
        let bridge_rpc = RpcClientManager::new(&config);
        let esplora_client =
            EsploraClient::new(config.esplora_url(), config.esplora_request_timeout_s());

        Self {
            config,
            bridge_rpc,
            esplora_client,
            withdrawal_index,
            state: BridgeMonitoringState::default(),
            status_available: AtomicBool::new(false),
            initial_status_query_complete: Notify::new(),
        }
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

    pub(crate) async fn wait_until_status_available(&self) {
        if !self.status_available.load(Ordering::Acquire) {
            debug!("Waiting for initial bridge status query to complete");
            self.initial_status_query_complete.notified().await;
        }
    }

    pub(crate) async fn bridge_status(&self) -> BridgeStatus {
        self.state.bridge_status().await
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

    #[tokio::test]
    async fn bridge_status_reflects_cached_state() {
        let config: BridgeMonitoringConfig = toml::from_str(
            r#"
            esplora_url = "http://localhost:3000"
            max_tx_confirmations = 6
            status_refetch_interval_s = 1
            operators = []
            "#,
        )
        .expect("test config should deserialize");
        let withdrawal_index =
            Arc::new(WithdrawalIndexerDbSled::open_temporary().expect("open db"));
        let context = BridgeMonitoringContext::new(config, withdrawal_index);
        let deposit_request_txid =
            Txid::from_str("0101010101010101010101010101010101010101010101010101010101010101")
                .expect("valid txid");
        let deposit_txid =
            Txid::from_str("0202020202020202020202020202020202020202020202020202020202020202")
                .expect("valid txid");

        context
            .state()
            .apply_deposit_info_updates(
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
            .await;

        let status = context.bridge_status().await;

        assert_eq!(status.deposits.len(), 1);
        assert_eq!(
            status.deposits[0].deposit_request_txid,
            deposit_request_txid
        );
        assert_eq!(status.deposits[0].deposit_txid, Some(deposit_txid));
    }
}
