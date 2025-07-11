use dotenvy::dotenv;
use tracing::info;

#[derive(Debug, Clone)]
pub(crate) struct NetworkConfig {
    /// JSON-RPC Endpoint for Alpen sequencer
    sequencer_url: String,

    /// JSON-RPC Endpoint for Alpen client
    rpc_url: String,

    /// Bundler health check URL (overrides `.env`)
    bundler_url: String,

    /// Max retries in querying status
    max_retries: u64,

    /// Total time in seconds to spend retrying
    total_retry_time: u64,
}

impl NetworkConfig {
    pub fn new() -> Self {
        dotenv().ok(); // Load `.env` file if present

        let sequencer_url = std::env::var("STRATA_SEQUENCER_URL")
            .ok()
            .unwrap_or_else(|| "http://localhost:8432".to_string());

        let rpc_url = std::env::var("STRATA_RPC_URL")
            .ok()
            .unwrap_or_else(|| "http://localhost:8433".to_string());

        let bundler_url = std::env::var("BUNDLER_URL")
            .ok()
            .unwrap_or_else(|| "http://localhost:8434".to_string());

        let max_retries: u64 = std::env::var("MAX_STATUS_RETRIES")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(5);

        let total_retry_time: u64 = std::env::var("TOTAL_RETRY_TIME")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(60);

        info!(%rpc_url, bundler_url, "Loaded Network monitoring config:");

        NetworkConfig {
            sequencer_url,
            rpc_url,
            bundler_url,
            max_retries,
            total_retry_time,
        }
    }

    /// Getter for `sequencer_url`
    pub fn sequencer_url(&self) -> &str {
        &self.sequencer_url
    }

    /// Getter for `rpc_url`
    pub fn rpc_url(&self) -> &str {
        &self.rpc_url
    }

    /// Getter for `bundler_url`
    pub fn bundler_url(&self) -> &str {
        &self.bundler_url
    }

    /// Getter for `max_retries`
    pub fn max_retries(&self) -> u64 {
        self.max_retries
    }

    /// Getter for `total_retry_time`
    pub fn total_retry_time(&self) -> u64 {
        self.total_retry_time
    }
}

/// Default bridge status refetch interval in seconds
const DEFAULT_BRIDGE_STATUS_REFETCH_INTERVAL_S: u64 = 120;

/// Bridge monitoring configuration
pub struct BridgeMonitoringConfig {
    /// Alpen bridge RPC url
    bridge_rpc_url: String,
    /// Bridge status refetch interval in seconds
    status_refetch_interval_s: u64,
}

impl BridgeMonitoringConfig {
    pub fn new() -> Self {
        dotenv().ok(); // Load `.env` file if present

        let bridge_rpc_url = std::env::var("ALPEN_BRIDGE_RPC_URL")
            .ok()
            .unwrap_or_else(|| "http://localhost:8546".to_string());

        let refresh_interval_s: u64 = std::env::var("BRIDGE_STATUS_REFETCH_INTERVAL_S")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(DEFAULT_BRIDGE_STATUS_REFETCH_INTERVAL_S);

        info!(%bridge_rpc_url, "Loaded Bridge monitoring config:");

        BridgeMonitoringConfig {
            bridge_rpc_url,
            status_refetch_interval_s: refresh_interval_s,
        }
    }

    /// Getter for `bridge_rpc_url`
    pub fn bridge_rpc_url(&self) -> &str {
        &self.bridge_rpc_url
    }

    /// Getter for `status_refetch_interval_s`
    pub fn status_refetch_interval(&self) -> u64 {
        self.status_refetch_interval_s
    }
}
