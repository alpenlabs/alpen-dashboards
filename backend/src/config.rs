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
    pub(crate) fn new() -> Self {
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
    pub(crate) fn new() -> Self {
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
    pub(crate) fn bridge_rpc_urls(&self) -> &HashMap<String, String> {
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

/// Configuration for faucet balance monitoring
#[derive(Debug, Clone)]
pub(crate) struct FaucetBalanceConfig {
    /// L1 faucet URL
    l1_url: String,
    /// L2 faucet URL  
    l2_url: String,
}

impl FaucetBalanceConfig {
    /// Getter for L1 faucet URL
    pub(crate) fn l1_url(&self) -> &str {
        &self.l1_url
    }

    /// Getter for L2 faucet URL
    pub(crate) fn l2_url(&self) -> &str {
        &self.l2_url
    }
}

/// Configuration for bridge operator balance monitoring
#[derive(Debug, Clone)]
pub(crate) struct BridgeOperatorConfig {
    /// Esplora API URL
    esplora_url: String,
    /// General wallet addresses keyed by public key
    general_addresses: HashMap<String, String>,
    /// Stake chain wallet addresses keyed by public key
    stake_chain_addresses: HashMap<String, String>,
}

impl BridgeOperatorConfig {
    /// Getter for Esplora URL
    pub(crate) fn esplora_url(&self) -> &str {
        &self.esplora_url
    }

    /// Getter for general addresses
    pub(crate) fn general_addresses(&self) -> &HashMap<String, String> {
        &self.general_addresses
    }

    /// Getter for stake chain addresses
    pub(crate) fn stake_chain_addresses(&self) -> &HashMap<String, String> {
        &self.stake_chain_addresses
    }
}

/// Unified configuration for all balance monitoring
#[derive(Debug, Clone)]
pub(crate) struct BalanceMonitoringConfig {
    /// Faucet balance monitoring configuration
    faucet: FaucetBalanceConfig,
    /// Bridge operator balance monitoring configuration
    bridge_operators: BridgeOperatorConfig,
    /// Refresh interval in seconds
    refresh_interval_s: u64,
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
            .unwrap_or(3);

        let mut general_addresses = HashMap::new();
        let mut stake_chain_addresses = HashMap::new();

        for i in 1..=bridge_operators_count {
            if let Ok(operator_pk) = std::env::var(format!("STRATA_BRIDGE_{i}_PUBLIC_KEY")) {
                if let Ok(general_addr) =
                    std::env::var(format!("STRATA_BRIDGE_{i}_GENERAL_WALLET_ADDRESS"))
                {
                    general_addresses.insert(operator_pk.clone(), general_addr);
                }
                if let Ok(stake_addr) =
                    std::env::var(format!("STRATA_BRIDGE_{i}_STAKE_CHAIN_WALLET_ADDRESS"))
                {
                    stake_chain_addresses.insert(operator_pk, stake_addr);
                }
            }
        }

        info!(
            faucet_l1_url = %faucet_l1_url,
            faucet_l2_url = %faucet_l2_url,
            esplora_url = %esplora_url,
            general_addresses = ?general_addresses,
            stake_addresses = ?stake_chain_addresses,
            refresh_interval = refresh_interval_s,
            "Loaded balance monitoring config"
        );

        Self {
            faucet: FaucetBalanceConfig {
                l1_url: faucet_l1_url,
                l2_url: faucet_l2_url,
            },
            bridge_operators: BridgeOperatorConfig {
                esplora_url,
                general_addresses,
                stake_chain_addresses,
            },
            refresh_interval_s,
        }
    }

    /// Getter for faucet configuration
    pub(crate) fn faucet(&self) -> &FaucetBalanceConfig {
        &self.faucet
    }

    /// Getter for bridge operators configuration
    pub(crate) fn bridge_operators(&self) -> &BridgeOperatorConfig {
        &self.bridge_operators
    }

    /// Getter for refresh interval
    pub(crate) fn refresh_interval_s(&self) -> u64 {
        self.refresh_interval_s
    }
}
