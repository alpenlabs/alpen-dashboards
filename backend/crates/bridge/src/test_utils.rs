//! Shared helpers for bridge tests.

use status_config::test_utils::{make_bridge_config, LOCAL_ESPLORA_URL};
use strata_retry::RetryConfig;

pub(crate) fn make_retry_config() -> RetryConfig {
    make_bridge_config(LOCAL_ESPLORA_URL, &[], Some(3)).retry_policy()
}
