use tracing::info;

use super::env::EnvVarParser;

/// Default network status refetch interval in seconds
const DEFAULT_NETWORK_STATUS_REFETCH_INTERVAL_S: u64 = 10;

/// Default retry policy max retries
const DEFAULT_RETRY_POLICY_MAX_RETRIES: u64 = 5;

/// Default retry policy total retry time
const DEFAULT_RETRY_POLICY_TOTAL_RETRY_TIME: u64 = 60;

/// Default retry policy base
const DEFAULT_RETRY_POLICY_BASE: f64 = 1.5;

#[derive(Debug, Clone)]
pub(crate) struct NetworkConfig {
    /// JSON-RPC Endpoint for Strata sequencer
    sequencer_url: String,

    /// JSON-RPC Endpoint for Strata client and reth
    rpc_url: String,

    /// Bundler health check URL (overrides `.env`)
    bundler_url: String,

    /// Max retries for status queries
    retry_policy_max_retries: u64,

    /// Total time in seconds to spend retrying status queries
    retry_policy_total_time_s: u64,

    /// Network status refetch interval in seconds
    status_refetch_interval_s: u64,
}

impl NetworkConfig {
    pub(crate) fn new() -> Self {
        let sequencer_url = String::parse_env_var("STRATA_SEQUENCER_URL")
            .unwrap_or_else(|| "http://localhost:8432".to_string());

        let rpc_url =
            String::parse_env_var("RPC_URL").unwrap_or_else(|| "http://localhost:8433".to_string());

        let bundler_url = String::parse_env_var("BUNDLER_URL")
            .unwrap_or_else(|| "http://localhost:8434".to_string());

        let retry_policy_max_retries = u64::parse_env_var("NETWORK_STATUS_MAX_RETRIES")
            .unwrap_or(DEFAULT_RETRY_POLICY_MAX_RETRIES);
        let retry_policy_total_time_s = u64::parse_env_var("NETWORK_STATUS_TOTAL_RETRY_TIME_S")
            .unwrap_or(DEFAULT_RETRY_POLICY_TOTAL_RETRY_TIME);

        let status_refetch_interval_s = u64::parse_env_var("NETWORK_STATUS_REFETCH_INTERVAL_S")
            .unwrap_or(DEFAULT_NETWORK_STATUS_REFETCH_INTERVAL_S);

        info!(%rpc_url, bundler_url, "Loaded Network monitoring config:");

        NetworkConfig {
            sequencer_url,
            rpc_url,
            bundler_url,
            retry_policy_max_retries,
            retry_policy_total_time_s,
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

    /// Getter for `status_refetch_interval_s`
    pub(crate) fn status_refetch_interval(&self) -> u64 {
        self.status_refetch_interval_s
    }

    /// Retry policy for sequencer status queries
    pub(crate) fn sequencer_retry_policy(&self) -> crate::utils::retry_policy::ExponentialBackoff {
        crate::utils::retry_policy::ExponentialBackoff::new(
            self.retry_policy_max_retries,
            self.retry_policy_total_time_s,
            DEFAULT_RETRY_POLICY_BASE,
        )
    }

    /// Retry policy for RPC endpoint status queries
    pub(crate) fn rpc_retry_policy(&self) -> crate::utils::retry_policy::ExponentialBackoff {
        crate::utils::retry_policy::ExponentialBackoff::new(
            self.retry_policy_max_retries,
            self.retry_policy_total_time_s,
            DEFAULT_RETRY_POLICY_BASE,
        )
    }
}
