use serde::{Deserialize, Serialize};

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
