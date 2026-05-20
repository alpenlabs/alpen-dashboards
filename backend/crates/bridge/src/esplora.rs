use anyhow::Result;
use bitcoin::Txid;
use serde::Deserialize;
use std::time::Duration;
use strata_primitives::L1Height;
use tracing::error;

#[derive(Deserialize)]
struct TxStatus {
    confirmed: bool,
    block_height: Option<L1Height>,
}

pub(crate) struct EsploraClient {
    base_url: String,
    client: reqwest::Client,
}

impl EsploraClient {
    pub(crate) fn new(esplora_url: &str, request_timeout_s: u64) -> Self {
        Self {
            base_url: esplora_url.trim_end_matches('/').to_string(),
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(request_timeout_s))
                .build()
                .expect("failed to create Esplora HTTP client"),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    async fn get_tip_height(&self) -> reqwest::Result<String> {
        let resp = self
            .client
            .get(self.url("/blocks/tip/height"))
            .send()
            .await?;
        resp.text().await
    }

    async fn get_tx_status(&self, txid: Txid) -> Option<TxStatus> {
        let status_path = format!("/tx/{txid}/status");
        let status_resp = self.client.get(self.url(&status_path)).send().await;

        match status_resp {
            Ok(resp) => match resp.json().await {
                Ok(status) => Some(status),
                Err(e) => {
                    error!(%txid, error = %e, "failed to parse tx status JSON from esplora");
                    None
                }
            },
            Err(e) => {
                error!(%txid, error = %e, "failed to fetch tx status from esplora");
                None
            }
        }
    }
}

/// Fetch bitcoin chain tip height.
pub(crate) async fn get_bitcoin_chain_tip_height(
    esplora_client: &EsploraClient,
) -> Result<L1Height> {
    let text = esplora_client.get_tip_height().await?;
    let height = text.trim().parse::<L1Height>()?;
    Ok(height)
}

/// Get transaction confirmations from esplora.
pub(crate) async fn get_tx_confirmations(
    esplora_client: &EsploraClient,
    txid: Txid,
    chain_tip_height: L1Height,
) -> Option<u64> {
    let status = esplora_client.get_tx_status(txid).await?;

    status
        .block_height
        .filter(|_| status.confirmed)
        .map(|height| confirmations_from_block_height(chain_tip_height, height))
}

fn confirmations_from_block_height(chain_tip_height: L1Height, block_height: L1Height) -> u64 {
    u64::from(chain_tip_height.saturating_sub(block_height) + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn esplora_client_normalizes_base_url_once() {
        let client = EsploraClient::new("http://localhost:3002///", 5);

        assert_eq!(
            client.url("/blocks/tip/height"),
            "http://localhost:3002/blocks/tip/height"
        );
        assert_eq!(
            client.url("/tx/abc/status"),
            "http://localhost:3002/tx/abc/status"
        );
    }

    #[test]
    fn tx_status_deserializes_l1_height_from_esplora_json() {
        let status: TxStatus =
            serde_json::from_str(r#"{"confirmed":true,"block_height":12345}"#).unwrap();

        assert!(status.confirmed);
        assert_eq!(status.block_height, Some(12345 as L1Height));
    }

    #[test]
    fn confirmations_from_block_height_counts_tip_as_one_confirmation() {
        assert_eq!(confirmations_from_block_height(100, 100), 1);
        assert_eq!(confirmations_from_block_height(100, 98), 3);
    }

    #[test]
    fn confirmations_from_block_height_saturates_for_future_height() {
        assert_eq!(confirmations_from_block_height(100, 101), 1);
    }
}
