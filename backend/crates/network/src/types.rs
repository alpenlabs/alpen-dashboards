use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{Notify, RwLock};
use tokio::time::Duration;

use status_config::NetworkMonitoringConfig;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Status {
    Online,
    Offline,
}

#[derive(Serialize, Clone, Debug)]
pub struct NetworkStatus {
    pub(crate) alpen_sequencer: Status,
    pub(crate) alpen_rpc: Status,
    pub(crate) alpen_bundler: Status,
    pub(crate) strata_sequencer: Status,
    pub(crate) strata_rpc: Status,
}

impl Default for NetworkStatus {
    fn default() -> Self {
        Self {
            alpen_sequencer: Status::Offline,
            alpen_rpc: Status::Offline,
            alpen_bundler: Status::Offline,
            strata_sequencer: Status::Offline,
            strata_rpc: Status::Offline,
        }
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
    use status_config::test_utils::{make_network_config, LOCAL_NETWORK_URLS};

    use super::*;

    #[tokio::test]
    async fn initial_status_wait_times_out_when_unavailable() {
        let context = NetworkMonitoringContext::new(make_network_config(&LOCAL_NETWORK_URLS, None));

        assert!(tokio::time::timeout(
            Duration::from_millis(1),
            context.wait_until_initial_status()
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn initial_status_wait_returns_when_available() {
        let context = NetworkMonitoringContext::new(make_network_config(&LOCAL_NETWORK_URLS, None));

        context.mark_status_available();

        assert!(tokio::time::timeout(
            Duration::from_millis(1),
            context.wait_until_initial_status()
        )
        .await
        .is_ok());
    }
}
