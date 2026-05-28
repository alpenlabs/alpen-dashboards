use crate::bridge_operator::BridgeOperatorBalances;
use crate::faucet::FaucetBalances;
use status_config::BalanceMonitoringConfig;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{Notify, RwLock};

/// Shared context for balance monitoring
#[derive(Debug)]
pub struct BalanceContext {
    config: BalanceMonitoringConfig,
    balances_available: AtomicBool,
    initial_balance_query_complete: Notify,
    balances: RwLock<WalletBalances>,
}

impl BalanceContext {
    pub fn new(config: BalanceMonitoringConfig) -> Self {
        Self {
            config,
            balances_available: AtomicBool::new(false),
            initial_balance_query_complete: Notify::new(),
            balances: RwLock::new(WalletBalances::default()),
        }
    }

    pub(crate) fn config(&self) -> &BalanceMonitoringConfig {
        &self.config
    }

    pub(crate) async fn set_balances(&self, balances: WalletBalances) {
        let mut locked_balances = self.balances.write().await;
        *locked_balances = balances;
    }

    pub(crate) async fn balances(&self) -> WalletBalances {
        self.balances.read().await.clone()
    }

    pub(crate) fn mark_balances_available(&self) {
        if self
            .balances_available
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            self.initial_balance_query_complete.notify_waiters();
        }
    }

    pub(crate) async fn wait_until_initial_balances(&self) {
        if self.balances_available.load(Ordering::Acquire) {
            return;
        }

        let notified = self.initial_balance_query_complete.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();

        if self.balances_available.load(Ordering::Acquire) {
            return;
        }

        notified.await;
    }
}

/// Combined wallet balances
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WalletBalances {
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
