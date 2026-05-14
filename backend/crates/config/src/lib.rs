use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::{debug, trace};

use status_utils::ExponentialBackoff;

/// Main configuration struct containing all application settings
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Config {
    /// Base directory for persisted dashboard data.
    datadir: PathBuf,
    /// API server configuration
    server: ApiServerConfig,
    /// Network monitoring configuration
    network: NetworkMonitoringConfig,
    /// Bridge monitoring configuration
    bridge: BridgeMonitoringConfig,

    /// Withdrawal-intent indexer configuration
    withdrawal_indexer: WithdrawalIndexerConfig,
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

/// Default EVM logs batch size for the withdrawal indexer.
const DEFAULT_ETH_LOGS_BATCH_SIZE: u64 = 1000;

/// Default finality lag (in blocks) the indexer keeps behind the chain head.
const DEFAULT_ETH_LOGS_FINALITY_LAG: u64 = 12;

/// Default starting block for the indexer when no checkpoint is persisted.
const DEFAULT_ETH_LOGS_START_BLOCK: u64 = 0;

/// Default poll interval for the indexer in seconds.
const DEFAULT_ETH_LOGS_POLL_INTERVAL_S: u64 = 10;

/// Default fixed withdrawal denomination in sats (1 BTC).
const DEFAULT_WITHDRAWAL_DENOMINATION_SATS: u64 = 100_000_000;

fn default_eth_logs_batch_size() -> u64 {
    DEFAULT_ETH_LOGS_BATCH_SIZE
}
fn default_eth_logs_finality_lag() -> u64 {
    DEFAULT_ETH_LOGS_FINALITY_LAG
}
fn default_eth_logs_start_block() -> u64 {
    DEFAULT_ETH_LOGS_START_BLOCK
}
fn default_eth_logs_poll_interval_s() -> u64 {
    DEFAULT_ETH_LOGS_POLL_INTERVAL_S
}
fn default_withdrawal_denomination_sats() -> u64 {
    DEFAULT_WITHDRAWAL_DENOMINATION_SATS
}

/// Configuration for the EVM withdrawal-intent indexer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WithdrawalIndexerConfig {
    /// JSON-RPC endpoint for the Strata EVM client.
    eth_rpc_url: String,

    /// Number of blocks to scan per `eth_getLogs` request.
    #[serde(default = "default_eth_logs_batch_size")]
    batch_size: u64,

    /// Number of blocks the indexer stays behind the chain head to avoid reorgs.
    #[serde(default = "default_eth_logs_finality_lag")]
    finality_lag: u64,

    /// Block number from which the indexer starts when no checkpoint is persisted.
    #[serde(default = "default_eth_logs_start_block")]
    start_block: u64,

    /// Poll interval (in seconds) between successive indexer scans.
    #[serde(default = "default_eth_logs_poll_interval_s")]
    poll_interval_s: u64,

    /// Fixed withdrawal denomination in sats.
    #[serde(default = "default_withdrawal_denomination_sats")]
    withdrawal_denomination_sats: u64,
}

impl WithdrawalIndexerConfig {
    pub fn eth_rpc_url(&self) -> &str {
        &self.eth_rpc_url
    }

    pub fn batch_size(&self) -> u64 {
        self.batch_size
    }

    pub fn finality_lag(&self) -> u64 {
        self.finality_lag
    }

    pub fn start_block(&self) -> u64 {
        self.start_block
    }

    pub fn poll_interval_s(&self) -> u64 {
        self.poll_interval_s
    }

    pub fn withdrawal_denomination_sats(&self) -> u64 {
        self.withdrawal_denomination_sats
    }
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

    pub fn datadir(&self) -> &Path {
        &self.datadir
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

    pub fn withdrawal_indexer(&self) -> &WithdrawalIndexerConfig {
        &self.withdrawal_indexer
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
datadir = "data"

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

[withdrawal_indexer]
eth_rpc_url = "https://rpc.testnet.alpenlabs.io"
withdrawal_denomination_sats = 100000000
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
datadir = "data"

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

[withdrawal_indexer]
eth_rpc_url = ""
batch_size = 500
finality_lag = 6
start_block = 100
poll_interval_s = 5
withdrawal_denomination_sats = 100000000
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
datadir = "data"

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

[withdrawal_indexer]
eth_rpc_url = "https://rpc.example.com"
batch_size = 250
finality_lag = 8
start_block = 1234
poll_interval_s = 7
withdrawal_denomination_sats = 100000000
"#;

        let config = toml::from_str::<Config>(config_content);
        let config = config.unwrap();

        // Verify the loaded configuration
        assert_eq!(config.datadir(), Path::new("data"));
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
        assert_eq!(
            config.withdrawal_indexer().eth_rpc_url(),
            "https://rpc.example.com"
        );
        assert_eq!(config.withdrawal_indexer().batch_size(), 250);
        assert_eq!(config.withdrawal_indexer().finality_lag(), 8);
        assert_eq!(config.withdrawal_indexer().start_block(), 1234);
        assert_eq!(config.withdrawal_indexer().poll_interval_s(), 7);
        assert_eq!(
            config.withdrawal_indexer().withdrawal_denomination_sats(),
            100_000_000
        );
    }

    #[test]
    fn test_withdrawal_indexer_defaults() {
        let toml_doc = r#"
datadir = "data"

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

[withdrawal_indexer]
eth_rpc_url = "https://rpc.example.com"
"#;

        let config = toml::from_str::<Config>(toml_doc).expect("parse");
        let indexer = config.withdrawal_indexer();
        assert_eq!(indexer.batch_size(), DEFAULT_ETH_LOGS_BATCH_SIZE);
        assert_eq!(indexer.finality_lag(), DEFAULT_ETH_LOGS_FINALITY_LAG);
        assert_eq!(indexer.start_block(), DEFAULT_ETH_LOGS_START_BLOCK);
        assert_eq!(indexer.poll_interval_s(), DEFAULT_ETH_LOGS_POLL_INTERVAL_S);
        assert_eq!(
            indexer.withdrawal_denomination_sats(),
            DEFAULT_WITHDRAWAL_DENOMINATION_SATS
        );
    }
}
