use crate::wallets::bridge_operator::{BridgeOperatorBalances, BridgeOperatorWallets};
use crate::wallets::context::{BalanceContext, WalletBalances};
use crate::wallets::faucet::{FaucetBalances, FaucetWalletBalance};
use crate::wallets::traits::WalletBalance;
use std::sync::{atomic::Ordering, Arc};
use tokio::time::{interval, Duration};
use tracing::info;

/// Background task that monitors all wallet balances
pub(crate) async fn balance_monitoring_task(context: Arc<BalanceContext>) {
    info!("Starting wallet balance monitoring task");
    let refresh_interval = Duration::from_secs(context.config().refresh_interval_s());
    let mut interval = interval(refresh_interval);

    loop {
        // Create wallet instances
        let faucet_wallet = FaucetWalletBalance::new(context.config().faucet());
        let bridge_operator_wallets =
            BridgeOperatorWallets::new(context.config().bridge_operators());

        // Fetch balances using trait-based APIs
        let faucet_balances = faucet_wallet
            .get_balance()
            .await
            .unwrap_or_else(|_| FaucetBalances::new(0, 0));
        let bridge_operator_balances = bridge_operator_wallets
            .get_balance()
            .await
            .unwrap_or_else(|_| BridgeOperatorBalances::new(Vec::new(), Vec::new()));

        let mut new_balances = WalletBalances::default();
        new_balances.set_faucet(faucet_balances);
        new_balances.set_bridge_operators(bridge_operator_balances);

        {
            let mut balances = context.balances().write().await;
            *balances = new_balances;
        }

        if !context.balances_available().load(Ordering::Acquire) {
            context.balances_available().store(true, Ordering::Release);
            context.initial_balance_query_complete().notify_waiters();
        }

        info!("Updated wallet balances using trait-based APIs");
        interval.tick().await;
    }
}

/// API endpoint to get current balances
pub(crate) async fn get_balances(context: Arc<BalanceContext>) -> axum::Json<WalletBalances> {
    // Check if balances are already available
    if !context.balances_available().load(Ordering::Acquire) {
        info!("Waiting for initial balance query to complete...");
        context.initial_balance_query_complete().notified().await;
    }

    let balances = context.balances().read().await;
    axum::Json(balances.clone())
}
