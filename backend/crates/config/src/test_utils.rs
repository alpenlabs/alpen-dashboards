//! Config factories shared by backend tests.

use bitcoin::PublicKey;
use toml::{Table, Value};

use crate::{BridgeMonitoringConfig, BridgeOperator, NetworkMonitoringConfig};

pub const LOCAL_ESPLORA_URL: &str = "http://localhost:3002";

/// Confirmation cutoff of configs built by [`make_bridge_config`].
pub const TEST_MAX_CONFIRMATIONS: u64 = 6;

/// Endpoints monitored by the network status task.
pub struct NetworkUrls<'a> {
    pub alpen_sequencer: &'a str,
    pub alpen_rpc: &'a str,
    pub strata_sequencer: &'a str,
    pub strata_rpc: &'a str,
    pub bundler: &'a str,
}

pub const LOCAL_NETWORK_URLS: NetworkUrls<'static> = NetworkUrls {
    alpen_sequencer: "http://localhost:8545",
    alpen_rpc: "http://localhost:8546",
    strata_sequencer: "http://localhost:8432",
    strata_rpc: "http://localhost:8433",
    bundler: "http://localhost:4337/health",
};

/// `Some(n)` retries n times with 1 ms delays; `None` keeps
/// [`RequestRetryConfig`](crate::RequestRetryConfig) defaults.
pub fn make_bridge_config(
    esplora_url: &str,
    operators: &[BridgeOperator],
    max_retries: Option<u16>,
) -> BridgeMonitoringConfig {
    let mut table = Table::new();
    table.insert("esplora_url".into(), esplora_url.into());
    table.insert(
        "max_tx_confirmations".into(),
        Value::try_from(TEST_MAX_CONFIRMATIONS).expect("serialize confirmation cutoff"),
    );
    table.insert("status_refetch_interval_s".into(), 1.into());
    table.insert(
        "operators".into(),
        Value::try_from(operators).expect("serialize test operators"),
    );
    insert_retry_table(&mut table, max_retries);
    table.try_into().expect("build test bridge config")
}

pub fn make_bridge_operator(name: &str, public_key: &PublicKey, rpc_url: &str) -> BridgeOperator {
    let mut table = Table::new();
    table.insert("name".into(), name.into());
    table.insert("public_key".into(), public_key.to_string().into());
    table.insert("rpc_url".into(), rpc_url.into());
    table.try_into().expect("build test bridge operator")
}

/// `Some(n)` retries n times with 1 ms delays; `None` keeps
/// [`RequestRetryConfig`](crate::RequestRetryConfig) defaults.
pub fn make_network_config(
    urls: &NetworkUrls<'_>,
    max_retries: Option<u16>,
) -> NetworkMonitoringConfig {
    let mut table = Table::new();
    table.insert("alpen_sequencer_url".into(), urls.alpen_sequencer.into());
    table.insert("alpen_rpc_url".into(), urls.alpen_rpc.into());
    table.insert("strata_sequencer_url".into(), urls.strata_sequencer.into());
    table.insert("strata_rpc_url".into(), urls.strata_rpc.into());
    table.insert("bundler_url".into(), urls.bundler.into());
    table.insert("status_refetch_interval_s".into(), 1.into());
    insert_retry_table(&mut table, max_retries);
    table.try_into().expect("build test network config")
}

fn insert_retry_table(table: &mut Table, max_retries: Option<u16>) {
    let Some(max_retries) = max_retries else {
        return;
    };
    let mut retry = Table::new();
    retry.insert("max_retries".into(), i64::from(max_retries).into());
    retry.insert("base_delay_ms".into(), 1.into());
    retry.insert("max_delay_ms".into(), 1.into());
    table.insert("retry".into(), retry.into());
}
