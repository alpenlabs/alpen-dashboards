//! Bitcoin connections and block-observer runtime configuration.

use std::{
    num::{NonZeroU32, NonZeroU64},
    path::PathBuf,
};

use serde::{Deserialize, Serialize};
use strata_retry::RetryConfig;

use crate::retry::RequestRetryConfig;

const DEFAULT_REQUEST_TIMEOUT_S: u64 = 30;
const DEFAULT_POLL_INTERVAL_S: u64 = 10;
const DEFAULT_BLOCKS_PER_BATCH: u32 = 64;
const DEFAULT_REORG_SAFE_DEPTH: u32 = 6;

/// Bitcoin RPC authentication. Passwords are referenced by environment-variable name.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BitcoinAuth {
    Cookie {
        path: PathBuf,
    },
    UserPass {
        username: String,
        password_env: String,
    },
}

/// Bitcoin node connection, separate from observer policy.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BitcoinConfig {
    /// Bitcoin Core JSON-RPC access.
    client: BitcoinClientConfig,
}

impl BitcoinConfig {
    pub fn client(&self) -> &BitcoinClientConfig {
        &self.client
    }
}

/// Bitcoin Core RPC connection settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BitcoinClientConfig {
    /// JSON-RPC endpoint, without embedded credentials.
    url: String,

    /// Credentials supplied separately from the endpoint.
    auth: BitcoinAuth,

    /// Timeout for an individual RPC attempt.
    #[serde(default = "default_request_timeout_s")]
    request_timeout_s: NonZeroU64,

    /// Bounded RPC request retries.
    #[serde(default)]
    retry: RequestRetryConfig,
}

impl BitcoinClientConfig {
    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn auth(&self) -> &BitcoinAuth {
        &self.auth
    }

    pub fn request_timeout_s(&self) -> u64 {
        self.request_timeout_s.get()
    }

    pub fn retry_policy(&self) -> RetryConfig {
        self.retry.policy()
    }
}

/// Dashboard block-observer policy, independent of Bitcoin node connections.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BitcoinObserverConfig {
    /// Canonical-chain reconciliation interval.
    #[serde(default = "default_poll_interval_s")]
    poll_interval_s: NonZeroU64,

    /// Maximum blocks processed per batch.
    #[serde(default = "default_blocks_per_batch")]
    blocks_per_batch: NonZeroU32,

    /// Confirmation depth for deposit completion and retained reorganization history.
    #[serde(default = "default_reorg_safe_depth")]
    reorg_safe_depth: NonZeroU32,
}

impl BitcoinObserverConfig {
    pub fn poll_interval_s(&self) -> u64 {
        self.poll_interval_s.get()
    }

    pub fn blocks_per_batch(&self) -> u32 {
        self.blocks_per_batch.get()
    }

    pub fn reorg_safe_depth(&self) -> u32 {
        self.reorg_safe_depth.get()
    }
}

fn default_request_timeout_s() -> NonZeroU64 {
    NonZeroU64::new(DEFAULT_REQUEST_TIMEOUT_S).expect("nonzero timeout")
}

fn default_poll_interval_s() -> NonZeroU64 {
    NonZeroU64::new(DEFAULT_POLL_INTERVAL_S).expect("nonzero interval")
}

fn default_blocks_per_batch() -> NonZeroU32 {
    NonZeroU32::new(DEFAULT_BLOCKS_PER_BATCH).expect("nonzero batch")
}

fn default_reorg_safe_depth() -> NonZeroU32 {
    NonZeroU32::new(DEFAULT_REORG_SAFE_DEPTH).expect("nonzero reorganization depth")
}

#[cfg(test)]
mod tests {
    use super::{BitcoinAuth, BitcoinClientConfig, BitcoinConfig, BitcoinObserverConfig};

    fn make_client_toml() -> &'static str {
        r#"url = "http://127.0.0.1:18443"
auth = { kind = "cookie", path = "/bitcoin/.cookie" }"#
    }

    #[test]
    fn bitcoin_config_parses_client_settings() {
        let config: BitcoinConfig = toml::from_str(
            r#"
[client]
url = "http://127.0.0.1:18443"
auth = { kind = "cookie", path = "/bitcoin/.cookie" }
request_timeout_s = 7

[client.retry]
max_retries = 1
base_delay_ms = 10
max_delay_ms = 20
"#,
        )
        .expect("Bitcoin connections");

        assert_eq!(config.client().url(), "http://127.0.0.1:18443");
        assert_eq!(config.client().request_timeout_s(), 7);
        let retry = config.client().retry_policy();
        assert_eq!(retry.max_retries, 1);
        assert_eq!(retry.base_delay_ms, 10);
        assert_eq!(retry.max_delay_ms, 20);
    }

    #[test]
    fn bitcoin_config_rejects_zmq_settings() {
        let input = format!("[client]\n{}\n[zmq]\n", make_client_toml());
        let error = toml::from_str::<BitcoinConfig>(&input).expect_err("removed ZMQ config");
        assert!(error.to_string().contains("unknown field `zmq`"));
    }

    #[test]
    fn bitcoin_client_config_uses_cookie_auth_and_request_defaults() {
        let config: BitcoinClientConfig =
            toml::from_str(make_client_toml()).expect("Bitcoin RPC config");
        assert_eq!(
            config.auth(),
            &BitcoinAuth::Cookie {
                path: "/bitcoin/.cookie".into()
            }
        );
        assert_eq!(config.request_timeout_s(), 30);
    }

    #[test]
    fn bitcoin_client_config_rejects_zero_request_timeout() {
        let input = format!("{}\nrequest_timeout_s = 0", make_client_toml());
        assert!(toml::from_str::<BitcoinClientConfig>(&input).is_err());
    }

    #[test]
    fn bitcoin_client_config_uses_password_environment_reference() {
        let input = r#"url = "http://127.0.0.1:18443"
[auth]
kind = "user_pass"
username = "bitcoin"
password_env = "DASHBOARD_BITCOIN_PASSWORD"
"#;
        let config: BitcoinClientConfig = toml::from_str(input).expect("credential reference");
        assert_eq!(
            config.auth(),
            &BitcoinAuth::UserPass {
                username: "bitcoin".to_owned(),
                password_env: "DASHBOARD_BITCOIN_PASSWORD".to_owned(),
            }
        );
    }

    #[test]
    fn bitcoin_observer_config_uses_bounded_scan_defaults() {
        let config: BitcoinObserverConfig = toml::from_str("").expect("observer defaults");
        assert_eq!(config.poll_interval_s(), 10);
        assert_eq!(config.blocks_per_batch(), 64);
        assert_eq!(config.reorg_safe_depth(), 6);
    }

    #[test]
    fn bitcoin_observer_config_rejects_zero_scan_limits() {
        for field in ["poll_interval_s", "blocks_per_batch", "reorg_safe_depth"] {
            let input = format!("{field} = 0");
            assert!(
                toml::from_str::<BitcoinObserverConfig>(&input).is_err(),
                "{field}"
            );
        }
    }

    #[test]
    fn bitcoin_config_rejects_observer_settings_in_connection_config() {
        let input = format!("blocks_per_batch = 4\n[client]\n{}", make_client_toml());
        assert!(toml::from_str::<BitcoinConfig>(&input).is_err());
    }

    #[test]
    fn bitcoin_observer_config_rejects_removed_settings() {
        for field in ["start_height", "reorg_window"] {
            let input = format!("{field} = 12");
            let error = toml::from_str::<BitcoinObserverConfig>(&input)
                .expect_err("removed observer setting");
            assert!(
                error
                    .to_string()
                    .contains(&format!("unknown field `{field}`")),
                "{error}"
            );
        }
    }

    #[test]
    fn bitcoin_client_config_requires_authentication() {
        let input = r#"url = "http://127.0.0.1:18443""#;
        let error = toml::from_str::<BitcoinClientConfig>(input).expect_err("missing auth");
        assert!(error.to_string().contains("missing field `auth`"));

        let input = format!("{input}\nauth = {{ kind = \"none\" }}");
        let error = toml::from_str::<BitcoinClientConfig>(&input).expect_err("unsupported auth");
        assert!(error.to_string().contains("unknown variant `none`"));
    }
}
