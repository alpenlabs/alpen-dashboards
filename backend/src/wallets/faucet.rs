use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;

use crate::config::FaucetBalanceConfig;
use crate::wallets::errors::WalletBalanceError;
use crate::wallets::traits::{BalanceProvider, WalletBalance};

/// Number of weis in one satoshi (1 ETH = 1 BTC = 100,000,000 sats)
const WEIS_PER_SAT: u128 = 10_000_000_000;

/// Faucet wallet balances
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct FaucetBalances {
    /// Bitcoin signet balance in satoshis
    l1_balance_sats: u128,
    /// Alpen wallet balance in satoshis
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

/// L1 faucet balance provider
#[derive(Debug)]
pub(crate) struct FaucetL1BalanceProvider {
    url: String,
}

impl FaucetL1BalanceProvider {
    pub(crate) fn new(url: String) -> Self {
        Self { url }
    }
}

#[async_trait]
impl BalanceProvider for FaucetL1BalanceProvider {
    type Balance = u128;
    type Error = WalletBalanceError;

    async fn fetch_balance(&self) -> Result<Self::Balance, Self::Error> {
        let response = reqwest::get(&self.url).await?;

        if !response.status().is_success() {
            return Err(WalletBalanceError::Config(format!(
                "HTTP error {} from {}",
                response.status(),
                self.url
            )));
        }

        let text = response.text().await?;
        let balance = text.trim().parse::<u128>().map_err(|e| {
            WalletBalanceError::BalanceFormat(format!("Could not parse balance '{text}': {e}"))
        })?;

        Ok(balance)
    }
}

/// L2 faucet balance provider
#[derive(Debug)]
pub(crate) struct FaucetL2BalanceProvider {
    url: String,
}

impl FaucetL2BalanceProvider {
    pub(crate) fn new(url: String) -> Self {
        Self { url }
    }
}

#[async_trait]
impl BalanceProvider for FaucetL2BalanceProvider {
    type Balance = u128;
    type Error = WalletBalanceError;

    async fn fetch_balance(&self) -> Result<Self::Balance, Self::Error> {
        let response = reqwest::get(&self.url).await?;

        if !response.status().is_success() {
            return Err(WalletBalanceError::Config(format!(
                "HTTP error {} from {}",
                response.status(),
                self.url
            )));
        }

        let text = response.text().await?;
        let wei = text.trim().parse::<u128>().map_err(|e| {
            WalletBalanceError::BalanceFormat(format!("Could not parse balance '{text}': {e}"))
        })?;

        // Convert wei to satoshis (BTC = ETH peg)
        // 1 ETH = 10^18 wei = 10^8 sats, so 1 wei = 10^-10 sats
        let sats = wei / WEIS_PER_SAT;

        Ok(sats)
    }
}

/// Faucet watch-only wallet that combines L1 and L2 balances
#[derive(Debug)]
pub(crate) struct FaucetWalletBalance {
    l1_provider: FaucetL1BalanceProvider,
    l2_provider: FaucetL2BalanceProvider,
}

impl FaucetWalletBalance {
    pub(crate) fn new(config: &FaucetBalanceConfig) -> Self {
        let l1_provider = FaucetL1BalanceProvider::new(config.l1_url().to_string());
        let l2_provider = FaucetL2BalanceProvider::new(config.l2_url().to_string());

        Self {
            l1_provider,
            l2_provider,
        }
    }
}

#[async_trait]
impl WalletBalance for FaucetWalletBalance {
    type Balance = FaucetBalances;
    type Error = WalletBalanceError;

    async fn get_balance(&self) -> Result<Self::Balance, Self::Error> {
        let l1_balance = self.l1_provider.fetch_balance().await?;
        let l2_balance = self.l2_provider.fetch_balance().await?;

        Ok(FaucetBalances::new(l1_balance, l2_balance))
    }
}
