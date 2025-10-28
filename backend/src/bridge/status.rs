use axum::Json;
use bitcoin::{secp256k1::PublicKey, Txid};

use jsonrpsee::http_client::HttpClient;
use std::collections::BTreeMap;
use std::sync::{atomic::Ordering, Arc};
use strata_bridge_rpc::traits::{StrataBridgeControlApiClient, StrataBridgeMonitoringApiClient};
use strata_bridge_rpc::types::{
    RpcDepositStatus, RpcOperatorStatus, RpcReimbursementStatus, RpcWithdrawalStatus,
};

use strata_primitives::buf::Buf32;

use super::{
    cache::BridgeStatusCache,
    types::{
        BridgeMonitoringContext, BridgeStatus, DepositInfo, DepositStatus, OperatorStatus,
        ReimbursementInfo, ReimbursementStatus, TxStatus, WithdrawalInfo, WithdrawalStatus,
    },
};

use tokio::time::{interval, Duration};
use tracing::{debug, error, info, warn};

use crate::{
    config::BridgeMonitoringConfig,
    utils::rpc_client::{create_rpc_client, execute_with_retries},
};

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
struct RpcClientManager {
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
    fn new(config: &BridgeMonitoringConfig) -> Self {
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
    ///         client.get_deposit_requests().await.map_err(|e| e.into())
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
            let operation_name = format!("RPC request to operator {key}");

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
                    debug!("RPC request succeeded for operator: {}", key);
                    return Some(result);
                }
                Err(e) => {
                    warn!(
                        "RPC request failed for operator {} after retries: {}",
                        key, e
                    );
                    // Continue to next operator
                }
            }
        }

        None
    }
}

/// Get transaction confirmations from esplora
async fn get_tx_confirmations(esplora_url: &str, txid: Txid, chain_tip_height: u64) -> Option<u64> {
    let url = format!("{}/tx/{}/status", esplora_url.trim_end_matches('/'), txid);

    let status_resp = reqwest::get(&url).await;

    let status: TxStatus = match status_resp {
        Ok(resp) => match resp.json().await {
            Ok(status) => status,
            Err(e) => {
                error!(%txid, %e, "Failed to parse tx status JSON from esplora");
                return None;
            }
        },
        Err(e) => {
            error!(%txid, %e, "Failed to fetch tx status from esplora");
            return None;
        }
    };

    status
        .block_height
        .filter(|_| status.confirmed)
        .map(|h| chain_tip_height.saturating_sub(h) + 1)
}

/// Determine which cached deposit entries should be purged
async fn determine_deposits_to_purge(
    cache: &BridgeStatusCache,
    config: &BridgeMonitoringConfig,
    chain_tip_height: u64,
) -> Vec<Txid> {
    let max_confirmations = config.max_tx_confirmations();
    let mut deposits_to_purge = Vec::new();

    let final_deposits =
        cache.filter_deposits(|s| matches!(s, DepositStatus::Complete | DepositStatus::Failed));

    for (txid, deposit_info) in final_deposits {
        let check_txid = match deposit_info.status {
            DepositStatus::Failed => deposit_info.deposit_request_txid,
            DepositStatus::Complete => deposit_info
                .deposit_txid
                .unwrap_or(deposit_info.deposit_request_txid),
            DepositStatus::InProgress => unreachable!(),
        };

        let current_confirmations =
            get_tx_confirmations(config.esplora_url(), check_txid, chain_tip_height).await;

        if let Some(confirmations) = current_confirmations {
            if confirmations >= max_confirmations {
                deposits_to_purge.push(txid);
            }
        }
    }

    deposits_to_purge
}

/// Determine which cached withdrawal entries should be purged
async fn determine_withdrawals_to_purge(
    cache: &BridgeStatusCache,
    config: &BridgeMonitoringConfig,
    chain_tip_height: u64,
) -> Vec<Buf32> {
    let max_confirmations = config.max_tx_confirmations();
    let mut withdrawals_to_purge = Vec::new();

    let final_withdrawals = cache.filter_withdrawals(|s| matches!(s, WithdrawalStatus::Complete));

    for (request_id, withdrawal_info) in final_withdrawals {
        if let Some(fulfillment_txid) = withdrawal_info.fulfillment_txid {
            let current_confirmations =
                get_tx_confirmations(config.esplora_url(), fulfillment_txid, chain_tip_height)
                    .await;

            if let Some(confirmations) = current_confirmations {
                if confirmations >= max_confirmations {
                    withdrawals_to_purge.push(request_id);
                }
            }
        }
    }

    withdrawals_to_purge
}

/// Determine which cached reimbursement entries should be purged
async fn determine_reimbursements_to_purge(
    cache: &BridgeStatusCache,
    config: &BridgeMonitoringConfig,
    chain_tip_height: u64,
) -> Vec<Txid> {
    let max_confirmations = config.max_tx_confirmations();
    let mut reimbursements_to_purge = Vec::new();

    let final_reimbursements = cache.filter_reimbursements(|s| {
        matches!(
            s,
            ReimbursementStatus::Complete | ReimbursementStatus::Cancelled
        )
    });

    for (txid, reimbursement_info) in final_reimbursements {
        let check_txid = match reimbursement_info.status {
            ReimbursementStatus::Cancelled => reimbursement_info.claim_txid,
            ReimbursementStatus::Complete => reimbursement_info
                .payout_txid
                .unwrap_or(reimbursement_info.claim_txid),
            ReimbursementStatus::Challenged
            | ReimbursementStatus::InProgress
            | ReimbursementStatus::NotStarted => unreachable!(),
        };

        let current_confirmations =
            get_tx_confirmations(config.esplora_url(), check_txid, chain_tip_height).await;

        if let Some(confirmations) = current_confirmations {
            if confirmations >= max_confirmations {
                reimbursements_to_purge.push(txid);
            }
        }
    }

    reimbursements_to_purge
}

/// Periodically fetch bridge status and update bridge cache
pub async fn bridge_monitoring_task(context: Arc<BridgeMonitoringContext>) {
    let mut interval = interval(Duration::from_secs(
        context.config.status_refetch_interval(),
    ));

    // Create RPC client manager once and reuse it
    let rpc_manager = RpcClientManager::new(&context.config);

    loop {
        // Fetch all data without holding lock

        // Bridge operator status
        let mut operator_statuses = Vec::new();

        for (index, operator) in context.config.operators().iter().enumerate() {
            let operator_id = format!("Alpen Labs #{}", index + 1);
            let pk_bytes = hex::decode(operator.public_key()).expect("decode to succeed");
            let operator_pk = PublicKey::from_slice(&pk_bytes).expect("conversion to succeed");
            let status = get_operator_status(operator.rpc_url()).await;
            operator_statuses.push(OperatorStatus::new(operator_id, operator_pk, status));
        }

        // Update operators incrementally
        {
            let mut cache = context.status_cache.write().await;
            cache.update_operators(operator_statuses);
        }

        let chain_tip_height =
            match get_bitcoin_chain_tip_height(context.config.esplora_url()).await {
                Ok(height) => height,
                Err(e) => {
                    error!(error = %e, "Failed to get Bitcoin chain tip");
                    continue;
                }
            };
        info!(%chain_tip_height, "bitcoin chain tip");

        // Get existing active entries from cache to avoid unnecessary RPC calls
        let (active_deposits, active_withdrawals, active_reimbursements) = {
            let cache = context.status_cache.read().await;
            let deposits: Vec<Txid> = cache
                .filter_deposits(|s| matches!(s, DepositStatus::InProgress))
                .iter()
                .map(|(txid, _)| *txid)
                .collect();
            let withdrawals: Vec<Buf32> = cache
                .filter_withdrawals(|s| matches!(s, WithdrawalStatus::InProgress))
                .iter()
                .map(|(request_id, _)| *request_id)
                .collect();
            let reimbursements: Vec<Txid> = cache
                .filter_reimbursements(|s| {
                    matches!(
                        s,
                        ReimbursementStatus::InProgress | ReimbursementStatus::Challenged
                    )
                })
                .iter()
                .map(|(txid, _)| *txid)
                .collect();
            (deposits, withdrawals, reimbursements)
        };

        // Update deposits incrementally
        let deposit_updates: Vec<(Txid, DepositInfo, u64)> = get_deposits(
            &rpc_manager,
            &context.config,
            chain_tip_height,
            &active_deposits,
        )
        .await
        .iter()
        .map(|(info, confirmations)| (info.deposit_request_txid, *info, *confirmations))
        .collect();

        {
            let mut cache = context.status_cache.write().await;
            cache.apply_deposit_updates(deposit_updates);

            // Determine deposits to purge after applying updates
            let deposits_to_purge =
                determine_deposits_to_purge(&cache, &context.config, chain_tip_height).await;
            cache.purge_deposits(deposits_to_purge);
        }

        // Update withdrawals incrementally
        let withdrawal_updates: Vec<(Buf32, WithdrawalInfo, u64)> = get_withdrawals(
            &rpc_manager,
            &context.config,
            chain_tip_height,
            &active_withdrawals,
        )
        .await
        .iter()
        .map(|(info, confirmations)| (info.withdrawal_request_txid, *info, *confirmations))
        .collect();

        {
            let mut cache = context.status_cache.write().await;
            cache.apply_withdrawal_updates(withdrawal_updates);

            // Determine withdrawals to purge after applying updates
            let withdrawals_to_purge =
                determine_withdrawals_to_purge(&cache, &context.config, chain_tip_height).await;
            cache.purge_withdrawals(withdrawals_to_purge);
        }

        // Update reimbursements incrementally
        let reimbursement_updates: Vec<(Txid, ReimbursementInfo, u64)> = get_reimbursements(
            &rpc_manager,
            &context.config,
            chain_tip_height,
            &active_reimbursements,
        )
        .await
        .iter()
        .map(|(info, confirmations)| (info.claim_txid, *info, *confirmations))
        .collect();

        {
            let mut cache = context.status_cache.write().await;
            cache.apply_reimbursement_updates(reimbursement_updates);

            // Determine reimbursements to purge after applying updates
            let reimbursements_to_purge =
                determine_reimbursements_to_purge(&cache, &context.config, chain_tip_height).await;
            cache.purge_reimbursements(reimbursements_to_purge);
        }

        // Mark initial status query as complete and notify waiters
        if !context.status_available.load(Ordering::Acquire) {
            context.status_available.store(true, Ordering::Release);
            context.initial_status_query_complete.notify_waiters();
        }

        // Wait for next interval
        interval.tick().await;
    }
}

/// Fetch operator status
async fn get_operator_status(rpc_url: &str) -> RpcOperatorStatus {
    let rpc_client = create_rpc_client(rpc_url);

    // Directly use `get_uptime`
    if rpc_client.get_uptime().await.is_ok() {
        RpcOperatorStatus::Online
    } else {
        warn!("Failed to fetch bridge operator uptime");
        RpcOperatorStatus::Offline
    }
}

/// Fetch bitcoin chain tip height
async fn get_bitcoin_chain_tip_height(
    esplora_url: &str,
) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
    let endpoint = format!("{}/blocks/tip/height", esplora_url.trim_end_matches('/'));
    let resp = reqwest::Client::new().get(&endpoint).send().await?;

    let text = resp.text().await?;
    let height = text.trim().parse::<u64>()?;
    Ok(height)
}

/// Fetch all pending deposit request transaction IDs from bridge operators
///
/// Uses the RPC client manager with retry logic to query all operators for their
/// list of deposit requests. Returns the first non-empty result, or an empty vector
/// if all operators have no deposits or all fail.
///
/// # Arguments
///
/// * `rpc_manager` - RPC client manager with retry/failover logic
///
/// # Returns
///
/// Vector of deposit request transaction IDs. Empty if no deposits found or all operators failed.
async fn get_deposit_requests(rpc_manager: &RpcClientManager) -> Vec<Txid> {
    let result = rpc_manager
        .query_clients_with_retry(|client| async move {
            client.get_deposit_requests().await.map_err(|e| e.into())
        })
        .await;

    result.unwrap_or_else(|| {
        warn!("No deposit requests found from any operator");
        Vec::new()
    })
}

/// Fetch detailed information for all deposit requests
///
/// Queries bridge operators for deposit details, including both new deposit requests
/// and existing active deposits that need status updates. Filters results based on
/// confirmation count to only include deposits below the maximum confirmation threshold.
///
/// # Arguments
///
/// * `rpc_manager` - RPC client manager with retry/failover logic
/// * `config` - Bridge monitoring configuration (contains max confirmations threshold)
/// * `chain_tip_height` - Current Bitcoin blockchain height
/// * `active_deposit_txids` - List of deposit txids already being tracked (for status updates)
///
/// # Returns
///
/// Vector of tuples containing:
///
/// - [`DepositInfo`] - Detailed deposit information
/// - [`u64`] - Number of confirmations (0 for in-progress deposits)
///
/// Deposits are filtered to only include those with confirmations < [`BridgeMonitoringConfig::max_tx_confirmations`].
async fn get_deposits(
    rpc_manager: &RpcClientManager,
    config: &BridgeMonitoringConfig,
    chain_tip_height: u64,
    active_deposit_txids: &[Txid],
) -> Vec<(DepositInfo, u64)> {
    let new_deposit_requests = get_deposit_requests(rpc_manager).await;
    let new_count = new_deposit_requests.len();
    let mut deposit_requests = new_deposit_requests;

    // Add existing active deposits that we need to check for status updates
    for txid in active_deposit_txids {
        if !deposit_requests.contains(txid) {
            deposit_requests.push(*txid);
        }
    }

    info!(
        "Checking {} deposit requests ({} new, {} existing active)",
        deposit_requests.len(),
        new_count,
        active_deposit_txids.len()
    );

    let mut deposit_infos: Vec<(DepositInfo, u64)> = Vec::new();
    for deposit_request_txid in deposit_requests.iter() {
        let txid = *deposit_request_txid;
        let rpc_info = rpc_manager
            .query_clients_with_retry(|client| async move {
                client
                    .get_deposit_request_info(txid)
                    .await
                    .map_err(|e| e.into())
            })
            .await;

        let Some(dep_info) = rpc_info else {
            error!(%deposit_request_txid, "Failed to fetch deposit info after retries");
            continue;
        };

        // Filter based on number of confirmations
        match &dep_info.status {
            RpcDepositStatus::InProgress => {
                // Always include in-progress deposits - no need to check confirmations
                deposit_infos.push((DepositInfo::from(&dep_info), 0));
            }
            RpcDepositStatus::Failed { .. } | RpcDepositStatus::Complete { .. } => {
                let txid = match &dep_info.status {
                    RpcDepositStatus::Failed { .. } => dep_info.deposit_request_txid,
                    RpcDepositStatus::Complete { deposit_txid } => *deposit_txid,
                    RpcDepositStatus::InProgress => unreachable!(), // Already matched
                };

                let confirmations =
                    get_tx_confirmations(config.esplora_url(), txid, chain_tip_height).await;
                if let Some(confirmations) = confirmations {
                    if confirmations < config.max_tx_confirmations() {
                        deposit_infos.push((DepositInfo::from(&dep_info), confirmations));
                    }
                }
            }
        }
    }

    if deposit_infos.is_empty() {
        warn!("No deposit infos found");
    }
    deposit_infos
}

/// Fetch all pending withdrawal request IDs from bridge operators
///
/// Uses the RPC client manager with retry logic to query all operators for their
/// list of withdrawal requests. Returns the first non-empty result, or an empty vector
/// if all operators have no withdrawals or all fail.
///
/// # Arguments
///
/// * `rpc_manager` - RPC client manager with retry/failover logic
///
/// # Returns
///
/// Vector of withdrawal request IDs. Empty if no withdrawals found or all operators failed.
async fn get_withdrawal_requests(rpc_manager: &RpcClientManager) -> Vec<Buf32> {
    let result = rpc_manager
        .query_clients_with_retry(|client| async move {
            client.get_withdrawals().await.map_err(|e| e.into())
        })
        .await;

    result.unwrap_or_else(|| {
        warn!("No withdrawal requests found from any operator");
        Vec::new()
    })
}

/// Fetch detailed information for all withdrawal requests and fulfillments
///
/// Queries bridge operators for withdrawal details, including both new withdrawal requests
/// and existing active withdrawals that need status updates. Filters results based on
/// confirmation count to only include withdrawals below the maximum confirmation threshold.
///
/// # Arguments
///
/// * `rpc_manager` - RPC client manager with retry/failover logic
/// * `config` - Bridge monitoring configuration (contains max confirmations threshold)
/// * `chain_tip_height` - Current Bitcoin blockchain height
/// * `active_withdrawal_request_ids` - List of withdrawal request IDs already being tracked
///
/// # Returns
///
/// Vector of tuples containing:
/// - `WithdrawalInfo` - Detailed withdrawal information
/// - `u64` - Number of confirmations (0 for in-progress withdrawals)
///
/// Withdrawals are filtered to only include those with confirmations < max_tx_confirmations.
async fn get_withdrawals(
    rpc_manager: &RpcClientManager,
    config: &BridgeMonitoringConfig,
    chain_tip_height: u64,
    active_withdrawal_request_ids: &[Buf32],
) -> Vec<(WithdrawalInfo, u64)> {
    let new_withdrawal_requests = get_withdrawal_requests(rpc_manager).await;
    let new_count = new_withdrawal_requests.len();
    let mut withdrawal_requests = new_withdrawal_requests;

    // Add existing active withdrawals that we need to check for status updates
    for request_id in active_withdrawal_request_ids {
        if !withdrawal_requests.contains(request_id) {
            withdrawal_requests.push(*request_id);
        }
    }

    info!(
        "Checking {} withdrawal requests ({} new, {} existing active)",
        withdrawal_requests.len(),
        new_count,
        active_withdrawal_request_ids.len()
    );

    let mut withdrawal_infos: Vec<(WithdrawalInfo, u64)> = Vec::new();
    for withdrawal_request_txid in withdrawal_requests.iter() {
        let request_id = *withdrawal_request_txid;
        let rpc_info = rpc_manager
            .query_clients_with_retry(|client| async move {
                match client.get_withdrawal_info(request_id).await {
                    Ok(Some(info)) => Ok(info),
                    Ok(None) => Err("No withdrawal info found".into()),
                    Err(e) => Err(e.into()),
                }
            })
            .await;

        let Some(wd_info) = rpc_info else {
            error!(%withdrawal_request_txid, "Failed to fetch withdrawal info after retries");
            continue;
        };

        // Filter based on number of confirmations
        match &wd_info.status {
            RpcWithdrawalStatus::InProgress => {
                // Always include in-progress withdrawals
                withdrawal_infos.push((WithdrawalInfo::from(&wd_info), 0));
            }
            RpcWithdrawalStatus::Complete { fulfillment_txid } => {
                let confirmations =
                    get_tx_confirmations(config.esplora_url(), *fulfillment_txid, chain_tip_height)
                        .await;
                if let Some(confirmations) = confirmations {
                    if confirmations < config.max_tx_confirmations() {
                        withdrawal_infos.push((WithdrawalInfo::from(&wd_info), confirmations));
                    }
                }
            }
        }
    }

    if withdrawal_infos.is_empty() {
        warn!("No withdrawal infos found");
    }
    withdrawal_infos
}

/// Fetch all pending claim transaction IDs from bridge operators
///
/// Uses the RPC client manager with retry logic to query all operators for their
/// list of claim transactions. Returns the first non-empty result, or an empty vector
/// if all operators have no claims or all fail.
///
/// # Arguments
///
/// * `rpc_manager` - RPC client manager with retry/failover logic
///
/// # Returns
///
/// Vector of claim transaction IDs. Empty if no claims found or all operators failed.
async fn get_claims(rpc_manager: &RpcClientManager) -> Vec<Txid> {
    let result = rpc_manager
        .query_clients_with_retry(|client| async move {
            client.get_claims().await.map_err(|e| e.into())
        })
        .await;

    result.unwrap_or_else(|| {
        warn!("No claims found from any operator");
        Vec::new()
    })
}

/// Fetch detailed information for all claim and reimbursement transactions
///
/// Queries bridge operators for claim/reimbursement details, including both new claims
/// and existing active reimbursements that need status updates. Filters results based on
/// confirmation count and status (skips NotStarted claims).
///
/// # Arguments
///
/// * `rpc_manager` - RPC client manager with retry/failover logic
/// * `config` - Bridge monitoring configuration (contains max confirmations threshold)
/// * `chain_tip_height` - Current Bitcoin blockchain height
/// * `active_reimbursement_txids` - List of claim txids already being tracked
///
/// # Returns
///
/// Vector of tuples containing:
/// - `ReimbursementInfo` - Detailed claim/reimbursement information
/// - `u64` - Number of confirmations (0 for in-progress/challenged claims)
///
/// Claims with status `NotStarted` are excluded. Completed/cancelled claims are filtered
/// to only include those with confirmations < max_tx_confirmations.
async fn get_reimbursements(
    rpc_manager: &RpcClientManager,
    config: &BridgeMonitoringConfig,
    chain_tip_height: u64,
    active_reimbursement_txids: &[Txid],
) -> Vec<(ReimbursementInfo, u64)> {
    let new_claims = get_claims(rpc_manager).await;
    let new_count = new_claims.len();
    let mut claims = new_claims;

    // Add existing active reimbursements that we need to check for status updates
    for txid in active_reimbursement_txids {
        if !claims.contains(txid) {
            claims.push(*txid);
        }
    }

    info!(
        "Checking {} claims ({} new, {} existing active)",
        claims.len(),
        new_count,
        active_reimbursement_txids.len()
    );

    let mut reimbursement_infos = Vec::new();
    for claim_txid in claims.iter() {
        let txid = *claim_txid;
        let rpc_info = rpc_manager
            .query_clients_with_retry(|client| async move {
                match client.get_claim_info(txid).await {
                    Ok(Some(info)) => Ok(info),
                    Ok(None) => Err("No claim info found".into()),
                    Err(e) => Err(e.into()),
                }
            })
            .await;

        let Some(claim_info) = rpc_info else {
            error!(%claim_txid, "Failed to fetch claim info after retries");
            continue;
        };

        // Filter based on number of confirmations
        match &claim_info.status {
            // Skip if not started
            RpcReimbursementStatus::NotStarted => {
                continue;
            }
            RpcReimbursementStatus::InProgress { .. }
            | RpcReimbursementStatus::Challenged { .. } => {
                // Always include in-progress or challenged claims - no need to check confirmations
                reimbursement_infos.push((ReimbursementInfo::from(&claim_info), 0));
            }
            RpcReimbursementStatus::Cancelled | RpcReimbursementStatus::Complete { .. } => {
                let txid = match &claim_info.status {
                    RpcReimbursementStatus::Cancelled => claim_info.claim_txid,
                    RpcReimbursementStatus::Complete { payout_txid } => *payout_txid,
                    RpcReimbursementStatus::NotStarted
                    | RpcReimbursementStatus::InProgress { .. }
                    | RpcReimbursementStatus::Challenged { .. } => unreachable!(), // Already matched
                };
                let confirmations =
                    get_tx_confirmations(config.esplora_url(), txid, chain_tip_height).await;
                if let Some(confirmations) = confirmations {
                    if confirmations < config.max_tx_confirmations() {
                        reimbursement_infos
                            .push((ReimbursementInfo::from(&claim_info), confirmations));
                    }
                }
            }
        }
    }

    if reimbursement_infos.is_empty() {
        warn!("No reimbursement infos found");
    }
    reimbursement_infos
}

/// Return latest bridge status extracted from cache
pub async fn get_bridge_status(context: Arc<BridgeMonitoringContext>) -> Json<BridgeStatus> {
    // Wait for initial status query to complete if not yet available
    if !context.status_available.load(Ordering::Acquire) {
        info!("Waiting for initial bridge status query to complete");
        context.initial_status_query_complete.notified().await;
    }

    let cache = context.status_cache.read().await;

    let bridge_status = BridgeStatus {
        operators: cache.get_operators(),
        deposits: cache
            .filter_deposits(|_| true)
            .into_iter()
            .map(|(_, info)| info)
            .collect(),
        withdrawals: cache
            .filter_withdrawals(|_| true)
            .into_iter()
            .map(|(_, info)| info)
            .collect(),
        reimbursements: cache
            .filter_reimbursements(|_| true)
            .into_iter()
            .map(|(_, info)| info)
            .collect(),
    };

    Json(bridge_status)
}
