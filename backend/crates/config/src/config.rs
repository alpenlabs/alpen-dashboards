use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::{debug, trace};

use crate::{BridgeMonitoringConfig, NetworkMonitoringConfig, WithdrawalIndexerConfig};

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

impl Config {
    /// Load configuration from the specified path
    ///
    /// # Panics
    ///
    /// Panics if the file cannot be read or does not contain a valid TOML configuration.
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

    fn make_config_toml() -> &'static str {
        r#"
datadir = "data"

[server]
host = "127.0.0.1"
port = 8080

[network]
alpen_sequencer_url = "https://alpen-sequencer.example.com"
alpen_rpc_url = "https://alpen-rpc.example.com"
strata_sequencer_url = "https://strata-sequencer.example.com"
strata_rpc_url = "https://strata-rpc.example.com"
bundler_url = "https://bundler.example.com/health"
status_refetch_interval_s = 5
initial_status_wait_timeout_s = 4

[network.retry]
max_retries = 1
base_delay_ms = 200
max_delay_ms = 800

[bridge]
esplora_request_timeout_s = 9
esplora_url = "https://esplora.example.com"
max_tx_confirmations = 12
status_refetch_interval_s = 60
initial_status_wait_timeout_s = 7
withdrawal_pairing_batch_size = 500

[bridge.retry]
max_retries = 2
base_delay_ms = 100
max_delay_ms = 900

[[bridge.operators]]
name = "Operator 1"
public_key = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
rpc_url = "https://bridge.example.com/1"

[[bridge.operators]]
name = "Operator 2"
public_key = "02c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5"
rpc_url = "https://bridge.example.com/2"

[withdrawal_indexer]
eth_rpc_url = "https://rpc.example.com"
batch_size = 250
finality_lag = 8
start_block = 1234
poll_interval_s = 7
withdrawal_denomination_sats = 100000000
"#
    }

    fn make_config() -> Config {
        toml::from_str(make_config_toml()).expect("parse configured settings")
    }

    #[test]
    fn config_preserves_settings_across_toml_roundtrip() {
        let config = make_config();

        // Serialization must preserve the same public configuration, including retry overrides.
        let serialized = toml::to_string(&config).expect("serialize config");
        let restored: Config = toml::from_str(&serialized).expect("restore config");
        assert_eq!(restored, config);
    }

    #[test]
    fn config_loads_settings_from_file() {
        let path = std::env::temp_dir().join(format!(
            "status_config_load_from_path_{}.toml",
            std::process::id()
        ));
        std::fs::write(&path, make_config_toml()).expect("write test config");

        let loaded = Config::load_from_path(path.to_str().expect("config path UTF-8"));
        std::fs::remove_file(&path).expect("remove test config");
        assert_eq!(loaded, make_config());
    }

    #[test]
    fn config_accessors_return_configured_values() {
        let config = make_config();
        // Each public accessor must return the configured value, including the nested retry policy.
        assert_eq!(config.datadir(), Path::new("data"));
        assert_eq!(config.server().host(), "127.0.0.1");
        assert_eq!(config.server().port(), 8080);
        assert_eq!(
            config.network().alpen_sequencer_url(),
            "https://alpen-sequencer.example.com"
        );
        assert_eq!(
            config.network().alpen_rpc_url(),
            "https://alpen-rpc.example.com"
        );
        assert_eq!(
            config.network().strata_sequencer_url(),
            "https://strata-sequencer.example.com"
        );
        assert_eq!(
            config.network().strata_rpc_url(),
            "https://strata-rpc.example.com"
        );
        assert_eq!(config.network().status_refetch_interval(), 5);
        assert_eq!(config.network().initial_status_wait_timeout_s(), 4);
        let network_retry = config.network().retry_policy();
        assert_eq!(network_retry.max_retries, 1);
        assert_eq!(network_retry.base_delay_ms, 200);
        assert_eq!(network_retry.max_delay_ms, 800);
        assert_eq!(config.bridge().esplora_url(), "https://esplora.example.com");
        assert_eq!(config.bridge().esplora_request_timeout_s(), 9);
        assert_eq!(config.bridge().max_tx_confirmations(), 12);
        assert_eq!(config.bridge().status_refetch_interval(), 60);
        assert_eq!(config.bridge().initial_status_wait_timeout_s(), 7);
        assert_eq!(config.bridge().withdrawal_pairing_batch_size(), 500);
        assert_eq!(config.bridge().operators().len(), 2);
        assert_eq!(config.bridge().operators()[0].name(), "Operator 1");
        assert_eq!(
            config.bridge().operators()[0].public_key().to_string(),
            "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
        );
        assert_eq!(
            config.bridge().operators()[0].rpc_url(),
            Some("https://bridge.example.com/1")
        );
        assert_eq!(config.bridge().operators()[1].name(), "Operator 2");
        assert_eq!(
            config.bridge().operators()[1].public_key().to_string(),
            "02c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5"
        );
        assert_eq!(
            config.bridge().operators()[1].rpc_url(),
            Some("https://bridge.example.com/2")
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
        let retry = config.bridge().retry_policy();
        assert_eq!(retry.max_retries, 2);
        assert_eq!(retry.base_delay_ms, 100);
        assert_eq!(retry.max_delay_ms, 900);
    }

    #[test]
    fn config_uses_defaults_for_omitted_optional_settings() {
        // Minimal existing config files remain valid when new optional settings are added.
        let config: Config = toml::from_str(
            r#"
datadir = "data"

[server]
host = "127.0.0.1"
port = 8080

[network]
alpen_sequencer_url = ""
alpen_rpc_url = ""
strata_sequencer_url = ""
strata_rpc_url = ""
bundler_url = ""
status_refetch_interval_s = 0

[bridge]
esplora_url = ""
max_tx_confirmations = 0
status_refetch_interval_s = 0
operators = []

[withdrawal_indexer]
eth_rpc_url = "https://rpc.example.com"
"#,
        )
        .expect("parse minimal config");
        assert_eq!(config.bridge().esplora_request_timeout_s(), 5);
        assert_eq!(config.bridge().withdrawal_pairing_batch_size(), 1_000);
        assert_eq!(config.network().initial_status_wait_timeout_s(), 5);
        assert_eq!(config.bridge().initial_status_wait_timeout_s(), 30);
        let indexer = config.withdrawal_indexer();
        assert_eq!(indexer.batch_size(), 1_000);
        assert_eq!(indexer.finality_lag(), 12);
        assert_eq!(indexer.start_block(), 0);
        assert_eq!(indexer.poll_interval_s(), 10);
        assert_eq!(indexer.withdrawal_denomination_sats(), 100_000_000);
    }
}
