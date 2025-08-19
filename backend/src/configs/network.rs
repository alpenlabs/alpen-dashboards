use tracing::info;

use super::env::EnvVarParser;

/// Default network status refetch interval in seconds
const DEFAULT_NETWORK_STATUS_REFETCH_INTERVAL_S: u64 = 10;

#[derive(Debug, Clone)]
pub(crate) struct NetworkConfig {
    /// JSON-RPC Endpoint for Strata sequencer
    sequencer_url: String,

    /// JSON-RPC Endpoint for Strata client and reth
    rpc_url: String,

    /// Bundler health check URL (overrides `.env`)
    bundler_url: String,

    /// Max retries in querying status
    max_retries: u64,

    /// Total time in seconds to spend retrying
    total_retry_time: u64,

    /// Network status refetch interval in seconds
    status_refetch_interval_s: u64,
}

impl NetworkConfig {
    pub(crate) fn new() -> Self {
        let sequencer_url = String::parse_env_var("STRATA_SEQUENCER_URL")
            .unwrap_or_else(|| "http://localhost:8432".to_string());

        let rpc_url =
            String::parse_env_var("RPC_URL").unwrap_or_else(|| "http://localhost:8433".to_string());

        let bundler_url = String::parse_env_var("BUNDLER_URL")
            .unwrap_or_else(|| "http://localhost:8434".to_string());

        let max_retries = u64::parse_env_var("MAX_STATUS_RETRIES").unwrap_or(5);

        let total_retry_time = u64::parse_env_var("TOTAL_RETRY_TIME").unwrap_or(60);

        let status_refetch_interval_s = u64::parse_env_var("NETWORK_STATUS_REFETCH_INTERVAL_S")
            .unwrap_or(DEFAULT_NETWORK_STATUS_REFETCH_INTERVAL_S);

        info!(%rpc_url, bundler_url, "Loaded Network monitoring config:");

        NetworkConfig {
            sequencer_url,
            rpc_url,
            bundler_url,
            max_retries,
            total_retry_time,
            status_refetch_interval_s,
        }
    }

    /// Getter for `sequencer_url`
    pub(crate) fn sequencer_url(&self) -> &str {
        &self.sequencer_url
    }

    /// Getter for `rpc_url`
    pub(crate) fn rpc_url(&self) -> &str {
        &self.rpc_url
    }

    /// Getter for `bundler_url`
    pub(crate) fn bundler_url(&self) -> &str {
        &self.bundler_url
    }

    /// Getter for `max_retries`
    pub(crate) fn max_retries(&self) -> u64 {
        self.max_retries
    }

    /// Getter for `total_retry_time`
    pub(crate) fn total_retry_time(&self) -> u64 {
        self.total_retry_time
    }

    /// Getter for `status_refetch_interval_s`
    pub(crate) fn status_refetch_interval(&self) -> u64 {
        self.status_refetch_interval_s
    }
}
