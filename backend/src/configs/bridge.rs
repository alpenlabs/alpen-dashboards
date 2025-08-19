use tracing::info;

use super::env::EnvVarParser;

/// Default bridge status refetch interval in seconds
const DEFAULT_BRIDGE_STATUS_REFETCH_INTERVAL_S: u64 = 120;

/// Default maximum number of confirmations for transactions tracked
const DEFAULT_MAX_TX_CONFIRMATIONS: u64 = 6;

/// Default number of bridge operators
const DEFAULT_BRIDGE_OPERATORS_COUNT: u64 = 3;

/// Bridge monitoring configuration
#[derive(Clone)]
pub struct BridgeMonitoringConfig {
    /// Strata bridge RPC urls
    bridge_rpc_urls: Vec<(String, String)>,

    /// Esplora URL
    esplora_url: String,

    /// Maximum confirmations
    max_tx_confirmations: u64,

    /// Bridge status refetch interval in seconds
    status_refetch_interval_s: u64,
}

impl BridgeMonitoringConfig {
    pub(crate) fn new() -> Self {
        let bridge_operators_count = u64::parse_env_var("STRATA_BRIDGE_OPERATORS_COUNT")
            .unwrap_or(DEFAULT_BRIDGE_OPERATORS_COUNT);

        let mut bridge_rpc_urls = Vec::new();
        for i in 1..=bridge_operators_count {
            let operator_pk = String::parse_env_var(&format!("STRATA_BRIDGE_{i}_PUBLIC_KEY"))
                .expect("valid public key");
            let rpc_url = String::parse_env_var(&format!("STRATA_BRIDGE_{i}_RPC_URL"))
                .unwrap_or_else(|| format!("http://localhost:{}", 8545 + i));
            bridge_rpc_urls.push((operator_pk, rpc_url));
        }

        let esplora_url = String::parse_env_var("ESPLORA_URL")
            .unwrap_or_else(|| "http://localhost:8545".to_string());

        let max_tx_confirmations = u64::parse_env_var("BRIDGE_TX_MAX_CONFIRMATIONS")
            .unwrap_or(DEFAULT_MAX_TX_CONFIRMATIONS);

        let refresh_interval_s = u64::parse_env_var("BRIDGE_STATUS_REFETCH_INTERVAL_S")
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
    pub(crate) fn bridge_rpc_urls(&self) -> &Vec<(String, String)> {
        &self.bridge_rpc_urls
    }

    /// Getter for `esplora_url`
    pub(crate) fn esplora_url(&self) -> &str {
        &self.esplora_url
    }

    /// Getter for `max_tx_confirmations`
    pub(crate) fn max_tx_confirmations(&self) -> u64 {
        self.max_tx_confirmations
    }

    /// Getter for `status_refetch_interval_s`
    pub(crate) fn status_refetch_interval(&self) -> u64 {
        self.status_refetch_interval_s
    }
}
