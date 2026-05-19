use anyhow::{anyhow, Result};
use jsonrpsee::http_client::HttpClient;
use std::collections::BTreeMap;
use strata_bridge_primitives::types::DepositIdx;
use strata_bridge_rpc::traits::{StrataBridgeControlApiClient, StrataBridgeMonitoringApiClient};
use strata_bridge_rpc::types::{RpcDepositInfo, RpcOperatorStatus, RpcReimbursementStatus};
use tracing::{debug, warn};

use status_config::BridgeMonitoringConfig;
use status_utils::{create_rpc_client, execute_with_retries};

/// RPC client manager with connection pooling and retry logic
///
/// This manager maintains a pool of reusable HTTP clients for each bridge operator,
/// preventing connection exhaustion by reusing clients across requests. It implements
/// automatic retry logic with exponential backoff to handle transient network failures.
///
/// # Design
///
/// - **Connection Pooling**: Creates one HTTP client per operator and reuses it for all requests
/// - **Retry Logic**: Implements exponential backoff (3 retries over 10 seconds with 1.5x multiplier)
/// - **Failover**: Tries all available operators in deterministic order (sorted by public key)
/// - **Graceful Degradation**: Returns `None` if all operators fail after retries
///
/// # Example Flow
///
/// For each RPC request:
///
/// 1. Try operator 1 with up to 3 retries (exponential backoff between retries)
/// 2. If operator 1 fails after retries, try operator 2 with up to 3 retries
/// 3. Continue until an operator succeeds or all fail
/// 4. Return the first successful result or [`None`] if all fail
pub(crate) struct RpcClientManager {
    /// HTTP clients for each operator, keyed by operator public key ([`String`])
    /// [`BTreeMap`] ensures deterministic ordering (sorted by key)
    clients: BTreeMap<String, HttpClient>,
}

impl RpcClientManager {
    /// Create a new RPC client manager for the configured bridge operators
    ///
    /// This initializes one HTTP client per operator with:
    ///
    /// - 30-second request timeout
    /// - 10MB max request size
    /// - Connection pooling enabled
    ///
    /// # Arguments
    ///
    /// * `config` - Bridge monitoring configuration containing operator RPC URLs
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
    /// This method tries the given operation on each operator sequentially (in sorted order
    /// by public key). For each operator, it retries up to 3 times with exponential
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
    /// * [`Some(T)`](Some) - If any operator succeeds (possibly after retries)
    /// * [`None`] - If all operators fail after exhausting their retries
    ///
    /// # Retry Behavior
    ///
    /// Uses [`execute_with_retries`] for each operator with exponential backoff:
    ///
    /// - **Attempt 0**: Immediate (no delay)
    /// - **Attempt 1**: After ~2s delay
    /// - **Attempt 2**: After ~3s delay
    /// - **Attempt 3**: After ~5s delay
    /// - Total: ~10 seconds per operator
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
        // BTreeMap maintains sorted order automatically
        for (key, client) in self.clients.iter() {
            let client_clone = client.clone();
            let operation_name = format!("RPC request to operator public key {key}");

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
                    debug!(operator_pk = %key, "rpc request succeeded");
                    return Some(result);
                }
                Err(e) => {
                    warn!(
                        operator_pk = %key,
                        error = %e,
                        "rpc request failed after retries"
                    );
                    // Continue to next operator
                }
            }
        }

        None
    }
}

/// Fetch operator status.
pub(crate) async fn get_operator_status(rpc_url: &str) -> RpcOperatorStatus {
    let rpc_client = create_rpc_client(rpc_url);

    // Directly use `get_uptime`
    if rpc_client.get_uptime().await.is_ok() {
        RpcOperatorStatus::Online
    } else {
        warn!("failed to fetch bridge operator uptime");
        RpcOperatorStatus::Offline
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
