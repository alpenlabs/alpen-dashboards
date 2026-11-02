use anyhow::{anyhow, Result};
use bitcoin::PublicKey;
use jsonrpsee::http_client::HttpClient;
use std::{collections::BTreeMap, future::Future};
use strata_bridge_primitives::types::DepositIdx;
use strata_bridge_rpc::traits::{StrataBridgeControlApiClient, StrataBridgeMonitoringApiClient};
use strata_bridge_rpc::types::{
    RpcDepositInfo, RpcOperatorStatus, RpcReimbursementStatus, RpcWithdrawalStatus,
};
use strata_retry::{retry_with_backoff_async, RetryConfig};
use tracing::{debug, warn};

use status_config::BridgeMonitoringConfig;
use status_utils::create_rpc_client;

/// Pooled bridge RPC clients with bounded retries and deterministic endpoint failover.
///
/// Each endpoint uses the configured request retry policy before failing over.
/// Request execution time is separate from backoff delays; each request retains
/// the HTTP client's 30-second timeout.
pub(crate) struct RpcClientManager {
    /// HTTP clients keyed by configured client key.
    ///
    /// [`BTreeMap`] ensures deterministic ordering.
    clients: BTreeMap<String, HttpClient>,
    retry_config: RetryConfig,
}

impl RpcClientManager {
    /// Create a new RPC client manager for the configured bridge RPC endpoints.
    ///
    /// This initializes one HTTP client per configured endpoint with:
    ///
    /// - 30-second request timeout
    /// - 10MB max request size
    /// - Connection pooling enabled
    ///
    /// # Arguments
    ///
    /// * `config` - Bridge monitoring configuration containing RPC endpoints
    pub(crate) fn new(config: &BridgeMonitoringConfig) -> Self {
        let mut clients = BTreeMap::new();
        for operator in config.operators() {
            let operator_pk = operator.public_key().to_string();
            if let Some(rpc_url) = operator.rpc_url() {
                clients.insert(operator_pk, create_rpc_client(rpc_url));
            }
        }

        Self {
            clients,
            retry_config: config.retry_policy(),
        }
    }

    /// Retries each endpoint in key order, stopping at the first successful response.
    /// Returns `None` if no endpoint succeeds or none are configured.
    /// A successful optional `None` is returned as `Some(None)` without retry or failover.
    async fn query_clients_with_retry<T, F, Fut>(&self, operation: F) -> Option<T>
    where
        F: Fn(HttpClient) -> Fut,
        Fut: Future<Output = Result<T, Box<dyn std::error::Error + Send + Sync>>>,
    {
        for (client_key, client) in self.clients.iter() {
            let client_clone = client.clone();
            let operation_name = format!("RPC request to bridge client {client_key}");

            let backoff = self.retry_config.backoff();
            match retry_with_backoff_async(
                &operation_name,
                self.retry_config.max_retries,
                &backoff,
                || {
                    let client = client_clone.clone();
                    operation(client)
                },
            )
            .await
            {
                Ok(result) => {
                    debug!(client_key = %client_key, "rpc request succeeded");
                    return Some(result);
                }
                Err(e) => {
                    debug!(
                        client_key = %client_key,
                        error = %e,
                        "rpc request failed after retries"
                    );
                }
            }
        }

        None
    }

    /// Return the pooled client for a configured client key.
    pub(crate) fn client(&self, client_key: &str) -> Option<&HttpClient> {
        self.clients.get(client_key)
    }
}

/// Fetch operator status.
pub(crate) async fn get_operator_status(
    rpc_manager: &RpcClientManager,
    operator_pk: PublicKey,
) -> RpcOperatorStatus {
    let operator_pk_str = operator_pk.to_string();
    let Some(client) = rpc_manager.client(&operator_pk_str) else {
        return rpc_manager
            .query_clients_with_retry(|client| async move {
                client
                    .get_operator_status(operator_pk)
                    .await
                    .map_err(|e| e.into())
            })
            .await
            .unwrap_or_else(|| {
                warn!(
                    operator_pk = %operator_pk_str,
                    "failed to fetch bridge operator status from peer clients"
                );
                RpcOperatorStatus::Offline
            });
    };

    match client.get_uptime().await {
        Ok(_) => RpcOperatorStatus::Online,
        Err(e) => {
            warn!(
                operator_pk = %operator_pk_str,
                error = %e,
                "failed to fetch bridge operator uptime"
            );
            RpcOperatorStatus::Offline
        }
    }
}

/// Fetch all known deposit indices from bridge operators.
///
/// Uses the RPC client manager with retry logic to query operators for the
/// bridge's durable deposit index set.
pub(crate) async fn get_deposit_indices(rpc_manager: &RpcClientManager) -> Result<Vec<DepositIdx>> {
    rpc_manager
        .query_clients_with_retry(|client| async move {
            client.get_deposit_indices().await.map_err(|e| e.into())
        })
        .await
        .ok_or_else(|| anyhow!("failed to fetch deposit indices after retries"))
}

/// Fetch deposit details by bridge-side deposit index.
pub(crate) async fn get_deposit_info(
    rpc_manager: &RpcClientManager,
    deposit_idx: DepositIdx,
) -> Result<RpcDepositInfo> {
    rpc_manager
        .query_clients_with_retry(|client| async move {
            client
                .get_deposit_info(deposit_idx)
                .await
                .map_err(|e| e.into())
        })
        .await
        .ok_or_else(|| anyhow!("failed to fetch deposit info for deposit_idx {deposit_idx}"))
}

/// Fetch withdrawal status by bridge-side deposit index.
pub(crate) async fn get_withdrawal_status(
    rpc_manager: &RpcClientManager,
    deposit_idx: DepositIdx,
) -> Result<Option<RpcWithdrawalStatus>> {
    rpc_manager
        .query_clients_with_retry(|client| async move {
            client
                .get_withdrawal_status(deposit_idx)
                .await
                .map_err(|e| e.into())
        })
        .await
        .ok_or_else(|| anyhow!("failed to fetch withdrawal status for deposit_idx {deposit_idx}"))
}

/// Fetch reimbursement status by bridge-side deposit index.
pub(crate) async fn get_reimbursement_status(
    rpc_manager: &RpcClientManager,
    deposit_idx: DepositIdx,
) -> Result<Option<RpcReimbursementStatus>> {
    rpc_manager
        .query_clients_with_retry(|client| async move {
            client
                .get_reimbursement_status(deposit_idx)
                .await
                .map_err(|e| e.into())
        })
        .await
        .ok_or_else(|| {
            anyhow!("failed to fetch reimbursement status for deposit_idx {deposit_idx}")
        })
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use axum::{extract::State, routing::post, Json, Router};
    use bitcoin::secp256k1::{Secp256k1, SecretKey};
    use serde_json::{json, Value};

    use super::*;
    use crate::test_utils::{make_retry_config, TestHttpServer};

    struct RpcResponses {
        replies: VecDeque<Value>,
        methods: Vec<String>,
    }

    struct RpcFixture {
        server: TestHttpServer,
        responses: Arc<Mutex<RpcResponses>>,
    }

    impl RpcFixture {
        /// Serves replies in order, then repeats the final reply.
        async fn new(replies: Vec<Value>) -> Self {
            assert!(!replies.is_empty(), "RPC fixture needs a response");
            let responses = Arc::new(Mutex::new(RpcResponses {
                replies: replies.into(),
                methods: Vec::new(),
            }));
            let router = Router::new()
                .route("/", post(rpc_response))
                .with_state(responses.clone());
            Self {
                server: TestHttpServer::new(router).await,
                responses,
            }
        }

        fn attempts(&self) -> usize {
            self.responses
                .lock()
                .expect("RPC response lock")
                .methods
                .len()
        }

        fn methods(&self) -> Vec<String> {
            self.responses
                .lock()
                .expect("RPC response lock")
                .methods
                .clone()
        }
    }

    async fn rpc_response(
        State(responses): State<Arc<Mutex<RpcResponses>>>,
        Json(request): Json<Value>,
    ) -> Json<Value> {
        let mut responses = responses.lock().expect("RPC response lock");
        responses
            .methods
            .push(request["method"].as_str().expect("RPC method").to_owned());
        let mut reply = responses.replies.front().expect("RPC reply").clone();
        if responses.replies.len() > 1 {
            responses.replies.pop_front();
        }
        reply["jsonrpc"] = json!("2.0");
        reply["id"] = request["id"].clone();
        Json(reply)
    }

    fn make_error_reply() -> Value {
        json!({"error": {"code": -32000, "message": "temporarily unavailable"}})
    }

    fn make_rpc_manager(fixtures: &[&RpcFixture]) -> RpcClientManager {
        RpcClientManager {
            clients: fixtures
                .iter()
                .enumerate()
                .map(|(index, fixture)| {
                    (index.to_string(), create_rpc_client(fixture.server.url()))
                })
                .collect(),
            retry_config: make_retry_config(),
        }
    }

    fn make_configured_rpc_manager(fixture: &RpcFixture, max_retries: u16) -> RpcClientManager {
        let config: BridgeMonitoringConfig = toml::from_str(&format!(
            r#"
            esplora_url = "http://localhost:3000"
            max_tx_confirmations = 6
            status_refetch_interval_s = 1
            operators = [{{ name = "Operator", public_key = "{}", rpc_url = "{}" }}]
            [retry]
            max_retries = {max_retries}
            base_delay_ms = 1
            max_delay_ms = 1
        "#,
            make_operator_key(),
            fixture.server.url()
        ))
        .expect("parse configured retry budget");
        RpcClientManager::new(&config)
    }

    fn make_operator_key() -> PublicKey {
        let secret = SecretKey::from_slice(&[1; 32]).expect("test secret key");
        PublicKey::new(secret.public_key(&Secp256k1::new()))
    }

    #[tokio::test]
    async fn rpc_queries_stop_after_first_success() {
        let first = RpcFixture::new(vec![json!({"result": [1, 2]})]).await;
        let second = RpcFixture::new(vec![make_error_reply()]).await;
        let manager = make_rpc_manager(&[&first, &second]);

        assert_eq!(
            get_deposit_indices(&manager)
                .await
                .expect("deposit indices"),
            vec![1, 2]
        );
        assert_eq!(first.attempts(), 1);
        assert_eq!(second.attempts(), 0);
    }

    #[tokio::test]
    async fn rpc_queries_recover_after_failed_attempts() {
        // Two failures must be retried against the same endpoint.
        let fixture = RpcFixture::new(vec![
            make_error_reply(),
            make_error_reply(),
            json!({"result": [3]}),
        ])
        .await;
        let manager = make_rpc_manager(&[&fixture]);

        assert_eq!(
            get_deposit_indices(&manager).await.expect("retry recovery"),
            vec![3]
        );
        assert_eq!(fixture.attempts(), 3);
    }

    #[tokio::test]
    async fn rpc_queries_fail_over_after_exhausting_first_endpoint() {
        let first = RpcFixture::new(vec![make_error_reply()]).await;
        let second = RpcFixture::new(vec![json!({"result": [4]})]).await;
        let manager = make_rpc_manager(&[&first, &second]);

        assert_eq!(
            get_deposit_indices(&manager)
                .await
                .expect("endpoint failover"),
            vec![4]
        );
        // Each endpoint has its own budget: initial attempt plus three retries.
        assert_eq!(first.attempts(), 4);
        assert_eq!(second.attempts(), 1);
    }

    #[tokio::test]
    async fn rpc_queries_return_error_after_all_endpoints_are_exhausted() {
        let first = RpcFixture::new(vec![make_error_reply()]).await;
        let second = RpcFixture::new(vec![make_error_reply()]).await;
        let manager = make_rpc_manager(&[&first, &second]);

        assert_eq!(
            get_deposit_indices(&manager).await.unwrap_err().to_string(),
            "failed to fetch deposit indices after retries"
        );
        assert_eq!(first.attempts(), 4);
        assert_eq!(second.attempts(), 4);
    }

    #[tokio::test]
    async fn rpc_queries_return_missing_status_without_retry_or_failover() {
        // A valid null result must retain its meaning instead of being treated as an RPC error.
        let first = RpcFixture::new(vec![json!({"result": null})]).await;
        let second = RpcFixture::new(vec![make_error_reply()]).await;
        let manager = make_rpc_manager(&[&first, &second]);

        assert!(get_withdrawal_status(&manager, 0)
            .await
            .expect("valid absent status")
            .is_none());
        assert_eq!(first.attempts(), 1);
        assert_eq!(second.attempts(), 0);
    }

    #[tokio::test]
    async fn operator_without_endpoint_uses_peer_status_query() {
        let fixture = RpcFixture::new(vec![json!({"result": RpcOperatorStatus::Online})]).await;
        let manager = make_rpc_manager(&[&fixture]);

        assert!(matches!(
            get_operator_status(&manager, make_operator_key()).await,
            RpcOperatorStatus::Online
        ));
        assert_eq!(fixture.methods(), vec!["stratabridge_operatorStatus"]);
    }

    #[tokio::test]
    async fn operator_with_endpoint_queries_uptime_once_on_failure() {
        let fixture = RpcFixture::new(vec![make_error_reply()]).await;
        // Query retries are enabled, but the configured operator uses a single uptime call.
        let manager = make_configured_rpc_manager(&fixture, 3);

        assert!(matches!(
            get_operator_status(&manager, make_operator_key()).await,
            RpcOperatorStatus::Offline
        ));
        // The failed lookup made one uptime request and did not fall back to peer status.
        assert_eq!(fixture.methods(), vec!["stratabridge_uptime"]);
    }

    #[tokio::test]
    async fn rpc_queries_stop_after_configured_retry_budget_is_exhausted() {
        let fixture = RpcFixture::new(vec![make_error_reply()]).await;
        let manager = make_configured_rpc_manager(&fixture, 1);

        assert_eq!(
            get_deposit_indices(&manager).await.unwrap_err().to_string(),
            "failed to fetch deposit indices after retries"
        );
        assert_eq!(fixture.attempts(), 2);
    }
}
