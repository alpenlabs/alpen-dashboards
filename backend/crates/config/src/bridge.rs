use bitcoin::PublicKey;
use serde::{Deserialize, Serialize};
use strata_retry::RetryConfig;

use crate::retry::RequestRetryConfig;

/// Default timeout for Esplora HTTP requests in seconds.
const DEFAULT_ESPLORA_REQUEST_TIMEOUT_S: u64 = 5;

/// Default time a bridge status request waits for the first poll result.
const DEFAULT_BRIDGE_INITIAL_STATUS_WAIT_TIMEOUT_S: u64 = 30;

/// Default limit for deposit indices considered for new pairings per cycle
/// and indexed WRT rows fetched per withdrawal-index DB batch.
const DEFAULT_WITHDRAWAL_PAIRING_BATCH_SIZE: usize = 1_000;

fn default_esplora_request_timeout_s() -> u64 {
    DEFAULT_ESPLORA_REQUEST_TIMEOUT_S
}

fn default_bridge_initial_status_wait_timeout_s() -> u64 {
    DEFAULT_BRIDGE_INITIAL_STATUS_WAIT_TIMEOUT_S
}

fn default_withdrawal_pairing_batch_size() -> usize {
    DEFAULT_WITHDRAWAL_PAIRING_BATCH_SIZE
}

/// Configuration for bridge monitoring services
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BridgeMonitoringConfig {
    /// Esplora URL
    esplora_url: String,

    /// Timeout for Esplora HTTP requests in seconds.
    #[serde(default = "default_esplora_request_timeout_s")]
    esplora_request_timeout_s: u64,

    /// Confirmation cutoff for terminal rows in bridge status.
    /// Rows with known confirmations below the cutoff are included; reaching it
    /// permits purging and cursor progress.
    max_tx_confirmations: u64,

    /// Bridge status refetch interval in seconds
    status_refetch_interval_s: u64,

    /// Timeout for HTTP status requests waiting on the first poll result.
    #[serde(default = "default_bridge_initial_status_wait_timeout_s")]
    initial_status_wait_timeout_s: u64,

    /// Maximum deposit indices considered for new pairings per cycle and indexed
    /// WRT rows fetched per withdrawal-index DB batch.
    #[serde(default = "default_withdrawal_pairing_batch_size")]
    withdrawal_pairing_batch_size: usize,

    /// Request retry policy for bridge RPC.
    #[serde(default)]
    retry: RequestRetryConfig,

    /// Bridge operators
    operators: Vec<BridgeOperator>,
}

impl BridgeMonitoringConfig {
    /// Builds the bridge RPC retry policy from `[bridge.retry]` settings.
    pub fn retry_policy(&self) -> RetryConfig {
        self.retry.policy()
    }

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
    /// JSON-RPC endpoint for the EVM execution node.
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

#[cfg(test)]
mod tests {
    use super::{BridgeMonitoringConfig, BridgeOperator};

    fn make_bridge_config(retry_table: &str) -> BridgeMonitoringConfig {
        toml::from_str(&format!(
            r#"
            esplora_url = "http://localhost:3000"
            max_tx_confirmations = 6
            status_refetch_interval_s = 1
            operators = []
            {retry_table}
            "#
        ))
        .expect("parse bridge config")
    }

    #[test]
    fn bridge_retry_uses_defaults_for_missing_or_empty_table() {
        // Existing config files and an explicitly empty table use the same policy.
        for table in ["", "[retry]"] {
            let policy = make_bridge_config(table).retry_policy();
            assert_eq!(policy.max_retries, 3);
            assert_eq!(policy.base_delay_ms, 2_000);
            assert_eq!(policy.max_delay_ms, 5_000);
            assert_eq!(policy.multiplier, 150);
            assert_eq!(policy.multiplier_base, 100);
        }
    }

    #[test]
    fn bridge_retry_preserves_overrides_across_config_roundtrip() {
        let config =
            make_bridge_config("[retry]\nmax_retries = 2\nbase_delay_ms = 100\nmax_delay_ms = 900");
        let policy = config.retry_policy();
        assert_eq!(policy.max_retries, 2);
        assert_eq!(policy.base_delay_ms, 100);
        assert_eq!(policy.max_delay_ms, 900);
        // Growth remains fixed at 1.5 while the operator controls the budget and cap.
        assert_eq!(policy.multiplier, 150);
        assert_eq!(policy.multiplier_base, 100);

        let serialized = toml::to_string(&config).expect("serialize bridge config");
        let restored: BridgeMonitoringConfig =
            toml::from_str(&serialized).expect("restore bridge config");
        assert_eq!(restored, config);
    }

    #[test]
    fn bridge_retry_partial_override_can_disable_retries() {
        // Zero retries still allows the initial request; omitted delay fields keep defaults.
        let policy = make_bridge_config("[retry]\nmax_retries = 0").retry_policy();
        assert_eq!(policy.max_retries, 0);
        assert_eq!(policy.base_delay_ms, 2_000);
        assert_eq!(policy.max_delay_ms, 5_000);
    }

    #[test]
    fn bridge_operator_requires_name() {
        let error = toml::from_str::<BridgeOperator>(
            r#"
            public_key = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
            rpc_url = "http://localhost:5678"
        "#,
        )
        .expect_err("operator name is required");
        assert!(
            error.to_string().contains("missing field `name`"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn bridge_operator_allows_missing_rpc_url() {
        // An external operator can have a display identity without its own RPC endpoint.
        let operator: BridgeOperator = toml::from_str(
            r#"
            name = "External Operator"
            public_key = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
        "#,
        )
        .expect("parse external operator");
        assert_eq!(operator.name(), "External Operator");
        assert_eq!(operator.rpc_url(), None);
    }
}
