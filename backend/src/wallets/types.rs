use crate::config::BalanceMonitoringConfig;
use serde::{Deserialize, Serialize};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tokio::sync::{Notify, RwLock};

/// Wallet type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum WalletType {
    General,
    StakeChain,
}

/// Individual wallet balance information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct BridgeOperatorWalletBalance {
    /// Wallet type
    wallet_type: WalletType,
    /// Operator ID (for bridge operators)
    operator_id: String,
    /// Operator public key
    operator_pk: String,
    /// Balance amount in satoshis (BTC and ETH pegged 1:1)
    balance_sats: u128,
}

impl BridgeOperatorWalletBalance {
    /// Create a new [`BridgeOperatorWalletBalance`]
    pub(crate) fn new(
        wallet_type: WalletType,
        operator_id: String,
        operator_pk: String,
        balance_sats: u128,
    ) -> Self {
        Self {
            wallet_type,
            operator_id,
            operator_pk,
            balance_sats,
        }
    }
}

/// Faucet wallet balances
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct FaucetBalances {
    /// L1 Bitcoin signet balance in satoshis
    l1_balance_sats: u128,
    /// L2 Alpen wallet balance in satoshis
    l2_balance_sats: u128,
}

impl FaucetBalances {
    /// Create a new [`FaucetBalances`]
    pub(crate) fn new(l1_balance_sats: u128, l2_balance_sats: u128) -> Self {
        Self {
            l1_balance_sats,
            l2_balance_sats,
        }
    }
}

/// Bridge operator wallet balances
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct BridgeOperatorBalances {
    /// General wallet balances keyed by operator public key
    general_wallets: Vec<BridgeOperatorWalletBalance>,
    /// Stake chain wallet balances keyed by operator public key
    stake_chain_wallets: Vec<BridgeOperatorWalletBalance>,
}

impl BridgeOperatorBalances {
    /// Create a new [`BridgeOperatorBalances`]
    pub(crate) fn new(
        general_wallets: Vec<BridgeOperatorWalletBalance>,
        stake_chain_wallets: Vec<BridgeOperatorWalletBalance>,
    ) -> Self {
        Self {
            general_wallets,
            stake_chain_wallets,
        }
    }
}

/// All wallet balances
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WalletBalances {
    /// Faucet wallet balances
    faucet: FaucetBalances,
    /// Bridge operator wallet balances
    bridge_operators: BridgeOperatorBalances,
}

impl WalletBalances {
    /// Create a new [`WalletBalances`]
    pub(crate) fn new(faucet: FaucetBalances, bridge_operators: BridgeOperatorBalances) -> Self {
        Self {
            faucet,
            bridge_operators,
        }
    }
}

impl Default for WalletBalances {
    fn default() -> Self {
        Self {
            faucet: FaucetBalances {
                l1_balance_sats: 0,
                l2_balance_sats: 0,
            },
            bridge_operators: BridgeOperatorBalances {
                general_wallets: Vec::new(),
                stake_chain_wallets: Vec::new(),
            },
        }
    }
}

/// Balance monitoring context that tracks loading state
#[derive(Debug)]
pub(crate) struct BalanceContext {
    /// The actual wallet balances
    pub(crate) balances: Arc<RwLock<WalletBalances>>,
    /// Whether balances are available (not loading)
    pub(crate) balances_available: Arc<AtomicBool>,
    /// Whether the initial balance query has completed
    pub(crate) initial_balance_query_complete: Arc<Notify>,
    /// Configuration for balance monitoring
    pub(crate) config: BalanceMonitoringConfig,
}

impl BalanceContext {
    /// Create new [`BalanceContext`]
    pub(crate) fn new(config: BalanceMonitoringConfig, balances: WalletBalances) -> Self {
        Self {
            balances: Arc::new(RwLock::new(balances)),
            balances_available: Arc::new(AtomicBool::new(false)),
            initial_balance_query_complete: Arc::new(Notify::new()),
            config,
        }
    }
}
