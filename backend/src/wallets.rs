use axum::Json;
use jsonrpsee::core::client::ClientT;
use jsonrpsee::http_client::HttpClient;
use serde::Serialize;
use serde_json::json;
use std::sync::Arc;
use tokio::{
    sync::RwLock,
    time::{interval, Duration},
};
use tracing::info;

use crate::config::NetworkConfig;
use crate::utils::rpc_client::create_rpc_client;

pub type SharedWallets = Arc<RwLock<PaymasterWallets>>;
#[derive(Clone, Debug, Serialize)]
pub struct Wallet {
    /// Wallet address
    address: String,
    /// Wallet balance in Wei
    balance: String,
}

impl Wallet {
    pub fn new(address: String, balance: String) -> Self {
        Self { address, balance }
    }

    pub fn update_balance(&mut self, balance: String) {
        self.balance = balance;
    }
}

#[derive(Debug, Serialize)]
pub struct PaymasterWallets {
    /// Deposit paymaster wallet
    deposit: Wallet,
    /// Validating paymaster wallet
    validating: Wallet,
}
impl PaymasterWallets {
    pub fn new(deposit: Wallet, validating: Wallet) -> Self {
        Self {
            deposit,
            validating,
        }
    }
}

/// Periodically fetches wallet balances
pub async fn fetch_balances_task(wallets: SharedWallets, config: &NetworkConfig) {
    info!("Fetching balances...");
    let mut interval = interval(Duration::from_secs(10));
    let rpc_client = create_rpc_client(config.reth_url());

    loop {
        interval.tick().await;

        let mut locked_wallets = wallets.write().await;

        let deposit_wallet = &mut locked_wallets.deposit;
        let balance_dep = fetch_wallet_balance(&rpc_client, &deposit_wallet.address).await;
        deposit_wallet.update_balance(balance_dep.clone().unwrap_or_else(|| "0".to_string()));

        let validating_wallet = &mut locked_wallets.validating;
        let balance_val = fetch_wallet_balance(&rpc_client, &validating_wallet.address).await;
        validating_wallet.update_balance(balance_val.clone().unwrap_or_else(|| "0".to_string()));
    }
}

/// Fetches the ETH balance of a given wallet address in Wei (integer)
pub async fn fetch_wallet_balance(client: &HttpClient, wallet_address: &str) -> Option<String> {
    info!(%wallet_address, "Fetching balance for wallet");

    let params = (wallet_address, "latest"); // ✅ Use a tuple instead of `serde_json::Value`
    let response: Result<serde_json::Value, _> = client.request("eth_getBalance", params).await;

    match response {
        Ok(json) => {
            if let Some(balance_hex) = json
                .as_str()
                .and_then(|s| s.strip_prefix("0x"))
                .and_then(|s| u128::from_str_radix(s, 16).ok())
            {
                return Some(balance_hex.to_string());
            }
        }
        Err(e) => {
            info!(%e, "Error fetching balance");
        }
    }
    None
}

/// Handler to fetch ETH wallet balances
pub async fn get_wallets_with_balances(wallets: SharedWallets) -> Json<serde_json::Value> {
    let locked_wallets = wallets.read().await;
    Json(json!({ "wallets": *locked_wallets }))
}

pub fn init_paymaster_wallets(config: &NetworkConfig) -> SharedWallets {
    let deposit = Wallet::new(config.deposit_wallet().to_string(), "0".to_string());
    let validating = Wallet::new(config.validating_wallet().to_string(), "0".to_string());
    Arc::new(RwLock::new(PaymasterWallets::new(deposit, validating))) // ✅ Returns tokio::sync::Mutex
}
