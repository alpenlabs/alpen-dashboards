use serde::{Deserialize, Serialize};

/// Configuration for faucet balance monitoring.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FaucetBalanceConfig {
    /// L1 faucet URL.
    l1_url: String,
    /// L2 faucet URL.
    l2_url: String,
}

impl FaucetBalanceConfig {
    /// Returns the L1 faucet URL.
    pub fn l1_url(&self) -> &str {
        &self.l1_url
    }

    /// Returns the L2 faucet URL.
    pub fn l2_url(&self) -> &str {
        &self.l2_url
    }
}

/// Wallet addresses monitored for a single bridge operator.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BridgeOperatorBalance {
    /// Display name of the bridge operator.
    name: String,

    /// Public key of the bridge operator.
    public_key: String,

    /// General wallet address.
    general_address: String,

    /// Stake chain wallet address.
    stake_chain_address: String,
}

impl BridgeOperatorBalance {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn public_key(&self) -> &str {
        &self.public_key
    }

    pub fn general_address(&self) -> &str {
        &self.general_address
    }

    pub fn stake_chain_address(&self) -> &str {
        &self.stake_chain_address
    }
}

/// Configuration for bridge operator balance monitoring.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BridgeOperatorConfig {
    /// Esplora API URL.
    esplora_url: String,

    /// Operators whose wallet balances are monitored.
    operators: Vec<BridgeOperatorBalance>,
}

impl BridgeOperatorConfig {
    /// Returns the Esplora API URL.
    pub fn esplora_url(&self) -> &str {
        &self.esplora_url
    }

    /// Returns the monitored operators.
    pub fn operators(&self) -> &[BridgeOperatorBalance] {
        &self.operators
    }
}

/// Configuration for wallet balance monitoring.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BalanceMonitoringConfig {
    /// Faucet balance monitoring configuration.
    faucet: FaucetBalanceConfig,

    /// Bridge operator balance monitoring configuration.
    bridge_operators: BridgeOperatorConfig,

    /// Refresh interval in seconds.
    refresh_interval_s: u64,
}

impl BalanceMonitoringConfig {
    /// Returns the faucet balance monitoring configuration.
    pub fn faucet(&self) -> &FaucetBalanceConfig {
        &self.faucet
    }

    /// Returns the bridge operator balance monitoring configuration.
    pub fn bridge_operators(&self) -> &BridgeOperatorConfig {
        &self.bridge_operators
    }

    /// Returns the refresh interval in seconds.
    pub fn refresh_interval_s(&self) -> u64 {
        self.refresh_interval_s
    }
}
