use serde::{Deserialize, Serialize};
use std::path::Path;
use tracing::{debug, trace};

use status_utils::ExponentialBackoff;

/// Main configuration struct containing all application settings
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Config {
    /// API server configuration
    server: ApiServerConfig,
    /// Network monitoring configuration
    network: NetworkMonitoringConfig,
    /// Bridge monitoring configuration
    bridge: BridgeMonitoringConfig,
}

/// Configuration for the API server
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ApiServerConfig {
    /// Host address to bind the server to
    host: String,
    /// Port number to bind the server to
    port: u16,
}

impl ApiServerConfig {
    /// Get a reference to the host address
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Get the port number
    pub fn port(&self) -> u16 {
        self.port
    }
}

/// Default retry policy base for exponential backoff
const DEFAULT_RETRY_POLICY_BASE: f64 = 1.5;

/// Configuration for network monitoring services
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NetworkMonitoringConfig {
    /// JSON-RPC Endpoint for Strata sequencer
    sequencer_url: String,

    /// JSON-RPC Endpoint for Strata client and reth
    rpc_url: String,

    /// Bundler health check URL
    bundler_url: String,

    /// Max retries for status queries
    retry_policy_max_retries: u64,

    /// Total time in seconds to spend retrying status queries
    retry_policy_total_time_s: u64,

    /// Network status refetch interval in seconds
    status_refetch_interval_s: u64,
}

impl NetworkMonitoringConfig {
    pub fn sequencer_url(&self) -> &str {
        &self.sequencer_url
    }

    pub fn rpc_url(&self) -> &str {
        &self.rpc_url
    }

    pub fn bundler_url(&self) -> &str {
        &self.bundler_url
    }

    pub fn status_refetch_interval(&self) -> u64 {
        self.status_refetch_interval_s
    }

    /// Retry policy for sequencer status queries
    pub fn sequencer_retry_policy(&self) -> ExponentialBackoff {
        ExponentialBackoff::new(
            self.retry_policy_max_retries,
            self.retry_policy_total_time_s,
            DEFAULT_RETRY_POLICY_BASE,
        )
    }

    /// Retry policy for RPC endpoint status queries
    pub fn rpc_retry_policy(&self) -> ExponentialBackoff {
        ExponentialBackoff::new(
            self.retry_policy_max_retries,
            self.retry_policy_total_time_s,
            DEFAULT_RETRY_POLICY_BASE,
        )
    }
}

/// Configuration for bridge monitoring services
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BridgeMonitoringConfig {
    /// Esplora URL
    esplora_url: String,

    /// Maximum confirmations
    max_tx_confirmations: u64,

    /// Bridge status refetch interval in seconds
    status_refetch_interval_s: u64,

    /// Bridge operators
    operators: Vec<BridgeOperator>,
}

/// Configuration for a bridge operator
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BridgeOperator {
    /// Public key of the bridge operator
    public_key: String,
    /// RPC URL for the bridge operator
    rpc_url: String,
}

impl BridgeOperator {
    pub fn public_key(&self) -> &str {
        &self.public_key
    }

    pub fn rpc_url(&self) -> &str {
        &self.rpc_url
    }
}

impl BridgeMonitoringConfig {
    pub fn esplora_url(&self) -> &str {
        &self.esplora_url
    }

    pub fn max_tx_confirmations(&self) -> u64 {
        self.max_tx_confirmations
    }

    pub fn status_refetch_interval(&self) -> u64 {
        self.status_refetch_interval_s
    }

    pub fn operators(&self) -> &Vec<BridgeOperator> {
        &self.operators
    }
}

impl Config {
    /// Load configuration from the specified path
    pub fn load_from_path(path: &str) -> Self {
        parse_toml::<Config>(path)
    }

    pub fn server(&self) -> &ApiServerConfig {
        &self.server
    }

    pub fn network(&self) -> &NetworkMonitoringConfig {
        &self.network
    }

    pub fn bridge(&self) -> &BridgeMonitoringConfig {
        &self.bridge
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
retry_policy_max_retries = 5
retry_policy_total_time_s = 60
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
        const TOML_FIXTURE: &str = r#"
[server]
host = "127.0.0.1"
port = 8080

[network]
sequencer_url = ""
rpc_url = ""
bundler_url = ""
retry_policy_max_retries = 0
retry_policy_total_time_s = 0
status_refetch_interval_s = 0

[bridge]
esplora_url = ""
max_tx_confirmations = 0
status_refetch_interval_s = 0
operators = []
"#;

        let path = std::env::temp_dir().join(format!(
            "status_config_load_from_path_{}.toml",
            std::process::id()
        ));
        std::fs::write(&path, TOML_FIXTURE).expect("write tmp config");

        let loaded = Config::load_from_path(path.to_str().expect("path utf-8"));

        let _ = std::fs::remove_file(&path);

        let expected = toml::from_str::<Config>(TOML_FIXTURE).expect("parse fixture");
        assert_eq!(loaded, expected);
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
retry_policy_max_retries = 3
retry_policy_total_time_s = 30
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
