use crate::config::BalanceMonitoringConfig;
use crate::wallets::types::FaucetBalances;
use tracing::warn;

/// Convert wei to satoshis (BTC = ETH peg)
/// 1 ETH = 10^18 wei = 10^8 sats, so 1 wei = 10^-10 sats
fn wei_to_sats(wei: u128) -> u128 {
    wei / 10_000_000_000u128
}

/// Fetch faucet balances from L1 and L2 endpoints
pub(crate) async fn fetch_faucet_balances(config: &BalanceMonitoringConfig) -> FaucetBalances {
    let l1_balance = fetch_balance_from_url(config.faucet_l1_url()).await;
    let l2_balance_wei = fetch_balance_from_url(config.faucet_l2_url()).await;
    let l2_balance_sats = wei_to_sats(l2_balance_wei);

    FaucetBalances::new(l1_balance, l2_balance_sats)
}

/// Fetch balance from a faucet URL
async fn fetch_balance_from_url(url: &str) -> u128 {
    match reqwest::get(url).await {
        Ok(response) => {
            if response.status().is_success() {
                match response.text().await {
                    Ok(text) => match text.trim().parse::<u128>() {
                        Ok(balance) => balance,
                        Err(e) => {
                            warn!("Could not parse balance '{}' from {}: {}", text, url, e);
                            0
                        }
                    },
                    Err(e) => {
                        warn!("Failed to get text from {}: {}", url, e);
                        0
                    }
                }
            } else {
                warn!("HTTP error {} from {}", response.status(), url);
                0
            }
        }
        Err(e) => {
            warn!("Failed to fetch balance from {}: {}", url, e);
            0
        }
    }
}
