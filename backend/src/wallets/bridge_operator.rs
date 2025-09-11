use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;

use crate::config::BridgeOperatorConfig;
use crate::wallets::errors::WalletBalanceError;
use crate::wallets::traits::{BalanceProvider, WalletBalance};

/// Wallet type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum WalletType {
    General,
    StakeChain,
}

/// Individual bridge operator wallet balance
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

    /// Get the wallet type
    pub(crate) fn wallet_type(&self) -> &WalletType {
        &self.wallet_type
    }
}

/// Bridge operator wallet balances
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct BridgeOperatorBalances {
    /// General wallet balances
    general_wallets: Vec<BridgeOperatorWalletBalance>,
    /// Stake chain wallet balances
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

/// Esplora balance provider for Bitcoin addresses
#[derive(Debug)]
pub(crate) struct EsploraProvider {
    esplora_url: String,
    address: String,
}

impl EsploraProvider {
    pub(crate) fn new(esplora_url: String, address: String) -> Self {
        Self {
            esplora_url,
            address,
        }
    }
}

#[async_trait]
impl BalanceProvider for EsploraProvider {
    type Balance = u128;
    type Error = WalletBalanceError;

    async fn fetch_balance(&self) -> Result<Self::Balance, Self::Error> {
        let url = format!("{}/address/{}", self.esplora_url, self.address);
        let response = reqwest::get(&url).await?;

        if !response.status().is_success() {
            return Err(WalletBalanceError::Config(format!(
                "HTTP error {} from Esplora for address {}",
                response.status(),
                self.address
            )));
        }

        let json: serde_json::Value = response.json().await?;

        // Esplora returns balance in the "chain_stats" field
        if let Some(chain_stats) = json.get("chain_stats") {
            if let Some(funded_txo_sum) = chain_stats.get("funded_txo_sum").and_then(|v| v.as_u64())
            {
                if let Some(spent_txo_sum) =
                    chain_stats.get("spent_txo_sum").and_then(|v| v.as_u64())
                {
                    return Ok((funded_txo_sum - spent_txo_sum) as u128);
                }
            }
        }

        Err(WalletBalanceError::BalanceFormat(format!(
            "Could not parse balance from Esplora response for {}: {json}",
            self.address
        )))
    }
}

/// Individual bridge operator wallet
#[derive(Debug)]
pub(crate) struct BridgeOperatorWallet {
    operator_id: String,
    operator_pk: String,
    wallet_type: WalletType,
    provider: EsploraProvider,
}

impl BridgeOperatorWallet {
    pub(crate) fn new(
        operator_id: String,
        operator_pk: String,
        wallet_type: WalletType,
        esplora_url: String,
        address: String,
    ) -> Self {
        let provider = EsploraProvider::new(esplora_url, address);

        Self {
            operator_id,
            operator_pk,
            wallet_type,
            provider,
        }
    }
}

#[async_trait]
impl WalletBalance for BridgeOperatorWallet {
    type Balance = BridgeOperatorWalletBalance;
    type Error = WalletBalanceError;

    async fn get_balance(&self) -> Result<Self::Balance, Self::Error> {
        let balance_sats = self.provider.fetch_balance().await?;

        Ok(BridgeOperatorWalletBalance::new(
            self.wallet_type.clone(),
            self.operator_id.clone(),
            self.operator_pk.clone(),
            balance_sats,
        ))
    }
}

/// Bridge operator wallets
#[derive(Debug)]
pub(crate) struct BridgeOperatorWallets {
    wallets: Vec<BridgeOperatorWallet>,
}

impl BridgeOperatorWallets {
    pub(crate) fn new(config: &BridgeOperatorConfig) -> Self {
        let mut wallets = Vec::new();

        // Create general wallets
        for (index, (pub_key_str, address)) in config.general_addresses().iter().enumerate() {
            let operator_id = format!("Alpen Labs #{}", index + 1);
            let wallet = BridgeOperatorWallet::new(
                operator_id,
                (*pub_key_str).clone(),
                WalletType::General,
                config.esplora_url().to_string(),
                (*address).clone(),
            );
            wallets.push(wallet);
        }

        // Create stake chain wallets
        for (index, (pub_key_str, address)) in config.stake_chain_addresses().iter().enumerate() {
            let operator_id = format!("Alpen Labs #{}", index + 1);
            let wallet = BridgeOperatorWallet::new(
                operator_id,
                (*pub_key_str).clone(),
                WalletType::StakeChain,
                config.esplora_url().to_string(),
                (*address).clone(),
            );
            wallets.push(wallet);
        }

        Self { wallets }
    }
}

#[async_trait]
impl WalletBalance for BridgeOperatorWallets {
    type Balance = BridgeOperatorBalances;
    type Error = WalletBalanceError;

    async fn get_balance(&self) -> Result<Self::Balance, Self::Error> {
        let mut general_wallets = Vec::new();
        let mut stake_chain_wallets = Vec::new();

        for wallet in &self.wallets {
            let balance = wallet.get_balance().await?;
            match balance.wallet_type() {
                WalletType::General => general_wallets.push(balance),
                WalletType::StakeChain => stake_chain_wallets.push(balance),
            }
        }

        Ok(BridgeOperatorBalances::new(
            general_wallets,
            stake_chain_wallets,
        ))
    }
}
