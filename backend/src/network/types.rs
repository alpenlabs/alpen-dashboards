//! Types to represent the health and connectivity status of Alpen network nodes and services.

use serde::{Deserialize, Serialize};

/// Represents the availability status of a network node or service.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Status {
    /// The component is online and reachable.
    Online,
    /// The component is offline or unresponsive.
    Offline,
}

/// Status of key components in the Alpen network.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub(crate) struct NetworkStatus {
    /// Status of batch producer.
    pub batch_producer: Status,

    /// Status of RPC endpoint.
    pub rpc_endpoint: Status,

    /// Status of bundler endpoint.
    pub bundler_endpoint: Status,
}

impl NetworkStatus {
    pub(crate) fn new(batch_producer: Status, rpc_endpoint: Status, bundler_endpoint: Status) -> Self {
        Self {
            batch_producer,
            rpc_endpoint,
            bundler_endpoint,
        }
    }
}

impl Default for NetworkStatus {
    fn default() -> Self {
        Self {
            batch_producer: Status::Offline,
            rpc_endpoint: Status::Offline,
            bundler_endpoint: Status::Offline,
        }
    }
}
