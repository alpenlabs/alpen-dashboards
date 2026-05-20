use anyhow::{anyhow, Result};
use jsonrpsee::http_client::HttpClient;
use std::collections::BTreeMap;
use strata_bridge_primitives::types::DepositIdx;
use strata_bridge_rpc::traits::{StrataBridgeControlApiClient, StrataBridgeMonitoringApiClient};
use strata_bridge_rpc::types::{RpcDepositInfo, RpcOperatorStatus, RpcReimbursementStatus};
use tracing::{debug, warn};

use status_config::BridgeMonitoringConfig;
use status_utils::{create_rpc_client, execute_with_retries};

/// RPC client manager with connection pooling and retry logic.
///
/// This manager maintains a pool of reusable HTTP clients for each bridge RPC endpoint,
/// preventing connection exhaustion by reusing clients across requests. It implements
/// automatic retry logic with exponential backoff to handle transient network failures.
///
/// # Design
///
/// - **Connection Pooling**: Creates one HTTP client per configured endpoint and reuses it
/// - **Retry Logic**: Implements exponential backoff (3 retries over 10 seconds with 1.5x multiplier)
/// - **Failover**: Tries all available clients in deterministic key order
/// - **Graceful Degradation**: Returns `None` if all clients fail after retries
///
/// # Example Flow
///
/// For each RPC request:
///
/// 1. Try client 1 with up to 3 retries (exponential backoff between retries)
/// 2. If client 1 fails after retries, try client 2 with up to 3 retries
/// 3. Continue until a client succeeds or all fail
/// 4. Return the first successful result or [`None`] if all fail
pub(crate) struct RpcClientManager {
    /// HTTP clients keyed by configured client key.
    ///
    /// [`BTreeMap`] ensures deterministic ordering.
    clients: BTreeMap<String, HttpClient>,
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
            clients.insert(
                operator.public_key().to_string(),
                create_rpc_client(operator.rpc_url()),
            );
        }

        Self { clients }
    }

    /// Execute an async operation across all available clients with retry logic
    ///
    /// This method tries the given operation on each client sequentially in sorted key
    /// order. For each client, it retries up to 3 times with exponential
    /// backoff between attempts using [`execute_with_retries`]. The first successful result
    /// is returned immediately.
    ///
    /// # Type Parameters
    ///
    /// * `T` - The expected return type of the operation
    /// * `F` - A function that takes an [`HttpClient`] and returns a future
    /// * `Fut` - The future returned by `F`
    ///
    /// # Arguments
    ///
    /// * `operation` - A closure that performs an RPC call using the provided client
    ///
    /// # Returns
    ///
    /// * [`Some(T)`](Some) - If any client succeeds (possibly after retries)
    /// * [`None`] - If all clients fail after exhausting their retries
    ///
    /// # Retry Behavior
    ///
    /// Uses [`execute_with_retries`] for each client with exponential backoff:
    ///
    /// - **Attempt 0**: Immediate (no delay)
    /// - **Attempt 1**: After ~2s delay
    /// - **Attempt 2**: After ~3s delay
    /// - **Attempt 3**: After ~5s delay
    /// - Total: ~10 seconds per client
    ///
    /// # Example
    ///
    /// ```ignore
    /// let result = rpc_manager
    ///     .query_clients_with_retry(|client| async move {
    ///         client.get_deposit_indices().await.map_err(|e| e.into())
    ///     })
    ///     .await;
    /// ```
    async fn query_clients_with_retry<T, F, Fut>(&self, operation: F) -> Option<T>
    where
        F: Fn(HttpClient) -> Fut,
        Fut: std::future::Future<Output = Result<T, Box<dyn std::error::Error + Send + Sync>>>,
    {
        // BTreeMap maintains sorted order automatically.
        for (client_key, client) in self.clients.iter() {
            let client_clone = client.clone();
            let operation_name = format!("RPC request to bridge client {client_key}");

            match execute_with_retries(
                || {
                    let client = client_clone.clone();
                    operation(client)
                },
                &operation_name,
            )
            .await
            {
                Ok(result) => {
                    debug!(client_key = %client_key, "rpc request succeeded");
                    return Some(result);
                }
                Err(e) => {
                    warn!(
                        client_key = %client_key,
                        error = %e,
                        "rpc request failed after retries"
                    );
                    // Continue to next client.
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
    operator_public_key: &str,
) -> RpcOperatorStatus {
    let Some(client) = rpc_manager.client(operator_public_key) else {
        warn!(
            operator_pk = %operator_public_key,
            "missing bridge operator RPC client"
        );
        return RpcOperatorStatus::Offline;
    };

    match client.get_uptime().await {
        Ok(_) => RpcOperatorStatus::Online,
        Err(e) => {
            warn!(
                operator_pk = %operator_public_key,
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
