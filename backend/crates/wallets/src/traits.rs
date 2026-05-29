use async_trait::async_trait;
use std::error::Error;
use std::fmt::Debug;

/// Represents a wallet balance that can be queried
#[async_trait]
pub(crate) trait WalletBalance: Send + Sync + Debug {
    /// The type of balance this wallet returns
    type Balance: Send + Sync + Debug;

    /// The type of error that can occur when querying this wallet
    type Error: Error + Send + Sync + Debug;

    /// Get the current balance
    async fn get_balance(&self) -> Result<Self::Balance, Self::Error>;
}

/// Represents a balance provider that can fetch balances from external sources
#[async_trait]
pub(crate) trait BalanceProvider: Send + Sync + Debug {
    /// The type of balance this provider returns
    type Balance: Send + Sync + Debug;

    /// The type of error that can occur when fetching balances
    type Error: Error + Send + Sync + Debug;

    /// Fetch balance from the external source
    async fn fetch_balance(&self) -> Result<Self::Balance, Self::Error>;
}
