use anyhow::Result;
use bitcoin::Txid;
use reqwest::StatusCode;
use serde::Deserialize;
use std::{future::Future, num::ParseIntError, time::Duration};
use strata_primitives::L1Height;
use strata_retry::{retry_with_backoff_async, RetryConfig};
use thiserror::Error;
use tracing::warn;

#[derive(Deserialize)]
struct TxStatus {
    confirmed: bool,
    block_height: Option<L1Height>,
}

#[derive(Debug, Error)]
enum EsploraError {
    #[error("Esplora request failed: {0}")]
    Request(#[from] reqwest::Error),

    #[error("invalid Esplora chain tip height: {0}")]
    InvalidHeight(#[from] ParseIntError),

    #[error("confirmed Esplora transaction has no block height")]
    MissingBlockHeight,
}

impl EsploraError {
    fn is_retryable(&self) -> bool {
        match self {
            Self::Request(error) => {
                error.is_timeout()
                    || error.is_connect()
                    || error.is_body()
                    || error.status().is_some_and(|status| {
                        status == StatusCode::REQUEST_TIMEOUT
                            || status == StatusCode::TOO_MANY_REQUESTS
                            || status.is_server_error()
                    })
            }
            Self::InvalidHeight(_) | Self::MissingBlockHeight => false,
        }
    }
}

pub(crate) struct EsploraClient {
    base_url: String,
    client: reqwest::Client,
    retry_config: RetryConfig,
}

impl EsploraClient {
    pub(crate) fn new(
        esplora_url: &str,
        request_timeout_s: u64,
        retry_config: RetryConfig,
    ) -> Self {
        Self {
            base_url: esplora_url.trim_end_matches('/').to_string(),
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(request_timeout_s))
                .build()
                .expect("failed to create Esplora HTTP client"),
            retry_config,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    /// Only transient errors enter the retry helper; terminal failures pass through unchanged.
    async fn request_with_retry<T, F, Fut>(
        &self,
        name: &str,
        operation: F,
    ) -> Result<T, EsploraError>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = Result<T, EsploraError>>,
    {
        let backoff = self.retry_config.backoff();
        let outcome =
            retry_with_backoff_async(name, self.retry_config.max_retries, &backoff, || async {
                match operation().await {
                    Err(error) if error.is_retryable() => Err(error),
                    outcome => Ok(outcome),
                }
            })
            .await?;
        if let Err(error) = &outcome {
            warn!(operation = name, error = %error, "Esplora request failed");
        }
        outcome
    }

    async fn get_tip_height(&self) -> Result<L1Height, EsploraError> {
        self.request_with_retry("esplora chain tip", || async {
            let response = self
                .client
                .get(self.url("/blocks/tip/height"))
                .send()
                .await?
                .error_for_status()?;
            let text = response.text().await?;
            Ok(text.trim().parse()?)
        })
        .await
    }

    async fn get_tx_status(&self, txid: Txid) -> Result<TxStatus, EsploraError> {
        let operation_name = format!("esplora transaction {txid} status");
        self.request_with_retry(&operation_name, || async {
            let response = self
                .client
                .get(self.url(&format!("/tx/{txid}/status")))
                .send()
                .await?
                .error_for_status()?;
            let status: TxStatus = response.json().await?;
            if status.confirmed && status.block_height.is_none() {
                return Err(EsploraError::MissingBlockHeight);
            }
            Ok(status)
        })
        .await
    }
}

/// Fetch bitcoin chain tip height.
pub(crate) async fn get_bitcoin_chain_tip_height(
    esplora_client: &EsploraClient,
) -> Result<L1Height> {
    Ok(esplora_client.get_tip_height().await?)
}

/// Gets transaction confirmations from Esplora; mempool transactions return `Some(0)`.
/// Returns `None` if the lookup fails or a confirmed transaction has no block height.
pub(crate) async fn get_tx_confirmations(
    esplora_client: &EsploraClient,
    txid: Txid,
    chain_tip_height: L1Height,
) -> Option<u64> {
    match esplora_client.get_tx_status(txid).await {
        Ok(status) => confirmations_from_status(&status, chain_tip_height),
        Err(_) => None,
    }
}

/// An unconfirmed (mempool) transaction has zero confirmations.
fn confirmations_from_status(status: &TxStatus, chain_tip_height: L1Height) -> Option<u64> {
    if !status.confirmed {
        return Some(0);
    }
    status
        .block_height
        .map(|height| confirmations_from_block_height(chain_tip_height, height))
}

fn confirmations_from_block_height(chain_tip_height: L1Height, block_height: L1Height) -> u64 {
    u64::from(chain_tip_height.saturating_sub(block_height) + 1)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use axum::{extract::State, http::Uri, routing::get, Router};
    use bitcoin::hashes::Hash;
    use status_config::test_utils::make_bridge_config;
    use status_utils::test_utils::TestHttpServer;
    use tokio::time::sleep;

    use super::*;
    use crate::test_utils::make_retry_config;

    struct HttpResponses {
        replies: VecDeque<(StatusCode, &'static str)>,
        paths: Vec<String>,
        delay: Duration,
    }

    struct EsploraFixture {
        server: TestHttpServer,
        responses: Arc<Mutex<HttpResponses>>,
    }

    impl EsploraFixture {
        async fn new(replies: Vec<(StatusCode, &'static str)>) -> Self {
            assert!(!replies.is_empty(), "Esplora fixture needs a response");
            let responses = Arc::new(Mutex::new(HttpResponses {
                replies: replies.into(),
                paths: Vec::new(),
                delay: Duration::ZERO,
            }));
            let router = Router::new()
                .route("/blocks/tip/height", get(esplora_response))
                .route("/tx/{txid}/status", get(esplora_response))
                .with_state(responses.clone());
            Self {
                server: TestHttpServer::new(router).await,
                responses,
            }
        }

        fn client(&self) -> EsploraClient {
            EsploraClient::new(self.server.url(), 5, make_retry_config())
        }

        fn attempts(&self) -> usize {
            self.responses
                .lock()
                .expect("Esplora response lock")
                .paths
                .len()
        }

        fn paths(&self) -> Vec<String> {
            self.responses
                .lock()
                .expect("Esplora response lock")
                .paths
                .clone()
        }

        fn delay_responses(&self, delay: Duration) {
            self.responses.lock().expect("Esplora response lock").delay = delay;
        }
    }

    async fn esplora_response(
        State(responses): State<Arc<Mutex<HttpResponses>>>,
        uri: Uri,
    ) -> (StatusCode, &'static str) {
        let (reply, delay) = {
            let mut responses = responses.lock().expect("Esplora response lock");
            responses.paths.push(uri.path().to_owned());
            let reply = *responses.replies.front().expect("Esplora reply");
            if responses.replies.len() > 1 {
                responses.replies.pop_front();
            }
            (reply, responses.delay)
        };
        if !delay.is_zero() {
            sleep(delay).await;
        }
        reply
    }

    fn make_txid() -> Txid {
        Txid::from_byte_array([1; 32])
    }

    #[test]
    fn esplora_client_normalizes_base_url_once() {
        let client = EsploraClient::new("http://localhost:3002///", 5, make_retry_config());

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

    #[tokio::test]
    async fn esplora_tip_retries_transient_http_failures() {
        for status in [
            StatusCode::REQUEST_TIMEOUT,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::SERVICE_UNAVAILABLE,
        ] {
            let fixture =
                EsploraFixture::new(vec![(status, "unavailable"), (StatusCode::OK, "100\n")]).await;
            assert_eq!(
                get_bitcoin_chain_tip_height(&fixture.client())
                    .await
                    .expect("tip after retry"),
                100
            );
            assert_eq!(fixture.paths(), vec!["/blocks/tip/height"; 2]);
        }
    }

    #[tokio::test]
    async fn esplora_transaction_status_retries_transient_http_failures() {
        for status in [
            StatusCode::REQUEST_TIMEOUT,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::SERVICE_UNAVAILABLE,
        ] {
            let fixture = EsploraFixture::new(vec![
                (status, "unavailable"),
                (StatusCode::OK, r#"{"confirmed":true,"block_height":98}"#),
            ])
            .await;
            assert_eq!(
                get_tx_confirmations(&fixture.client(), make_txid(), 100).await,
                Some(3)
            );
            assert_eq!(
                fixture.paths(),
                vec![format!("/tx/{}/status", make_txid()); 2]
            );
        }
    }

    #[tokio::test]
    async fn esplora_tip_returns_error_after_retry_exhaustion() {
        let fixture = EsploraFixture::new(vec![(StatusCode::INTERNAL_SERVER_ERROR, "100")]).await;
        let error = fixture
            .client()
            .get_tip_height()
            .await
            .expect_err("tip request must fail");
        assert!(
            matches!(error, EsploraError::Request(error) if error.status() == Some(StatusCode::INTERNAL_SERVER_ERROR))
        );
        assert_eq!(fixture.attempts(), 4);
    }

    #[tokio::test]
    async fn esplora_transaction_status_returns_unknown_after_retry_exhaustion() {
        // Exhaustion must not be converted into zero confirmations.
        let fixture =
            EsploraFixture::new(vec![(StatusCode::INTERNAL_SERVER_ERROR, "unavailable")]).await;
        assert_eq!(
            get_tx_confirmations(&fixture.client(), make_txid(), 100).await,
            None
        );
        assert_eq!(fixture.attempts(), 4);
    }

    #[tokio::test]
    async fn esplora_tip_does_not_retry_permanent_http_failures() {
        for status in [
            StatusCode::BAD_REQUEST,
            StatusCode::UNAUTHORIZED,
            StatusCode::FORBIDDEN,
            StatusCode::NOT_FOUND,
        ] {
            // Even a numeric error body cannot be mistaken for a valid height.
            let fixture = EsploraFixture::new(vec![(status, "100")]).await;
            let error = fixture
                .client()
                .get_tip_height()
                .await
                .expect_err("HTTP failure");
            assert!(
                matches!(error, EsploraError::Request(error) if error.status() == Some(status))
            );
            assert_eq!(fixture.attempts(), 1);
        }
    }

    #[tokio::test]
    async fn esplora_transaction_status_does_not_retry_permanent_http_failures() {
        for status in [
            StatusCode::BAD_REQUEST,
            StatusCode::UNAUTHORIZED,
            StatusCode::FORBIDDEN,
            StatusCode::NOT_FOUND,
        ] {
            let fixture = EsploraFixture::new(vec![(status, r#"{"confirmed":false}"#)]).await;
            assert_eq!(
                get_tx_confirmations(&fixture.client(), make_txid(), 100).await,
                None
            );
            assert_eq!(fixture.attempts(), 1);
        }
    }

    #[tokio::test]
    async fn esplora_tip_does_not_retry_invalid_height() {
        let fixture = EsploraFixture::new(vec![(StatusCode::OK, "not a height")]).await;
        let error = fixture
            .client()
            .get_tip_height()
            .await
            .expect_err("invalid height");
        assert!(matches!(error, EsploraError::InvalidHeight(_)));
        assert_eq!(fixture.attempts(), 1);
    }

    #[tokio::test]
    async fn esplora_transaction_status_does_not_retry_invalid_payloads() {
        for body in ["not JSON", r#"{"confirmed":true}"#] {
            let fixture = EsploraFixture::new(vec![(StatusCode::OK, body)]).await;
            assert_eq!(
                get_tx_confirmations(&fixture.client(), make_txid(), 100).await,
                None
            );
            assert_eq!(fixture.attempts(), 1);
        }
    }

    #[tokio::test]
    async fn esplora_transaction_status_returns_zero_for_mempool_response() {
        let fixture = EsploraFixture::new(vec![(StatusCode::OK, r#"{"confirmed":false}"#)]).await;
        assert_eq!(
            get_tx_confirmations(&fixture.client(), make_txid(), 100).await,
            Some(0)
        );
        assert_eq!(fixture.attempts(), 1);
    }

    #[tokio::test]
    async fn esplora_request_timeouts_are_retried() {
        // The server holds each request past the per-attempt timeout.
        let fixture = EsploraFixture::new(vec![(StatusCode::OK, r#"{"confirmed":false}"#)]).await;
        fixture.delay_responses(Duration::from_millis(250));
        let mut client = fixture.client();
        client.client = reqwest::Client::builder()
            .timeout(Duration::from_millis(50))
            .build()
            .expect("short request timeout");

        assert_eq!(get_tx_confirmations(&client, make_txid(), 100).await, None);
        assert_eq!(fixture.attempts(), 4);
    }

    #[tokio::test]
    async fn esplora_transaction_status_recovers_on_later_poll_after_exhaustion() {
        // Four errors exhaust one request; the next polling cycle gets a fresh budget.
        let fixture = EsploraFixture::new(vec![
            (StatusCode::INTERNAL_SERVER_ERROR, "unavailable"),
            (StatusCode::INTERNAL_SERVER_ERROR, "unavailable"),
            (StatusCode::INTERNAL_SERVER_ERROR, "unavailable"),
            (StatusCode::INTERNAL_SERVER_ERROR, "unavailable"),
            (StatusCode::OK, r#"{"confirmed":true,"block_height":98}"#),
        ])
        .await;
        let client = fixture.client();
        assert_eq!(get_tx_confirmations(&client, make_txid(), 100).await, None);
        assert_eq!(fixture.attempts(), 4);
        assert_eq!(
            get_tx_confirmations(&client, make_txid(), 100).await,
            Some(3)
        );
        assert_eq!(fixture.attempts(), 5);
    }

    #[tokio::test]
    async fn esplora_requests_use_configured_retry_budget() {
        let fixture =
            EsploraFixture::new(vec![(StatusCode::INTERNAL_SERVER_ERROR, "unavailable")]).await;
        let config = make_bridge_config(fixture.server.url(), &[], Some(1));
        let client = EsploraClient::new(
            config.esplora_url(),
            config.esplora_request_timeout_s(),
            config.retry_policy(),
        );

        // One configured retry permits two attempts, independently for every request.
        assert!(get_bitcoin_chain_tip_height(&client).await.is_err());
        assert_eq!(fixture.attempts(), 2);
        assert_eq!(get_tx_confirmations(&client, make_txid(), 100).await, None);
        assert_eq!(fixture.attempts(), 4);
    }
}
