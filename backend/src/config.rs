use dotenvy::dotenv;
use std::collections::HashMap;
use tracing::info;

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
    pub fn new() -> Self {
        dotenv().ok(); // Load `.env` file if present

        let sequencer_url = std::env::var("STRATA_SEQUENCER_URL")
            .ok()
            .unwrap_or_else(|| "http://localhost:8432".to_string());

        let rpc_url = std::env::var("RPC_URL")
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

        let status_refetch_interval_s: u64 = std::env::var("NETWORK_STATUS_REFETCH_INTERVAL_S")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
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

    /// Getter for `status_refetch_interval_s`
    pub fn status_refetch_interval(&self) -> u64 {
        self.status_refetch_interval_s
    }
}

/// Default bridge status refetch interval in seconds
const DEFAULT_BRIDGE_STATUS_REFETCH_INTERVAL_S: u64 = 120;

/// Default maximum number of confirmations for transactions tracked
const DEFAULT_MAX_TX_CONFIRMATIONS: u64 = 6;

/// Default number of bridge operators
const DEFAULT_BRIDGE_OPERATORS_COUNT: u64 = 3;

/// Bridge monitoring configuration
pub struct BridgeMonitoringConfig {
    /// Strata bridge RPC urls
    bridge_rpc_urls: HashMap<String, String>,

    /// Esplora URL
    esplora_url: String,

    /// Maximum confirmations
    max_tx_confirmations: u64,

    /// Bridge status refetch interval in seconds
    status_refetch_interval_s: u64,
}

impl BridgeMonitoringConfig {
    pub fn new() -> Self {
        dotenv().ok(); // Load `.env` file if present

        let bridge_operators_count = std::env::var("STRATA_BRIDGE_OPERATORS_COUNT")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(DEFAULT_BRIDGE_OPERATORS_COUNT);

        let mut bridge_rpc_urls = HashMap::new();
        for i in 1..=bridge_operators_count {
            let operator_pk =
                std::env::var(format!("STRATA_BRIDGE_{i}_PUBLIC_KEY")).expect("valid public key");
            let rpc_url = std::env::var(format!("STRATA_BRIDGE_{i}_RPC_URL"))
                .ok()
                .unwrap_or_else(|| format!("http://localhost:{}", 8545 + i));
            bridge_rpc_urls.insert(operator_pk, rpc_url);
        }

        let esplora_url = std::env::var("ESPLORA_URL")
            .ok()
            .unwrap_or_else(|| "http://localhost:8545".to_string());

        let max_tx_confirmations: u64 = std::env::var("BRIDGE_TX_MAX_CONFIRMATIONS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(DEFAULT_MAX_TX_CONFIRMATIONS);

        let refresh_interval_s: u64 = std::env::var("BRIDGE_STATUS_REFETCH_INTERVAL_S")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(DEFAULT_BRIDGE_STATUS_REFETCH_INTERVAL_S);

        info!(?bridge_rpc_urls, %esplora_url, "Loaded Bridge monitoring config:");

        BridgeMonitoringConfig {
            bridge_rpc_urls,
            esplora_url,
            max_tx_confirmations,
            status_refetch_interval_s: refresh_interval_s,
        }
    }

    /// Getter for `bridge_rpc_urls`
    pub fn bridge_rpc_urls(&self) -> &HashMap<String, String> {
        &self.bridge_rpc_urls
    }

    /// Getter for `esplora_url`
    pub fn esplora_url(&self) -> &str {
        &self.esplora_url
    }

    /// Getter for `max_tx_confirmations`
    pub fn max_tx_confirmations(&self) -> u64 {
        self.max_tx_confirmations
    }

    /// Getter for `status_refetch_interval_s`
    pub fn status_refetch_interval(&self) -> u64 {
        self.status_refetch_interval_s
    }
}
