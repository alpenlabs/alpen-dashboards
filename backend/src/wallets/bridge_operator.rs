use tracing::warn;

use crate::config::BalanceMonitoringConfig;
use crate::wallets::types::{BridgeOperatorBalances, BridgeOperatorWalletBalance, WalletType};

/// Fetch bridge operator balances from Esplora
pub(crate) async fn fetch_bridge_operator_balances(
    config: &BalanceMonitoringConfig,
) -> BridgeOperatorBalances {
    let mut general_wallets = Vec::new();
    let mut stake_chain_wallets = Vec::new();

    // Fetch general wallet balances
    for (index, (pub_key, address)) in config
        .bridge_operator_general_addresses()
        .iter()
        .enumerate()
    {
        let operator_id = format!("Alpen Labs #{}", index + 1);
        let balance = fetch_bitcoin_balance(config.esplora_url(), address).await;

        general_wallets.push(BridgeOperatorWalletBalance::new(
            WalletType::General,
            operator_id,
            pub_key.clone(),
            balance,
        ));
    }

    // Fetch stake chain wallet balances
    for (index, (pub_key, address)) in config.bridge_operator_stake_addresses().iter().enumerate() {
        let operator_id = format!("Alpen Labs #{}", index + 1);
        let balance = fetch_bitcoin_balance(config.esplora_url(), address).await;

        stake_chain_wallets.push(BridgeOperatorWalletBalance::new(
            WalletType::StakeChain,
            operator_id,
            pub_key.clone(),
            balance,
        ));
    }

    BridgeOperatorBalances::new(general_wallets, stake_chain_wallets)
}

/// Fetch Bitcoin balance for an address from Esplora
async fn fetch_bitcoin_balance(esplora_url: &str, address: &str) -> u128 {
    let url = format!("{esplora_url}/address/{address}");

    match reqwest::get(&url).await {
        Ok(response) => {
            if response.status().is_success() {
                match response.json::<serde_json::Value>().await {
                    Ok(json) => {
                        // Esplora returns balance in the "chain_stats" field
                        if let Some(chain_stats) = json.get("chain_stats") {
                            if let Some(funded_txo_sum) =
                                chain_stats.get("funded_txo_sum").and_then(|v| v.as_u64())
                            {
                                if let Some(spent_txo_sum) =
                                    chain_stats.get("spent_txo_sum").and_then(|v| v.as_u64())
                                {
                                    return (funded_txo_sum - spent_txo_sum) as u128;
                                }
                            }
                        }
                        warn!(
                            "Could not parse balance from Esplora response for {}: {}",
                            address, json
                        );
                        0
                    }
                    Err(e) => {
                        warn!("Failed to parse JSON from Esplora for {}: {}", address, e);
                        0
                    }
                }
            } else {
                warn!(
                    "HTTP error {} from Esplora for address {}",
                    response.status(),
                    address
                );
                0
            }
        }
        Err(e) => {
            warn!(
                "Failed to fetch balance from Esplora for {}: {}",
                address, e
            );
            0
        }
    }
}
