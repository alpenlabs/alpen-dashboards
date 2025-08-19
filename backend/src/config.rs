use serde::{Deserialize, Serialize};
use std::path::Path;
use tracing::{debug, trace};

/// Main configuration struct containing all application settings
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct Config {
    /// API server configuration
    pub(crate) server: ApiServerConfig,
    /// Network monitoring configuration
    pub(crate) network: NetworkMonitoringConfig,
    /// Bridge monitoring configuration
    pub(crate) bridge: BridgeMonitoringConfig,
    /// Balance monitoring configuration
    pub(crate) balance: BalanceMonitoringConfig,
}

/// Configuration for the API server
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct ApiServerConfig {
    /// Host address to bind the server to
    pub(crate) host: String,
    /// Port number to bind the server to
    pub(crate) port: u16,
}

impl ApiServerConfig {
    /// Get a reference to the host address
    pub(crate) fn host(&self) -> &str {
        &self.host
    }

    /// Get the port number
    pub(crate) fn port(&self) -> u16 {
        self.port
    }
}

/// Configuration for network monitoring services
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct NetworkMonitoringConfig {
    /// JSON-RPC Endpoint for Strata sequencer
    pub(crate) sequencer_url: String,

    /// JSON-RPC Endpoint for Strata client and reth
    pub(crate) rpc_url: String,

    /// Bundler health check URL
    pub(crate) bundler_url: String,

    /// Max retries for status queries
    pub(crate) max_retries: u64,

    /// Total time in seconds to spend retrying status queries
    pub(crate) total_retry_time_s: u64,

    /// Network status refetch interval in seconds
    pub(crate) status_refetch_interval_s: u64,
}

impl NetworkMonitoringConfig {
    pub(crate) fn sequencer_url(&self) -> &str {
        &self.sequencer_url
    }

    pub(crate) fn rpc_url(&self) -> &str {
        &self.rpc_url
    }

    pub(crate) fn bundler_url(&self) -> &str {
        &self.bundler_url
    }

    pub(crate) fn max_retries(&self) -> u64 {
        self.max_retries
    }

    pub(crate) fn total_retry_time(&self) -> u64 {
        self.total_retry_time_s
    }

    pub(crate) fn status_refetch_interval(&self) -> u64 {
        self.status_refetch_interval_s
    }
}

/// Configuration for bridge monitoring services
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct BridgeMonitoringConfig {
    /// Esplora URL
    pub(crate) esplora_url: String,

    /// Maximum confirmations
    pub(crate) max_tx_confirmations: u64,

    /// Bridge status refetch interval in seconds
    pub(crate) status_refetch_interval_s: u64,

    /// Bridge operators
    pub(crate) operators: Vec<BridgeOperator>,
}

/// Configuration for a bridge operator
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct BridgeOperator {
    /// Public key of the bridge operator
    pub(crate) public_key: String,
    /// RPC URL for the bridge operator
    pub(crate) rpc_url: String,
}

impl BridgeOperator {
    pub(crate) fn public_key(&self) -> &str {
        &self.public_key
    }

    pub(crate) fn rpc_url(&self) -> &str {
        &self.rpc_url
    }
}

impl BridgeMonitoringConfig {
    pub(crate) fn esplora_url(&self) -> &str {
        &self.esplora_url
    }

    pub(crate) fn max_tx_confirmations(&self) -> u64 {
        self.max_tx_confirmations
    }

    pub(crate) fn status_refetch_interval(&self) -> u64 {
        self.status_refetch_interval_s
    }

    pub(crate) fn operators(&self) -> &Vec<BridgeOperator> {
        &self.operators
    }
}

/// Configuration for faucet balance monitoring
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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

/// Bridge operator configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct BridgeOperatorConfig {
    /// Esplora API URL
    esplora_url: String,
    /// General wallet addresses as vector of (public_key, address) tuples
    general_addresses: Vec<(String, String)>,
    /// Stake chain wallet addresses as vector of (public_key, address) tuples
    stake_chain_addresses: Vec<(String, String)>,
}

impl BridgeOperatorConfig {
    /// Getter for Esplora URL
    pub(crate) fn esplora_url(&self) -> &str {
        &self.esplora_url
    }

    /// Getter for general addresses
    pub(crate) fn general_addresses(&self) -> &Vec<(String, String)> {
        &self.general_addresses
    }

    /// Getter for stake chain addresses
    pub(crate) fn stake_chain_addresses(&self) -> &Vec<(String, String)> {
        &self.stake_chain_addresses
    }
}

/// Unified configuration for all balance monitoring
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct BalanceMonitoringConfig {
    /// Faucet balance monitoring configuration
    faucet: FaucetBalanceConfig,
    /// Bridge operator balance monitoring configuration
    bridge_operators: BridgeOperatorConfig,
    /// Refresh interval in seconds
    refresh_interval_s: u64,
}

impl BalanceMonitoringConfig {
    pub(crate) fn faucet(&self) -> &FaucetBalanceConfig {
        &self.faucet
    }

    pub(crate) fn bridge_operators(&self) -> &BridgeOperatorConfig {
        &self.bridge_operators
    }

    pub(crate) fn refresh_interval_s(&self) -> u64 {
        self.refresh_interval_s
    }
}

impl Config {
    /// Load configuration from the specified path
    pub(crate) fn load_from_path(path: &str) -> Self {
        parse_toml::<Config>(path)
    }
}

/// Reads and parses a TOML file from the given path into the given type `T`.
///
/// # Panics
///
/// 1. If the file is not readable.
/// 2. If the contents of the file cannot be deserialized into the given type `T`.
///
fn parse_toml<T>(path: impl AsRef<Path>) -> T
where
    T: std::fmt::Debug + serde::de::DeserializeOwned,
{
    // Code borrowed from strata-bridge: https://github.com/alpenlabs/strata-bridge
    std::fs::read_to_string(path)
        .map(|p| {
            trace!(?p, "read file");

            let parsed = toml::from_str::<T>(&p).unwrap_or_else(|e| {
                panic!("failed to parse TOML file: {e:?}");
            });
            debug!(?parsed, "parsed TOML file");

            parsed
        })
        .unwrap_or_else(|_| {
            panic!("failed to read TOML file");
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_serde_toml() {
        let config = r#"
[server]
host = "0.0.0.0"
port = 3000

[network]
sequencer_url = "https://rpc.testnet.alpenlabs.io"
rpc_url = "https://rpc.testnet.alpenlabs.io"
bundler_url = "https://bundler.testnet.alpenlabs.io/health"
max_retries = 5
total_retry_time_s = 60
status_refetch_interval_s = 10

[bridge]
esplora_url = "https://esplora.testnet.alpenlabs.io"
max_tx_confirmations = 6
status_refetch_interval_s = 120

[[bridge.operators]]
public_key = "02deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
rpc_url = "https://bridge.testnet.alpenlabs.io/1"

[[bridge.operators]]
public_key = "02cafebabe1234567890abcdef1234567890abcdef1234567890abcdef123456"
rpc_url = "https://bridge.testnet.alpenlabs.io/2"

[[bridge.operators]]
public_key = "02f00dbabe9876543210fedcba9876543210fedcba9876543210fedcba987654"
rpc_url = "https://bridge.testnet.alpenlabs.io/3"

[[bridge.operators]]
public_key = "02badcafe1111111111111111111111111111111111111111111111111111111111"
rpc_url = "https://bridge.testnet.alpenlabs.io/4"

[balance]
refresh_interval_s = 300

[balance.faucet]
l1_url = "https://faucet-api.testnet.alpenlabs.io/balance/l1"
l2_url = "https://faucet-api.testnet.alpenlabs.io/balance/l2"

[balance.bridge_operators]
esplora_url = "https://esplora.testnet.alpenlabs.io"
general_addresses = [
    ["0273441f2ba801b557b23c15829f4a87c02332d59a71499da1479048e6175ff4e0", "tb1p9e8cemc7q7emc0s0gklwrlpl4jjh98en95as4pka35t0luv4dhjsdn098l"],
    ["026bc16ede3b4b30edd4b59ab3a7209de21b468508349983e17a08910ec7a82f5f", "tb1pkyy0wrpgjhtjtga6nh8jx9qq9l3vns3jcywyvyh3ms9p20f7zyfskvnvsj"]
]
stake_chain_addresses = [
    ["0273441f2ba801b557b23c15829f4a87c02332d59a71499da1479048e6175ff4e0", "tb1p22v50hp20j5644m88yjs7de3mn5ju7llw44hc6gtqfr2nsu35nkqn2n4qq"],
    ["026bc16ede3b4b30edd4b59ab3a7209de21b468508349983e17a08910ec7a82f5f", "tb1pa042m8jz7622qdvydakxj3ufrhxsg7wlc6kp2rnzkld3t92rcghsdtg6wg"]
]
"#;

        let config = toml::from_str::<Config>(config);
        assert!(
            config.is_ok(),
            "must be able to deserialize config from toml but got: {}",
            config.unwrap_err()
        );

        let config = config.unwrap();
        let serialized = toml::to_string(&config).unwrap();
        let deserialized = toml::from_str::<Config>(&serialized).unwrap();

        assert_eq!(
            deserialized, config,
            "must be able to serialize and deserialize config to toml"
        );
    }

    #[test]
    fn test_load_from_path() {
        // Test that load_from_path works with a valid config
        let config = Config::load_from_path("config.toml");
        assert_eq!(config.server.host(), "0.0.0.0");
        assert_eq!(config.server.port(), 3000);
        assert_eq!(config.network.max_retries(), 5);
        assert_eq!(config.bridge.max_tx_confirmations(), 6);
        assert_eq!(config.balance.refresh_interval_s(), 300);
    }

    #[test]
    fn test_getter_functions() {
        let config_content = r#"
[server]
host = "127.0.0.1"
port = 8080

[network]
sequencer_url = "https://sequencer.example.com"
rpc_url = "https://rpc.example.com"
bundler_url = "https://bundler.example.com/health"
max_retries = 3
total_retry_time_s = 30
status_refetch_interval_s = 5

[bridge]
esplora_url = "https://esplora.example.com"
max_tx_confirmations = 12
status_refetch_interval_s = 60

[[bridge.operators]]
public_key = "02deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
rpc_url = "https://bridge.example.com/1"

[[bridge.operators]]
public_key = "02cafebabe1234567890abcdef1234567890abcdef1234567890abcdef123456"
rpc_url = "https://bridge.example.com/2"

[balance]
refresh_interval_s = 300

[balance.faucet]
l1_url = "https://faucet-api.testnet.alpenlabs.io/balance/l1"
l2_url = "https://faucet-api.testnet.alpenlabs.io/balance/l2"

[balance.bridge_operators]
esplora_url = "https://esplora.testnet.alpenlabs.io"
general_addresses = [
    ["0273441f2ba801b557b23c15829f4a87c02332d59a71499da1479048e6175ff4e0", "tb1p9e8cemc7q7emc0s0gklwrlpl4jjh98en95as4pka35t0luv4dhjsdn098l"],
    ["026bc16ede3b4b30edd4b59ab3a7209de21b468508349983e17a08910ec7a82f5f", "tb1pkyy0wrpgjhtjtga6nh8jx9qq9l3vns3jcywyvyh3ms9p20f7zyfskvnvsj"]
]
stake_chain_addresses = [
    ["0273441f2ba801b557b23c15829f4a87c02332d59a71499da1479048e6175ff4e0", "tb1p22v50hp20j5644m88yjs7de3mn5ju7llw44hc6gtqfr2nsu35nkqn2n4qq"],
    ["026bc16ede3b4b30edd4b59ab3a7209de21b468508349983e17a08910ec7a82f5f", "tb1pa042m8jz7622qdvydakxj3ufrhxsg7wlc6kp2rnzkld3t92rcghsdtg6wg"]
]
"#;

        let config = toml::from_str::<Config>(config_content);
        let config = config.unwrap();

        // Verify the loaded configuration
        assert_eq!(config.server.host(), "127.0.0.1");
        assert_eq!(config.server.port(), 8080);
        assert_eq!(
            config.network.sequencer_url(),
            "https://sequencer.example.com"
        );
        assert_eq!(config.network.max_retries(), 3);
        assert_eq!(config.network.total_retry_time(), 30);
        assert_eq!(config.network.status_refetch_interval(), 5);
        assert_eq!(config.bridge.esplora_url(), "https://esplora.example.com");
        assert_eq!(config.bridge.max_tx_confirmations(), 12);
        assert_eq!(config.bridge.status_refetch_interval(), 60);
        assert_eq!(config.bridge.operators().len(), 2);
        assert_eq!(
            config.bridge.operators()[0].public_key(),
            "02deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
        );
        assert_eq!(
            config.bridge.operators()[0].rpc_url(),
            "https://bridge.example.com/1"
        );
        assert_eq!(
            config.bridge.operators()[1].public_key(),
            "02cafebabe1234567890abcdef1234567890abcdef1234567890abcdef123456"
        );
        assert_eq!(
            config.bridge.operators()[1].rpc_url(),
            "https://bridge.example.com/2"
        );

        // Verify balance configuration
        assert_eq!(config.balance.refresh_interval_s(), 300);
        assert_eq!(
            config.balance.faucet().l1_url(),
            "https://faucet-api.testnet.alpenlabs.io/balance/l1"
        );
        assert_eq!(
            config.balance.faucet().l2_url(),
            "https://faucet-api.testnet.alpenlabs.io/balance/l2"
        );
        assert_eq!(
            config.balance.bridge_operators().esplora_url(),
            "https://esplora.testnet.alpenlabs.io"
        );
        assert_eq!(
            config.balance.bridge_operators().general_addresses().len(),
            2
        );
        assert_eq!(
            config
                .balance
                .bridge_operators()
                .stake_chain_addresses()
                .len(),
            2
        );
        assert_eq!(
            config.balance.bridge_operators().general_addresses()[0].0,
            "0273441f2ba801b557b23c15829f4a87c02332d59a71499da1479048e6175ff4e0"
        );
        assert_eq!(
            config.balance.bridge_operators().general_addresses()[0].1,
            "tb1p9e8cemc7q7emc0s0gklwrlpl4jjh98en95as4pka35t0luv4dhjsdn098l"
        );
    }
}
