use axum::Json;
use bitcoin::{secp256k1::PublicKey, Txid};
use jsonrpsee::core::client::ClientT;
use jsonrpsee::core::ClientError;
use jsonrpsee::http_client::HttpClient;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use strata_bridge_rpc::types::{
    RpcClaimInfo, RpcDepositInfo, RpcDepositStatus, RpcOperatorStatus, RpcReimbursementStatus,
    RpcWithdrawalInfo, RpcWithdrawalStatus,
};
use strata_primitives::buf::Buf32;

use tokio::{
    sync::RwLock,
    time::{interval, Duration},
};
use tracing::{error, info};

use crate::{config::BridgeMonitoringConfig, utils::create_rpc_client};

/// Bridge operator status
#[derive(Serialize, Deserialize, Clone, Debug)]
pub(crate) struct OperatorStatus {
    operator_id: String,
    operator_address: PublicKey,
    status: RpcOperatorStatus,
}

impl OperatorStatus {
    pub(crate) fn new(
        operator_id: String,
        operator_address: PublicKey,
        status: RpcOperatorStatus,
    ) -> Self {
        Self {
            operator_id,
            operator_address,
            status,
        }
    }
}

#[derive(Deserialize)]
struct TxStatus {
    confirmed: bool,
    block_height: Option<u64>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub(crate) enum DepositStatus {
    #[serde(rename = "In progress")]
    InProgress,
    Failed,
    Complete,
}

/// Deposit information passed to dashboard
#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct DepositInfo {
    pub deposit_request_txid: Txid,
    pub deposit_txid: Option<Txid>,
    pub status: DepositStatus,
}

impl From<&RpcDepositInfo> for DepositInfo {
    fn from(rpc_info: &RpcDepositInfo) -> Self {
        match &rpc_info.status {
            RpcDepositStatus::InProgress => DepositInfo {
                deposit_request_txid: rpc_info.deposit_request_txid,
                deposit_txid: None,
                status: DepositStatus::InProgress,
            },
            RpcDepositStatus::Failed { .. } => DepositInfo {
                deposit_request_txid: rpc_info.deposit_request_txid,
                deposit_txid: None,
                status: DepositStatus::Failed,
            },
            RpcDepositStatus::Complete { deposit_txid } => DepositInfo {
                deposit_request_txid: rpc_info.deposit_request_txid,
                deposit_txid: Some(*deposit_txid),
                status: DepositStatus::Complete,
            },
        }
    }
}

/// Withdrawal status
#[derive(Serialize, Deserialize, Clone, Debug)]
pub(crate) enum WithdrawalStatus {
    #[serde(rename = "In progress")]
    InProgress,
    Complete,
}

/// Withdrawal information passed to dashboard
#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct WithdrawalInfo {
    pub withdrawal_request_txid: Buf32,
    pub fulfillment_txid: Option<Txid>,
    pub status: WithdrawalStatus,
}

impl From<&RpcWithdrawalInfo> for WithdrawalInfo {
    fn from(rpc_info: &RpcWithdrawalInfo) -> Self {
        match &rpc_info.status {
            RpcWithdrawalStatus::InProgress => Self {
                withdrawal_request_txid: rpc_info.withdrawal_request_txid,
                fulfillment_txid: None,
                status: WithdrawalStatus::InProgress,
            },
            RpcWithdrawalStatus::Complete { fulfillment_txid } => Self {
                withdrawal_request_txid: rpc_info.withdrawal_request_txid,
                fulfillment_txid: Some(*fulfillment_txid),
                status: WithdrawalStatus::Complete,
            },
        }
    }
}

/// Reimbursement status
#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum ReimbursementStatus {
    #[serde(rename = "In progress")]
    InProgress,
    Challenged,
    Cancelled,
    Complete,
}

/// Claim and reimbursement information passed to dashboard
#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct ReimbursementInfo {
    pub claim_txid: Txid,
    pub challenge_step: String,
    pub payout_txid: Option<Txid>,
    pub status: ReimbursementStatus,
}

impl From<&RpcClaimInfo> for ReimbursementInfo {
    fn from(rpc_info: &RpcClaimInfo) -> Self {
        match &rpc_info.status {
            RpcReimbursementStatus::NotStarted => Self {
                claim_txid: rpc_info.claim_txid,
                challenge_step: "N/A".to_string(),
                payout_txid: None,
                status: ReimbursementStatus::InProgress,
            },
            RpcReimbursementStatus::InProgress { challenge_step, .. } => Self {
                claim_txid: rpc_info.claim_txid,
                challenge_step: format!("{challenge_step:?}"),
                payout_txid: None,
                status: ReimbursementStatus::InProgress,
            },
            RpcReimbursementStatus::Challenged { challenge_step, .. } => Self {
                claim_txid: rpc_info.claim_txid,
                challenge_step: format!("{challenge_step:?}"),
                payout_txid: None,
                status: ReimbursementStatus::Challenged,
            },
            RpcReimbursementStatus::Cancelled => Self {
                claim_txid: rpc_info.claim_txid,
                challenge_step: "N/A".to_string(),
                payout_txid: None,
                status: ReimbursementStatus::Cancelled,
            },
            RpcReimbursementStatus::Complete { payout_txid } => Self {
                claim_txid: rpc_info.claim_txid,
                challenge_step: "N/A".to_string(),
                payout_txid: Some(*payout_txid),
                status: ReimbursementStatus::Complete,
            },
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct BridgeStatus {
    operators: Vec<OperatorStatus>,
    deposits: Vec<DepositInfo>,
    withdrawals: Vec<WithdrawalInfo>,
    reimbursements: Vec<ReimbursementInfo>,
}

/// Shared bridge state
pub type SharedBridgeState = Arc<RwLock<BridgeStatus>>;

/// Periodically fetch bridge status and update shared bridge state
pub async fn bridge_monitoring_task(state: SharedBridgeState, config: &BridgeMonitoringConfig) {
    let mut interval = interval(Duration::from_secs(config.status_refetch_interval()));
    let bridge_rpc = create_rpc_client(config.bridge_rpc_url());

    loop {
        interval.tick().await;
        let mut locked_state = state.write().await;

        // Bridge operator status
        let operators = get_bridge_operators(&bridge_rpc).await.unwrap_or_default();
        let mut operator_statuses = Vec::new();
        for (index, public_key) in operators.iter().enumerate() {
            let operator_id = format!("Alpen Labs #{}", index + 1);
            let status = get_operator_status(&bridge_rpc, *public_key)
                .await
                .unwrap_or(RpcOperatorStatus::Offline);

            operator_statuses.push(OperatorStatus::new(operator_id, *public_key, status));
        }

        locked_state.operators = operator_statuses;

        let chain_tip_height = match get_bitcoin_chain_tip_height(config.esplora_url()).await {
            Ok(height) => height,
            Err(e) => {
                error!(error = %e, "Failed to get Bitcoin chain tip");
                continue;
            }
        };
        info!(%chain_tip_height, "bitcoin chain tip");

        // Deposits
        let deposit_infos: Vec<DepositInfo> = get_deposits(&bridge_rpc, chain_tip_height, config)
            .await
            .unwrap_or_default();
        locked_state.deposits = deposit_infos;

        // Withdrawals and fulfillments
        let withdrawal_infos: Vec<WithdrawalInfo> =
            get_withdrawals(&bridge_rpc, chain_tip_height, config)
                .await
                .unwrap_or_default();
        locked_state.withdrawals = withdrawal_infos;

        // Claims and reimbursements
        let reimbursements: Vec<ReimbursementInfo> =
            get_reimbursements(&bridge_rpc, chain_tip_height, config)
                .await
                .unwrap_or_default();
        locked_state.reimbursements = reimbursements;
    }
}

/// Fetch operator idx and public keys
async fn get_bridge_operators(rpc_client: &HttpClient) -> Result<Vec<PublicKey>, ClientError> {
    let operator_table: Vec<PublicKey> = match rpc_client
        .request("stratabridge_bridgeOperators", ((),))
        .await
    {
        Ok(data) => data,
        Err(e) => {
            error!(error = %e, "Failed to fetch bridge operators");
            return Err(e);
        }
    };

    Ok(operator_table)
}

/// Fetch operator status
async fn get_operator_status(
    bridge_client: &HttpClient,
    operator_pk: PublicKey,
) -> Result<RpcOperatorStatus, ClientError> {
    let status: RpcOperatorStatus = match bridge_client
        .request("stratabridge_operatorStatus", (operator_pk,))
        .await
    {
        Ok(data) => data,
        Err(e) => {
            error!(error = %e, "Failed to fetch bridge operator status");
            return Err(e);
        }
    };

    Ok(status)
}

/// Fetch bitcoin chain tip height
async fn get_bitcoin_chain_tip_height(esplora_url: &str) -> Result<u64, reqwest::Error> {
    let endpoint = format!("{}/blocks/tip/height", esplora_url.trim_end_matches('/'));
    let resp = reqwest::Client::new()
        .get(&endpoint)
        .send()
        .await?
        .text()
        .await?;

    let height = resp.trim().parse::<u64>().expect("valid height");
    Ok(height)
}

/// Check whether to keep entry with txid based on number of confirmations
///
/// Keep if < `max_confirmations`
async fn keep_entry(
    esplora_url: &str,
    txid: Txid,
    chain_tip_height: u64,
    max_confirmations: u64,
) -> bool {
    let url = format!("{}/tx/{}/status", esplora_url.trim_end_matches('/'), txid);

    let status_resp = reqwest::get(&url).await;

    let status: TxStatus = match status_resp {
        Ok(resp) => match resp.json().await {
            Ok(status) => status,
            Err(e) => {
                error!(%txid, %e, "Failed to parse tx status JSON from esplora");
                return false;
            }
        },
        Err(e) => {
            error!(%txid, %e, "Failed to fetch tx status from esplora");
            return false;
        }
    };

    let confirmations = status
        .block_height
        .filter(|_| status.confirmed)
        .map(|h| chain_tip_height.saturating_sub(h) + 1)
        .unwrap_or(0);

    confirmations < max_confirmations
}

/// Fetch deposit requests
async fn get_deposit_requests(bridge_client: &HttpClient) -> Result<Vec<Txid>, ClientError> {
    let deposit_request_txids: Vec<Txid> = match bridge_client
        .request("stratabridge_depositRequests", ((),))
        .await
    {
        Ok(data) => data,
        Err(e) => {
            error!(error = %e, "Failed to fetch deposit requests");
            return Err(e);
        }
    };

    Ok(deposit_request_txids)
}

/// Fetch deposit details
async fn get_deposits(
    bridge_client: &HttpClient,
    chain_tip_height: u64,
    config: &BridgeMonitoringConfig,
) -> Result<Vec<DepositInfo>, ClientError> {
    let deposit_requests = match get_deposit_requests(bridge_client).await {
        Ok(data) => data,
        Err(e) => {
            error!(error = %e, "Failed to fetch deposit requests");
            return Err(e);
        }
    };

    let mut deposit_infos = Vec::new();
    for deposit_request_txid in deposit_requests.iter() {
        let rpc_info: RpcDepositInfo = match bridge_client
            .request("stratabridge_depositInfo", (deposit_request_txid,))
            .await
        {
            Ok(data) => data,
            Err(e) => {
                error!(error = %e, "Failed to fetch deposit info");
                return Err(e);
            }
        };

        info!(
            ?rpc_info,
            "Fetched deposit info for {}", deposit_request_txid
        );

        // Filter based on number of confirmations
        match &rpc_info.status {
            RpcDepositStatus::InProgress => {
                // Always include in-progress deposits
                deposit_infos.push(DepositInfo::from(&rpc_info));
            }
            RpcDepositStatus::Failed { .. } | RpcDepositStatus::Complete { .. } => {
                let txid = match &rpc_info.status {
                    RpcDepositStatus::Failed { .. } => rpc_info.deposit_request_txid,
                    RpcDepositStatus::Complete { deposit_txid } => *deposit_txid,
                    _ => unreachable!(), // Already matched
                };

                if keep_entry(
                    config.esplora_url(),
                    txid,
                    chain_tip_height,
                    config.max_tx_confirmations(),
                )
                .await
                {
                    deposit_infos.push(DepositInfo::from(&rpc_info));
                }
            }
        }
    }
    Ok(deposit_infos)
}

/// Fetch withdrawal request txids
async fn get_withdrawal_requests(bridge_client: &HttpClient) -> Result<Vec<Buf32>, ClientError> {
    let withdrawal_requests: Vec<Buf32> = match bridge_client
        .request("stratabridge_withdrawals", ((),))
        .await
    {
        Ok(data) => data,
        Err(e) => {
            error!(error = %e, "Failed to fetch withdrawal requests");
            return Err(e);
        }
    };

    Ok(withdrawal_requests)
}

/// Fetch withdrawal/fullfillment details
async fn get_withdrawals(
    bridge_client: &HttpClient,
    chain_tip_height: u64,
    config: &BridgeMonitoringConfig,
) -> Result<Vec<WithdrawalInfo>, ClientError> {
    let withdrawal_requests = match get_withdrawal_requests(bridge_client).await {
        Ok(data) => data,
        Err(e) => {
            error!(error = %e, "Failed to fetch withdrawal requests");
            return Err(e);
        }
    };

    let mut withdrawal_infos = Vec::new();
    for withdrawal_request_txid in withdrawal_requests.iter() {
        let rpc_info: RpcWithdrawalInfo = match bridge_client
            .request("stratabridge_withdrawalInfo", (withdrawal_request_txid,))
            .await
        {
            Ok(data) => data,
            Err(e) => {
                error!(error = %e, "Failed to fetch withdrawal info");
                return Err(e);
            }
        };

        // Filter based on number of confirmations
        match &rpc_info.status {
            RpcWithdrawalStatus::InProgress => {
                // Always include in-progress withdrawals
                withdrawal_infos.push(WithdrawalInfo::from(&rpc_info));
            }
            RpcWithdrawalStatus::Complete { fulfillment_txid } => {
                if keep_entry(
                    config.esplora_url(),
                    *fulfillment_txid,
                    chain_tip_height,
                    config.max_tx_confirmations(),
                )
                .await
                {
                    withdrawal_infos.push(WithdrawalInfo::from(&rpc_info));
                }
            }
        }
    }

    Ok(withdrawal_infos)
}

/// Fetch claim/reimbursement details
async fn get_reimbursements(
    bridge_rpc: &HttpClient,
    chain_tip_height: u64,
    config: &BridgeMonitoringConfig,
) -> Result<Vec<ReimbursementInfo>, ClientError> {
    let claim_txids: Vec<String> = match bridge_rpc.request("stratabridge_claims", ((),)).await {
        Ok(data) => data,
        Err(e) => {
            error!(error = %e, "Failed to fetch claims");
            return Err(e);
        }
    };

    let mut reimbursement_infos = Vec::new();
    for txid in claim_txids.iter() {
        let rpc_info: RpcClaimInfo = match bridge_rpc
            .request("stratabridge_claimInfo", (txid.clone(),))
            .await
        {
            Ok(data) => data,
            Err(e) => {
                error!(error = %e, "Failed to fetch claim info");
                return Err(e);
            }
        };

        // Filter based on number of confirmations
        match &rpc_info.status {
            // Skip if not started
            RpcReimbursementStatus::NotStarted => {
                continue;
            }
            RpcReimbursementStatus::InProgress { .. }
            | RpcReimbursementStatus::Challenged { .. } => {
                // Always include in-progress or challenged claims
                reimbursement_infos.push(ReimbursementInfo::from(&rpc_info));
            }
            RpcReimbursementStatus::Cancelled | RpcReimbursementStatus::Complete { .. } => {
                let txid = match &rpc_info.status {
                    RpcReimbursementStatus::Cancelled => rpc_info.claim_txid,
                    RpcReimbursementStatus::Complete { payout_txid } => *payout_txid,
                    _ => unreachable!(), // Already matched
                };
                if keep_entry(
                    config.esplora_url(),
                    txid,
                    chain_tip_height,
                    config.max_tx_confirmations(),
                )
                .await
                {
                    reimbursement_infos.push(ReimbursementInfo::from(&rpc_info));
                }
            }
        }
    }

    Ok(reimbursement_infos)
}

/// Return latest bridge status
pub async fn get_bridge_status(state: SharedBridgeState) -> Json<BridgeStatus> {
    let data = state.read().await.clone();
    Json(data)
}
