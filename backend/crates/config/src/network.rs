use serde::{Deserialize, Serialize};
use status_utils::ExponentialBackoff;

/// Default exponential growth factor for network request retries.
const DEFAULT_RETRY_POLICY_BASE: f64 = 1.5;

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

    /// Retry policy for status queries
    pub fn retry_policy(&self) -> ExponentialBackoff {
        ExponentialBackoff::new(
            self.retry_policy_max_retries,
            self.retry_policy_total_time_s,
            DEFAULT_RETRY_POLICY_BASE,
        )
    }
}
