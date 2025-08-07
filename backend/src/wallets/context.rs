use crate::config::BalanceMonitoringConfig;
use crate::wallets::bridge_operator::BridgeOperatorBalances;
use crate::wallets::faucet::FaucetBalances;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tokio::sync::{Notify, RwLock};

/// Shared context for balance monitoring
#[derive(Debug)]
pub(crate) struct BalanceContext {
    balances: Arc<RwLock<WalletBalances>>,
    balances_available: Arc<AtomicBool>,
    initial_balance_query_complete: Arc<Notify>,
    config: BalanceMonitoringConfig,
}

impl BalanceContext {
    pub(crate) fn new(config: BalanceMonitoringConfig) -> Self {
        Self {
            balances: Arc::new(RwLock::new(WalletBalances::default())),
            balances_available: Arc::new(AtomicBool::new(false)),
            initial_balance_query_complete: Arc::new(Notify::new()),
            config,
        }
    }

    pub(crate) fn balances(&self) -> &Arc<RwLock<WalletBalances>> {
        &self.balances
    }

    pub(crate) fn balances_available(&self) -> &Arc<AtomicBool> {
        &self.balances_available
    }

    pub(crate) fn initial_balance_query_complete(&self) -> &Arc<Notify> {
        &self.initial_balance_query_complete
    }

    pub(crate) fn config(&self) -> &BalanceMonitoringConfig {
        &self.config
    }
}

/// Combined wallet balances
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct WalletBalances {
    faucet: FaucetBalances,
    bridge_operators: BridgeOperatorBalances,
}

impl Default for WalletBalances {
    fn default() -> Self {
        Self {
            faucet: FaucetBalances::new(0, 0),
            bridge_operators: BridgeOperatorBalances::new(Vec::new(), Vec::new()),
        }
    }
}

impl WalletBalances {
    pub(crate) fn set_faucet(&mut self, faucet: FaucetBalances) {
        self.faucet = faucet;
    }

    pub(crate) fn set_bridge_operators(&mut self, bridge_operators: BridgeOperatorBalances) {
        self.bridge_operators = bridge_operators;
    }
}
