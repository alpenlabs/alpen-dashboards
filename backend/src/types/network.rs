use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Online,
    Offline,
}

#[derive(Serialize, Clone, Debug)]
pub struct NetworkStatus {
    batch_producer: Status,
    rpc_endpoint: Status,
    bundler_endpoint: Status,
}

impl NetworkStatus {
    pub fn default() -> Self {
        Self {
            batch_producer: Status::Offline,
            rpc_endpoint: Status::Offline,
            bundler_endpoint: Status::Offline,
        }
    }

    pub fn new(batch_producer: Status, rpc_endpoint: Status, bundler_endpoint: Status) -> Self {
        Self {
            batch_producer,
            rpc_endpoint,
            bundler_endpoint,
        }
    }
}
