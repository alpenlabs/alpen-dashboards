//! Shared helpers for bridge tests.

use status_config::BridgeMonitoringConfig;
use strata_retry::RetryConfig;

pub(crate) fn make_retry_config() -> RetryConfig {
    let config: BridgeMonitoringConfig = toml::from_str(
        r#"
        esplora_url = "http://localhost:3000"
        max_tx_confirmations = 6
        status_refetch_interval_s = 1
        operators = []

        [retry]
        base_delay_ms = 1
        max_delay_ms = 1
        "#,
    )
    .expect("parse test retry config");
    config.retry_policy()
}
