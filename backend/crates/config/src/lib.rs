use bitcoin::PublicKey;
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
    /// Balance monitoring configuration
    balance: BalanceMonitoringConfig,

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

/// Default timeout for Esplora HTTP requests in seconds.
const DEFAULT_ESPLORA_REQUEST_TIMEOUT_S: u64 = 5;

/// Default time a network status request waits for the first poll result.
const DEFAULT_NETWORK_INITIAL_STATUS_WAIT_TIMEOUT_S: u64 = 5;

/// Default time a bridge status request waits for the first poll result.
const DEFAULT_BRIDGE_INITIAL_STATUS_WAIT_TIMEOUT_S: u64 = 30;

/// Default indexed WRT rows to read per withdrawal-index DB request.
const DEFAULT_WITHDRAWAL_PAIRING_BATCH_SIZE: usize = 1_000;

fn default_esplora_request_timeout_s() -> u64 {
    DEFAULT_ESPLORA_REQUEST_TIMEOUT_S
}
fn default_network_initial_status_wait_timeout_s() -> u64 {
    DEFAULT_NETWORK_INITIAL_STATUS_WAIT_TIMEOUT_S
}
fn default_bridge_initial_status_wait_timeout_s() -> u64 {
    DEFAULT_BRIDGE_INITIAL_STATUS_WAIT_TIMEOUT_S
}
fn default_withdrawal_pairing_batch_size() -> usize {
    DEFAULT_WITHDRAWAL_PAIRING_BATCH_SIZE
}

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

    /// Timeout for HTTP status requests waiting on the first poll result.
    #[serde(default = "default_network_initial_status_wait_timeout_s")]
    initial_status_wait_timeout_s: u64,
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

    pub fn initial_status_wait_timeout_s(&self) -> u64 {
        self.initial_status_wait_timeout_s
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

    /// Timeout for Esplora HTTP requests in seconds.
    #[serde(default = "default_esplora_request_timeout_s")]
    esplora_request_timeout_s: u64,

    /// Maximum confirmations
    max_tx_confirmations: u64,

    /// Bridge status refetch interval in seconds
    status_refetch_interval_s: u64,

    /// Timeout for HTTP status requests waiting on the first poll result.
    #[serde(default = "default_bridge_initial_status_wait_timeout_s")]
    initial_status_wait_timeout_s: u64,

    /// Indexed WRT rows to read per withdrawal-index DB request.
    #[serde(default = "default_withdrawal_pairing_batch_size")]
    withdrawal_pairing_batch_size: usize,

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
    /// Display name of the bridge operator
    name: String,

    /// Public key of the bridge operator
    public_key: PublicKey,

    /// RPC URL for the bridge operator.
    rpc_url: Option<String>,
}

impl BridgeOperator {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn public_key(&self) -> &PublicKey {
        &self.public_key
    }

    pub fn rpc_url(&self) -> Option<&str> {
        self.rpc_url.as_deref()
    }
}

impl BridgeMonitoringConfig {
    pub fn esplora_url(&self) -> &str {
        &self.esplora_url
    }

    pub fn esplora_request_timeout_s(&self) -> u64 {
        self.esplora_request_timeout_s
    }

    pub fn max_tx_confirmations(&self) -> u64 {
        self.max_tx_confirmations
    }

    pub fn status_refetch_interval(&self) -> u64 {
        self.status_refetch_interval_s
    }

    pub fn initial_status_wait_timeout_s(&self) -> u64 {
        self.initial_status_wait_timeout_s
    }

    pub fn withdrawal_pairing_batch_size(&self) -> usize {
        self.withdrawal_pairing_batch_size
    }

    pub fn operators(&self) -> &Vec<BridgeOperator> {
        &self.operators
    }
}

/// Configuration for faucet balance monitoring.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FaucetBalanceConfig {
    /// L1 faucet URL.
    l1_url: String,
    /// L2 faucet URL.
    l2_url: String,
}

impl FaucetBalanceConfig {
    /// Returns the L1 faucet URL.
    pub fn l1_url(&self) -> &str {
        &self.l1_url
    }

    /// Returns the L2 faucet URL.
    pub fn l2_url(&self) -> &str {
        &self.l2_url
    }
}

/// Configuration for bridge operator balance monitoring.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BridgeOperatorConfig {
    /// Esplora API URL.
    esplora_url: String,
    /// General wallet addresses as `(public_key, address)` tuples.
    general_addresses: Vec<(String, String)>,
    /// Stake chain wallet addresses as `(public_key, address)` tuples.
    stake_chain_addresses: Vec<(String, String)>,
}

impl BridgeOperatorConfig {
    /// Returns the Esplora API URL.
    pub fn esplora_url(&self) -> &str {
        &self.esplora_url
    }

    /// Returns the general wallet addresses.
    pub fn general_addresses(&self) -> &Vec<(String, String)> {
        &self.general_addresses
    }

    /// Returns the stake chain wallet addresses.
    pub fn stake_chain_addresses(&self) -> &Vec<(String, String)> {
        &self.stake_chain_addresses
    }
}

/// Configuration for wallet balance monitoring.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BalanceMonitoringConfig {
    /// Faucet balance monitoring configuration.
    faucet: FaucetBalanceConfig,
    /// Bridge operator balance monitoring configuration.
    bridge_operators: BridgeOperatorConfig,
    /// Refresh interval in seconds.
    refresh_interval_s: u64,
}

impl BalanceMonitoringConfig {
    /// Returns the faucet balance monitoring configuration.
    pub fn faucet(&self) -> &FaucetBalanceConfig {
        &self.faucet
    }

    /// Returns the bridge operator balance monitoring configuration.
    pub fn bridge_operators(&self) -> &BridgeOperatorConfig {
        &self.bridge_operators
    }

    /// Returns the refresh interval in seconds.
    pub fn refresh_interval_s(&self) -> u64 {
        self.refresh_interval_s
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

    pub fn balance(&self) -> &BalanceMonitoringConfig {
        &self.balance
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
sequencer_url = "https://strata.testnet.alpenlabs.io"
rpc_url = "https://alpen.testnet.alpenlabs.io"
bundler_url = "https://bundler.testnet.alpenlabs.io/health"
retry_policy_max_retries = 5
retry_policy_total_time_s = 60
status_refetch_interval_s = 10
initial_status_wait_timeout_s = 5

[bridge]
esplora_request_timeout_s = 5
esplora_url = "https://esplora.testnet.alpenlabs.io"
max_tx_confirmations = 6
status_refetch_interval_s = 120
initial_status_wait_timeout_s = 5

[[bridge.operators]]
name = "Operator 1"
public_key = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
rpc_url = "https://bridge.testnet.alpenlabs.io/1"

[[bridge.operators]]
name = "Operator 2"
public_key = "02c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5"
rpc_url = "https://bridge.testnet.alpenlabs.io/2"

[[bridge.operators]]
name = "Operator 3"
public_key = "02f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9"
rpc_url = "https://bridge.testnet.alpenlabs.io/3"

[[bridge.operators]]
name = "Operator 4"
public_key = "02e493dbf1c10d80f3581e4904930b1404cc6c13900ee0758474fa94abe8c4cd13"
rpc_url = "https://bridge.testnet.alpenlabs.io/4"

[balance]
refresh_interval_s = 300

[balance.faucet]
l1_url = "https://faucet.example.com/balance/l1"
l2_url = "https://faucet.example.com/balance/l2"

[balance.bridge_operators]
esplora_url = "https://esplora.testnet.alpenlabs.io"
general_addresses = [
    ["02deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef", "tb1p9e8cemc7q7emc0s0gklwrlpl4jjh98en95as4pka35t0luv4dhjsdn098l"]
]
stake_chain_addresses = [
    ["02deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef", "tb1p22v50hp20j5644m88yjs7de3mn5ju7llw44hc6gtqfr2nsu35nkqn2n4qq"]
]

[withdrawal_indexer]
eth_rpc_url = "https://alpen.testnet.alpenlabs.io"
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

[balance]
refresh_interval_s = 300

[balance.faucet]
l1_url = ""
l2_url = ""

[balance.bridge_operators]
esplora_url = ""
general_addresses = []
stake_chain_addresses = []

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
initial_status_wait_timeout_s = 4

[bridge]
esplora_request_timeout_s = 9
esplora_url = "https://esplora.example.com"
max_tx_confirmations = 12
status_refetch_interval_s = 60
initial_status_wait_timeout_s = 7
withdrawal_pairing_batch_size = 500

[[bridge.operators]]
name = "Operator 1"
public_key = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
rpc_url = "https://bridge.example.com/1"

[[bridge.operators]]
name = "Operator 2"
public_key = "02c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5"
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
        assert_eq!(config.network.initial_status_wait_timeout_s(), 4);
        assert_eq!(config.bridge.esplora_url(), "https://esplora.example.com");
        assert_eq!(config.bridge.esplora_request_timeout_s(), 9);
        assert_eq!(config.bridge.max_tx_confirmations(), 12);
        assert_eq!(config.bridge.status_refetch_interval(), 60);
        assert_eq!(config.bridge.initial_status_wait_timeout_s(), 7);
        assert_eq!(config.bridge.withdrawal_pairing_batch_size(), 500);
        assert_eq!(config.bridge.operators().len(), 2);
        assert_eq!(config.bridge.operators()[0].name(), "Operator 1");
        assert_eq!(
            config.bridge.operators()[0].public_key().to_string(),
            "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
        );
        assert_eq!(
            config.bridge.operators()[0].rpc_url(),
            Some("https://bridge.example.com/1")
        );
        assert_eq!(config.bridge.operators()[1].name(), "Operator 2");
        assert_eq!(
            config.bridge.operators()[1].public_key().to_string(),
            "02c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5"
        );
        assert_eq!(
            config.bridge.operators()[1].rpc_url(),
            Some("https://bridge.example.com/2")
        );
        assert_eq!(config.balance().refresh_interval_s(), 300);
        assert_eq!(
            config.balance().faucet().l1_url(),
            "https://faucet-api.testnet.alpenlabs.io/balance/l1"
        );
        assert_eq!(
            config.balance().faucet().l2_url(),
            "https://faucet-api.testnet.alpenlabs.io/balance/l2"
        );
        assert_eq!(
            config.balance().bridge_operators().esplora_url(),
            "https://esplora.testnet.alpenlabs.io"
        );
        assert_eq!(
            config
                .balance()
                .bridge_operators()
                .general_addresses()
                .len(),
            2
        );
        assert_eq!(
            config
                .balance()
                .bridge_operators()
                .stake_chain_addresses()
                .len(),
            2
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
    fn bridge_operator_name_is_required() {
        let config_content = r#"
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

[[bridge.operators]]
public_key = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
rpc_url = "https://bridge.example.com/1"

[balance]
refresh_interval_s = 300

[balance.faucet]
l1_url = ""
l2_url = ""

[balance.bridge_operators]
esplora_url = ""
general_addresses = []
stake_chain_addresses = []

[withdrawal_indexer]
eth_rpc_url = "https://rpc.example.com"
"#;

        let error = toml::from_str::<Config>(config_content).expect_err("missing operator name");
        assert!(
            error.to_string().contains("missing field `name`"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn bridge_operator_rpc_url_is_optional() {
        let config_content = r#"
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

[[bridge.operators]]
name = "External Operator"
public_key = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"

[balance]
refresh_interval_s = 300

[balance.faucet]
l1_url = ""
l2_url = ""

[balance.bridge_operators]
esplora_url = ""
general_addresses = []
stake_chain_addresses = []

[withdrawal_indexer]
eth_rpc_url = "https://rpc.example.com"
"#;

        let config = toml::from_str::<Config>(config_content).expect("parse config");
        assert_eq!(config.bridge.operators()[0].rpc_url(), None);
    }

    #[test]
    fn test_monitoring_defaults() {
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

[balance]
refresh_interval_s = 300

[balance.faucet]
l1_url = ""
l2_url = ""

[balance.bridge_operators]
esplora_url = ""
general_addresses = []
stake_chain_addresses = []

[withdrawal_indexer]
eth_rpc_url = "https://rpc.example.com"
"#;

        let config = toml::from_str::<Config>(toml_doc).expect("parse");
        assert_eq!(
            config.bridge().esplora_request_timeout_s(),
            DEFAULT_ESPLORA_REQUEST_TIMEOUT_S
        );
        assert_eq!(
            config.bridge().withdrawal_pairing_batch_size(),
            DEFAULT_WITHDRAWAL_PAIRING_BATCH_SIZE
        );
        assert_eq!(
            config.network().initial_status_wait_timeout_s(),
            DEFAULT_NETWORK_INITIAL_STATUS_WAIT_TIMEOUT_S
        );
        assert_eq!(
            config.bridge().initial_status_wait_timeout_s(),
            DEFAULT_BRIDGE_INITIAL_STATUS_WAIT_TIMEOUT_S
        );
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
