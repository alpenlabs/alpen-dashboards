use std::sync::{atomic::Ordering, Arc};
use tokio::time::{interval, Duration};
use tracing::info;

use super::{bridge_operator::fetch_bridge_operator_balances, faucet::fetch_faucet_balances};
use crate::wallets::types::{BalanceContext, WalletBalances};

/// Background task to monitor wallet balances (stub implementation returning zeros)
pub(crate) async fn balance_monitoring_task(context: Arc<BalanceContext>) {
    info!("Starting wallet balance monitoring task");

    let refresh_interval = Duration::from_secs(context.config.refresh_interval_s());
    let mut interval = interval(refresh_interval);

    loop {
        interval.tick().await;

        // Mark as loading before fetching
        context.balances_available.store(false, Ordering::Release);

        // Update faucet balances
        let faucet_balances = fetch_faucet_balances(&context.config).await;

        // Update bridge operator balances
        let bridge_operators = fetch_bridge_operator_balances(&context.config).await;

        // Update state with new balances
        let new_balances = WalletBalances::new(faucet_balances, bridge_operators);

        {
            let mut balances = context.balances.write().await;
            *balances = new_balances;
        }

        // Mark initial balance query as complete and notify waiters
        if !context.balances_available.load(Ordering::Acquire) {
            context.balances_available.store(true, Ordering::Release);
            context.initial_balance_query_complete.notify_waiters();
        }

        info!("Updated wallet balances");
    }
}

/// Get current wallet balances, waiting for initial query to complete
pub(crate) async fn get_balances(context: Arc<BalanceContext>) -> axum::Json<WalletBalances> {
    if !context.balances_available.load(Ordering::Acquire) {
        info!("Waiting for initial balance query to complete");

        let mut wait_interval = interval(Duration::from_secs(context.config.refresh_interval_s()));
        wait_interval.tick().await; // First tick completes immediately

        // Wait for either notification or interval timeout
        tokio::select! {
            _ = context.initial_balance_query_complete.notified() => {},
            _ = wait_interval.tick() => {},
        }
    }

    let balances = context.balances.read().await.clone();
    axum::Json(balances)
}
