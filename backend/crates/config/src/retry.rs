use serde::{Deserialize, Serialize};
use strata_retry::RetryConfig;

/// Request retry settings used by network and bridge monitoring.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct RequestRetryConfig {
    /// Retries after the initial request; zero disables retries.
    max_retries: u16,

    /// Delay before the first retry.
    base_delay_ms: u64,

    /// Cap applied as retry delays increase.
    max_delay_ms: u64,
}

impl Default for RequestRetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay_ms: 2_000,
            max_delay_ms: 5_000,
        }
    }
}

impl RequestRetryConfig {
    pub(crate) fn policy(&self) -> RetryConfig {
        RetryConfig {
            max_retries: self.max_retries,
            base_delay_ms: self.base_delay_ms,
            multiplier: 150,
            multiplier_base: 100,
            max_delay_ms: self.max_delay_ms,
        }
    }
}
