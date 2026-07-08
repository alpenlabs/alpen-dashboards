use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{Notify, RwLock};
use tokio::time::Duration;

use status_config::NetworkMonitoringConfig;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Status {
    Online,
    Offline,
}

#[derive(Serialize, Clone, Debug)]
pub struct NetworkStatus {
    sequencer: Status,
    rpc_endpoint: Status,
    ee_endpoint: Status,
    bundler_endpoint: Status,
    sequencer_chain: Option<OlChainStatus>,
    rpc_chain: Option<OlChainStatus>,
    ee_chain: Option<EvmChainStatus>,
}

impl Default for NetworkStatus {
    fn default() -> Self {
        Self {
            sequencer: Status::Offline,
            rpc_endpoint: Status::Offline,
            ee_endpoint: Status::Offline,
            bundler_endpoint: Status::Offline,
            sequencer_chain: None,
            rpc_chain: None,
            ee_chain: None,
        }
    }
}

impl NetworkStatus {
    pub(crate) fn new(
        sequencer: Status,
        rpc_endpoint: Status,
        ee_endpoint: Status,
        bundler_endpoint: Status,
        sequencer_chain: Option<OlChainStatus>,
        rpc_chain: Option<OlChainStatus>,
        ee_chain: Option<EvmChainStatus>,
    ) -> Self {
        Self {
            sequencer,
            rpc_endpoint,
            ee_endpoint,
            bundler_endpoint,
            sequencer_chain,
            rpc_chain,
            ee_chain,
        }
    }
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct BlockInfoStatus {
    slot: u64,
    block_id: String,
    epoch: u32,
    is_terminal: bool,
}

impl BlockInfoStatus {
    pub(crate) fn new(slot: u64, block_id: String, epoch: u32, is_terminal: bool) -> Self {
        Self {
            slot,
            block_id,
            epoch,
            is_terminal,
        }
    }

    pub(crate) fn slot(&self) -> u64 {
        self.slot
    }
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct EpochCommitmentStatus {
    epoch: u32,
    last_slot: u64,
    last_block_id: String,
}

impl EpochCommitmentStatus {
    pub(crate) fn new(epoch: u32, last_slot: u64, last_block_id: String) -> Self {
        Self {
            epoch,
            last_slot,
            last_block_id,
        }
    }

    pub(crate) fn last_slot(&self) -> u64 {
        self.last_slot
    }
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct OlChainStatus {
    tip: BlockInfoStatus,
    latest: EpochCommitmentStatus,
    confirmed: EpochCommitmentStatus,
    finalized: EpochCommitmentStatus,
    confirmation_lag_slots: u64,
    finality_lag_slots: u64,
    latest_slot_stale_seconds: Option<u64>,
}

impl OlChainStatus {
    pub(crate) fn new(
        tip: BlockInfoStatus,
        latest: EpochCommitmentStatus,
        confirmed: EpochCommitmentStatus,
        finalized: EpochCommitmentStatus,
    ) -> Self {
        let latest_slot = tip.slot();
        let confirmed_slot = confirmed.last_slot();
        let finalized_slot = finalized.last_slot();

        Self {
            tip,
            latest,
            confirmed,
            finalized,
            confirmation_lag_slots: latest_slot.saturating_sub(confirmed_slot),
            finality_lag_slots: latest_slot.saturating_sub(finalized_slot),
            latest_slot_stale_seconds: None,
        }
    }

    pub(crate) fn latest_slot(&self) -> u64 {
        self.tip.slot()
    }

    pub(crate) fn set_latest_slot_stale_seconds(&mut self, seconds: Option<u64>) {
        self.latest_slot_stale_seconds = seconds;
    }
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct EvmChainStatus {
    latest_block_number: u64,
    latest_block_stale_seconds: Option<u64>,
}

impl EvmChainStatus {
    pub(crate) fn new(latest_block_number: u64) -> Self {
        Self {
            latest_block_number,
            latest_block_stale_seconds: None,
        }
    }

    pub(crate) fn latest_block_number(&self) -> u64 {
        self.latest_block_number
    }

    pub(crate) fn set_latest_block_stale_seconds(&mut self, seconds: Option<u64>) {
        self.latest_block_stale_seconds = seconds;
    }
}

/// Network monitoring context
pub struct NetworkMonitoringContext {
    config: NetworkMonitoringConfig,
    status_available: AtomicBool,
    initial_status_query_complete: Notify,
    network_status: RwLock<NetworkStatus>,
}

impl NetworkMonitoringContext {
    pub fn new(config: NetworkMonitoringConfig) -> Self {
        Self {
            config,
            status_available: AtomicBool::new(false),
            initial_status_query_complete: Notify::new(),
            network_status: RwLock::new(NetworkStatus::default()),
        }
    }

    pub(crate) fn config(&self) -> &NetworkMonitoringConfig {
        &self.config
    }

    pub(crate) async fn set_status(&self, status: NetworkStatus) {
        let mut locked_status = self.network_status.write().await;
        *locked_status = status;
    }

    pub(crate) async fn status(&self) -> NetworkStatus {
        self.network_status.read().await.clone()
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> NetworkMonitoringConfig {
        toml::from_str(
            r#"
            sequencer_url = "http://localhost:8545"
            rpc_url = "http://localhost:8546"
            bundler_url = "http://localhost:3000/health"
            retry_policy_max_retries = 1
            retry_policy_total_time_s = 1
            status_refetch_interval_s = 1
            "#,
        )
        .expect("test config should deserialize")
    }

    #[tokio::test]
    async fn wait_for_initial_status_times_out_when_unavailable() {
        let context = NetworkMonitoringContext::new(test_config());

        assert!(tokio::time::timeout(
            Duration::from_millis(1),
            context.wait_until_initial_status()
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn wait_for_initial_status_returns_when_available() {
        let context = NetworkMonitoringContext::new(test_config());

        context.mark_status_available();

        assert!(tokio::time::timeout(
            Duration::from_millis(1),
            context.wait_until_initial_status()
        )
        .await
        .is_ok());
    }
}
