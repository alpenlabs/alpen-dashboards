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
total_retry_time = 60
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
        assert_eq!(config.bridge.max_tx_confirmations(), 144);
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
total_retry_time = 30
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
    }
}
