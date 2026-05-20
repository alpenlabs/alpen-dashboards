use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{Notify, RwLock};
use tracing::debug;

use status_config::NetworkMonitoringConfig;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Status {
    Online,
    Offline,
}

#[derive(Serialize, Clone, Debug)]
pub struct NetworkStatus {
    sequencer: Status,
    rpc_endpoint: Status,
    bundler_endpoint: Status,
}

impl Default for NetworkStatus {
    fn default() -> Self {
        Self {
            sequencer: Status::Offline,
            rpc_endpoint: Status::Offline,
            bundler_endpoint: Status::Offline,
        }
    }
}

impl NetworkStatus {
    pub(crate) fn new(sequencer: Status, rpc_endpoint: Status, bundler_endpoint: Status) -> Self {
        Self {
            sequencer,
            rpc_endpoint,
            bundler_endpoint,
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

    pub(crate) async fn wait_until_status_available(&self) {
        if !self.status_available.load(Ordering::Acquire) {
            debug!("Waiting for initial network status query to complete");
            self.initial_status_query_complete.notified().await;
        }
    }
}
