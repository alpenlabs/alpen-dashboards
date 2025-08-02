use dotenvy::dotenv;
use std::collections::HashMap;
use tracing::info;

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

/// Default maximum number of confirmations for transactions tracked
const DEFAULT_MAX_TX_CONFIRMATIONS: u64 = 6;

/// Default number of bridge operators
const DEFAULT_BRIDGE_OPERATORS_COUNT: u64 = 3;

/// Bridge monitoring configuration
#[derive(Clone)]
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

/// Balance monitoring configuration
#[derive(Debug, Clone)]
pub(crate) struct BalanceMonitoringConfig {
    /// Faucet L1 balance URL
    pub(crate) faucet_l1_url: String,
    /// Faucet L2 balance URL
    pub(crate) faucet_l2_url: String,
    /// Esplora URL for Bitcoin address balance queries
    pub(crate) esplora_url: String,
    /// Bridge operator general wallet addresses keyed by public key
    pub(crate) bridge_operator_general_addresses: HashMap<String, String>,
    /// Bridge operator stake chain wallet addresses keyed by public key
    pub(crate) bridge_operator_stake_addresses: HashMap<String, String>,
    /// Refresh interval in seconds
    pub(crate) refresh_interval_s: u64,
}

impl BalanceMonitoringConfig {
    pub(crate) fn new() -> Self {
        dotenv().ok(); // Load `.env` file if present

        let refresh_interval_s: u64 = std::env::var("BALANCE_MONITORING_REFRESH_INTERVAL_S")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(300); // Default to 5 minutes

        let faucet_l1_url = std::env::var("FAUCET_L1_BALANCE_URL")
            .unwrap_or_else(|_| "http://localhost:8080/balance/l1".to_string());

        let faucet_l2_url = std::env::var("FAUCET_L2_BALANCE_URL")
            .unwrap_or_else(|_| "http://localhost:8080/balance/l2".to_string());

        let esplora_url = std::env::var("ESPLORA_URL")
            .unwrap_or_else(|_| "https://bitcoin.testnet.alpenlabs.io".to_string());

        // Load bridge operator addresses from environment
        let bridge_operators_count = std::env::var("STRATA_BRIDGE_OPERATORS_COUNT")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(DEFAULT_BRIDGE_OPERATORS_COUNT);

        let mut bridge_operator_general_addresses = HashMap::new();
        let mut bridge_operator_stake_addresses = HashMap::new();

        for i in 1..=bridge_operators_count {
            if let Ok(operator_pk) = std::env::var(format!("STRATA_BRIDGE_{i}_PUBLIC_KEY")) {
                if let Ok(general_addr) =
                    std::env::var(format!("STRATA_BRIDGE_{i}_GENERAL_WALLET_ADDRESS"))
                {
                    bridge_operator_general_addresses.insert(operator_pk.clone(), general_addr);
                }
                if let Ok(stake_addr) =
                    std::env::var(format!("STRATA_BRIDGE_{i}_STAKE_CHAIN_WALLET_ADDRESS"))
                {
                    bridge_operator_stake_addresses.insert(operator_pk, stake_addr);
                }
            }
        }

        info!(
            faucet_l1_url = %faucet_l1_url,
            faucet_l2_url = %faucet_l2_url,
            esplora_url = %esplora_url,
            general_addresses = bridge_operator_general_addresses.len(),
            stake_addresses = bridge_operator_stake_addresses.len(),
            refresh_interval = refresh_interval_s,
            "Loaded balance monitoring config"
        );

        BalanceMonitoringConfig {
            faucet_l1_url,
            faucet_l2_url,
            esplora_url,
            bridge_operator_general_addresses,
            bridge_operator_stake_addresses,
            refresh_interval_s,
        }
    }

    /// Getter for faucet L1 URL
    pub(crate) fn faucet_l1_url(&self) -> &str {
        &self.faucet_l1_url
    }

    /// Getter for faucet L2 URL
    pub(crate) fn faucet_l2_url(&self) -> &str {
        &self.faucet_l2_url
    }

    /// Getter for Esplora URL
    pub(crate) fn esplora_url(&self) -> &str {
        &self.esplora_url
    }

    /// Getter for bridge operator general addresses
    pub(crate) fn bridge_operator_general_addresses(&self) -> &HashMap<String, String> {
        &self.bridge_operator_general_addresses
    }

    /// Getter for bridge operator stake addresses
    pub(crate) fn bridge_operator_stake_addresses(&self) -> &HashMap<String, String> {
        &self.bridge_operator_stake_addresses
    }

    /// Getter for refresh interval
    pub(crate) fn refresh_interval_s(&self) -> u64 {
        self.refresh_interval_s
    }
}
