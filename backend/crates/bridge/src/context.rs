use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::{Notify, RwLock};
use tracing::debug;

use super::{cache::BridgeStatusCache, types::BridgeStatus};
use status_config::BridgeMonitoringConfig;

/// Bridge monitoring task context.
pub struct BridgeMonitoringContext {
    config: BridgeMonitoringConfig,
    state: RwLock<BridgeStatusCache>,
    status_available: AtomicBool,
    initial_status_query_complete: Notify,
}

impl BridgeMonitoringContext {
    pub fn new(config: BridgeMonitoringConfig) -> Self {
        Self {
            config,
            state: RwLock::new(BridgeStatusCache::default()),
            status_available: AtomicBool::new(false),
            initial_status_query_complete: Notify::new(),
        }
    }

    pub(crate) fn config(&self) -> &BridgeMonitoringConfig {
        &self.config
    }

    pub(crate) async fn with_state<T>(&self, f: impl FnOnce(&BridgeStatusCache) -> T) -> T {
        let state = self.state.read().await;
        f(&state)
    }

    pub(crate) async fn with_state_mut<T>(&self, f: impl FnOnce(&mut BridgeStatusCache) -> T) -> T {
        let mut state = self.state.write().await;
        f(&mut state)
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
        let cache = self.state.read().await;

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

#[cfg(test)]
mod tests {
    use bitcoin::Txid;
    use std::str::FromStr;

    use super::*;
    use crate::types::{DepositInfo, DepositStatus};

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
        let context = BridgeMonitoringContext::new(config);
        let deposit_request_txid =
            Txid::from_str("0101010101010101010101010101010101010101010101010101010101010101")
                .expect("valid txid");
        let deposit_txid =
            Txid::from_str("0202020202020202020202020202020202020202020202020202020202020202")
                .expect("valid txid");

        context
            .with_state_mut(|state| {
                state.update_deposit(
                    deposit_request_txid,
                    DepositInfo {
                        deposit_request_txid,
                        deposit_txid: Some(deposit_txid),
                        status: DepositStatus::Complete,
                    },
                    1,
                );
            })
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
