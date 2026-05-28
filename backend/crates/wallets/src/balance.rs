use crate::bridge_operator::{BridgeOperatorBalances, BridgeOperatorWallets};
use crate::context::{BalanceContext, WalletBalances};
use crate::faucet::{FaucetBalances, FaucetWalletBalance};
use crate::traits::WalletBalance;
use anyhow::Result;
use std::sync::Arc;
use strata_tasks::ShutdownGuard;
use tokio::time::{interval, Duration};
use tracing::info;

/// Background task that monitors all wallet balances
pub async fn balance_monitoring_task(
    context: Arc<BalanceContext>,
    shutdown: ShutdownGuard,
) -> Result<()> {
    info!("Starting wallet balance monitoring task");
    let refresh_interval = Duration::from_secs(context.config().refresh_interval_s());
    let mut interval = interval(refresh_interval);

    loop {
        tokio::select! {
            _ = shutdown.wait_for_shutdown() => break,
            _ = interval.tick() => {}
        }

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

        context.set_balances(new_balances).await;
        context.mark_balances_available();

        info!("Updated wallet balances using trait-based APIs");
    }

    Ok(())
}

/// API endpoint to get current balances
pub async fn get_balances(context: Arc<BalanceContext>) -> axum::Json<WalletBalances> {
    context.wait_until_initial_balances().await;

    axum::Json(context.balances().await)
}
