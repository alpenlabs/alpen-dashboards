use serde::{Deserialize, Serialize};
use strata_retry::RetryConfig;

use crate::retry::RequestRetryConfig;

/// Default timeout for bundler health check requests in seconds.
const DEFAULT_BUNDLER_REQUEST_TIMEOUT_S: u64 = 5;

/// Default time a network status request waits for the first poll result.
const DEFAULT_NETWORK_INITIAL_STATUS_WAIT_TIMEOUT_S: u64 = 5;

fn default_bundler_request_timeout_s() -> u64 {
    DEFAULT_BUNDLER_REQUEST_TIMEOUT_S
}

fn default_network_initial_status_wait_timeout_s() -> u64 {
    DEFAULT_NETWORK_INITIAL_STATUS_WAIT_TIMEOUT_S
}

/// Configuration for network monitoring services
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NetworkMonitoringConfig {
    /// JSON-RPC endpoint for the Alpen sequencer
    alpen_sequencer_url: String,

    /// Public JSON-RPC endpoint for Alpen
    alpen_rpc_url: String,

    /// JSON-RPC endpoint for the Strata sequencer
    strata_sequencer_url: String,

    /// Public JSON-RPC endpoint for Strata
    strata_rpc_url: String,

    /// Bundler health check URL
    bundler_url: String,

    /// Timeout for bundler health check requests in seconds.
    #[serde(default = "default_bundler_request_timeout_s")]
    bundler_request_timeout_s: u64,

    /// Request retry policy for network JSON-RPC status queries.
    #[serde(default)]
    retry: RequestRetryConfig,

    /// Network status refetch interval in seconds
    status_refetch_interval_s: u64,

    /// Timeout for HTTP status requests waiting on the first poll result.
    #[serde(default = "default_network_initial_status_wait_timeout_s")]
    initial_status_wait_timeout_s: u64,
}

impl NetworkMonitoringConfig {
    pub fn alpen_sequencer_url(&self) -> &str {
        &self.alpen_sequencer_url
    }

    pub fn alpen_rpc_url(&self) -> &str {
        &self.alpen_rpc_url
    }

    pub fn strata_sequencer_url(&self) -> &str {
        &self.strata_sequencer_url
    }

    pub fn strata_rpc_url(&self) -> &str {
        &self.strata_rpc_url
    }

    pub fn bundler_url(&self) -> &str {
        &self.bundler_url
    }

    pub fn bundler_request_timeout_s(&self) -> u64 {
        self.bundler_request_timeout_s
    }

    pub fn status_refetch_interval(&self) -> u64 {
        self.status_refetch_interval_s
    }

    pub fn initial_status_wait_timeout_s(&self) -> u64 {
        self.initial_status_wait_timeout_s
    }

    /// Builds the request retry policy from `[network.retry]` settings.
    pub fn retry_policy(&self) -> RetryConfig {
        self.retry.policy()
    }
}

#[cfg(test)]
mod tests {
    use super::NetworkMonitoringConfig;

    fn make_config_with_retry(retry_table: &str) -> NetworkMonitoringConfig {
        toml::from_str(&format!(
            r#"
            alpen_sequencer_url = "http://localhost:8545"
            alpen_rpc_url = "http://localhost:8546"
            strata_sequencer_url = "http://localhost:8432"
            strata_rpc_url = "http://localhost:8433"
            bundler_url = "http://localhost:4337/health"
            status_refetch_interval_s = 1
            {retry_table}
            "#
        ))
        .expect("parse network config")
    }

    #[test]
    fn network_retry_uses_shared_defaults_for_missing_or_empty_table() {
        for table in ["", "[retry]"] {
            let policy = make_config_with_retry(table).retry_policy();
            assert_eq!(policy.max_retries, 3);
            assert_eq!(policy.base_delay_ms, 2_000);
            assert_eq!(policy.max_delay_ms, 5_000);
            assert_eq!(policy.multiplier, 150);
            assert_eq!(policy.multiplier_base, 100);
        }
    }

    #[test]
    fn network_retry_preserves_overrides_across_config_roundtrip() {
        let config = make_config_with_retry(
            "[retry]\nmax_retries = 2\nbase_delay_ms = 100\nmax_delay_ms = 900",
        );
        let policy = config.retry_policy();
        assert_eq!(policy.max_retries, 2);
        assert_eq!(policy.base_delay_ms, 100);
        assert_eq!(policy.max_delay_ms, 900);
        assert_eq!(policy.multiplier, 150);
        assert_eq!(policy.multiplier_base, 100);

        let serialized = toml::to_string(&config).expect("serialize network config");
        let restored: NetworkMonitoringConfig =
            toml::from_str(&serialized).expect("restore network config");
        assert_eq!(restored, config);
    }

    #[test]
    fn network_retry_partial_override_can_disable_retries() {
        // Omitting the delays keeps their defaults even when retries are disabled.
        let policy = make_config_with_retry("[retry]\nmax_retries = 0").retry_policy();
        assert_eq!(policy.max_retries, 0);
        assert_eq!(policy.base_delay_ms, 2_000);
        assert_eq!(policy.max_delay_ms, 5_000);
    }

    #[test]
    fn network_config_rejects_unknown_settings() {
        let config =
            toml::to_string(&make_config_with_retry("")).expect("serialize network config");

        // Unknown keys must fail instead of being silently ignored.
        for key in ["retry_policy_max_retries", "retry_policy_total_time_s"] {
            let error = toml::from_str::<NetworkMonitoringConfig>(&format!("{key} = 5\n{config}"))
                .expect_err("unknown network setting must be rejected");
            assert!(
                error
                    .to_string()
                    .contains(&format!("unknown field `{key}`")),
                "unexpected error: {error}"
            );
        }
    }
}
