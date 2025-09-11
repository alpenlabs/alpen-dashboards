use serde::{Deserialize, Serialize};
use std::sync::{atomic::AtomicBool, Arc};
use tokio::sync::{Notify, RwLock};

use crate::config::NetworkMonitoringConfig;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Status {
    Online,
    Offline,
}

#[derive(Serialize, Clone, Debug)]
pub(crate) struct NetworkStatus {
    pub(crate) sequencer: Status,
    pub(crate) rpc_endpoint: Status,
    pub(crate) bundler_endpoint: Status,
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

/// Network monitoring context
pub(crate) struct NetworkMonitoringContext {
    pub(crate) network_status: Arc<RwLock<NetworkStatus>>,
    pub(crate) config: NetworkMonitoringConfig,
    pub(crate) status_available: Arc<AtomicBool>,
    pub(crate) initial_status_query_complete: Arc<Notify>,
}

impl NetworkMonitoringContext {
    pub(crate) fn new(config: NetworkMonitoringConfig) -> Self {
        Self {
            network_status: Arc::new(RwLock::new(NetworkStatus::default())),
            config,
            status_available: Arc::new(AtomicBool::new(false)),
            initial_status_query_complete: Arc::new(Notify::new()),
        }
    }
}
