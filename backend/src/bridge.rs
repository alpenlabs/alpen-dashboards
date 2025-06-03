use axum::Json;
use bitcoin::{secp256k1::PublicKey, OutPoint, Txid};
use jsonrpsee::core::client::ClientT;
use jsonrpsee::core::ClientError;
use jsonrpsee::http_client::HttpClient;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use strata_bridge_rpc::types::{
    RpcBridgeDutyStatus, RpcClaimInfo, RpcDepositStatus, RpcOperatorStatus, RpcReimbursementStatus,
    RpcWithdrawalInfo, RpcWithdrawalStatus,
};

use tokio::{
    sync::RwLock,
    time::{interval, Duration},
};
use tracing::{error, info, warn};

use crate::{config::BridgeMonitoringConfig, utils::create_rpc_client};

/// Bridge operator status
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct OperatorStatus {
    operator_id: String,
    operator_address: PublicKey,
    status: RpcOperatorStatus,
}

impl OperatorStatus {
    pub fn new(operator_id: String, operator_address: PublicKey, status: RpcOperatorStatus) -> Self {
        Self {
            operator_id,
            operator_address,
            status,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum DepositStatus {
    #[serde(rename = "In progress")]
    InProgress,
    Failed,
    Complete,
}

/// Deposit information passed to dashboard
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DepositInfo {
    pub deposit_request_txid: Txid,
    pub deposit_txid: Option<Txid>,
    pub status: DepositStatus,
}

impl From<RpcDepositStatus> for DepositInfo {
    fn from(status: RpcDepositStatus) -> Self {
        match status {
            RpcDepositStatus::InProgress {
                deposit_request_txid,
            } => DepositInfo {
                deposit_request_txid,
                deposit_txid: None,
                status: DepositStatus::InProgress,
            },
            RpcDepositStatus::Failed {
                deposit_request_txid,
                failure_reason: _,
            } => DepositInfo {
                deposit_request_txid,
                deposit_txid: None,
                status: DepositStatus::Failed,
            },
            RpcDepositStatus::Complete {
                deposit_request_txid,
                deposit_txid,
            } => DepositInfo {
                deposit_request_txid,
                deposit_txid: Some(deposit_txid),
                status: DepositStatus::Complete,
            },
        }
    }
}

/// Withdrawal status
#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum WithdrawalStatus {
    #[serde(rename = "In progress")]
    InProgress,
    Complete,
}

/// Withdrawal information passed to dashboard
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WithdrawalInfo {
    pub withdrawal_request_txid: Txid,
    pub fulfillment_txid: Option<Txid>,
    pub status: WithdrawalStatus,
}

impl WithdrawalInfo {
    pub fn from_rpc(rpc_info: &RpcWithdrawalInfo, withdrawal_request_txid: Txid) -> Self {
        match &rpc_info.status {
            RpcWithdrawalStatus::InProgress => Self {
                withdrawal_request_txid,
                fulfillment_txid: None,
                status: WithdrawalStatus::InProgress,
            },
            RpcWithdrawalStatus::Complete { fulfillment_txid } => Self {
                withdrawal_request_txid,
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
pub struct ReimbursementInfo {
    pub claim_txid: Txid,
    pub challenge_step: String,
    pub payout_txid: Option<Txid>,
    pub status: ReimbursementStatus,
}

impl From<&RpcClaimInfo> for ReimbursementInfo {
    fn from(rpc_info: &RpcClaimInfo) -> Self {
        match &rpc_info.status {
            RpcReimbursementStatus::InProgress => Self {
                claim_txid: rpc_info.claim_txid,
                challenge_step: "N/A".to_string(),
                payout_txid: None,
                status: ReimbursementStatus::InProgress,
            },
            RpcReimbursementStatus::Challenged => Self {
                claim_txid: rpc_info.claim_txid,
                challenge_step: "N/A".to_string(),
                payout_txid: None,
                status: ReimbursementStatus::Challenged,
            },
            RpcReimbursementStatus::Cancelled => Self {
                claim_txid: rpc_info.claim_txid,
                challenge_step: "N/A".to_string(),
                payout_txid: None,
                status: ReimbursementStatus::Cancelled,
            },
            RpcReimbursementStatus::Complete => Self {
                claim_txid: rpc_info.claim_txid,
                challenge_step: "N/A".to_string(),
                payout_txid: None,
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
        let operators = get_bridge_operators(&bridge_rpc).await.unwrap();
        let mut operator_statuses = Vec::new();
        for (index, public_key) in operators.iter().enumerate() {
            let operator_id = format!("Alpen Labs #{}", index);
            let status = get_operator_status(&bridge_rpc, *public_key).await.unwrap();

            operator_statuses.push(OperatorStatus::new(
                operator_id,
                *public_key,
                status)
            );
        }

        locked_state.operators = operator_statuses;

        // Reimbursements
        let reimbursements: Vec<ReimbursementInfo> =
            (get_reimbursements(&bridge_rpc).await).unwrap_or_default();
        locked_state.reimbursements = reimbursements;

        // Bridge duties to fetch deposits and withdrawals
        let bridge_duties: Vec<RpcBridgeDutyStatus> =
            (get_bridge_duties(&bridge_rpc).await).unwrap_or_default();

        if bridge_duties.is_empty() {
            warn!("No bridge duties found");
            continue;
        }

        // Separate deposit and withdrawal requests
        let mut deposit_requests = Vec::new();
        let mut withdrawal_requests = Vec::new();
        for duty in bridge_duties.iter() {
            if let RpcBridgeDutyStatus::Deposit {
                deposit_request_txid,
            } = duty
            {
                info!(%deposit_request_txid, "Found deposit request duty");
                deposit_requests.push(*deposit_request_txid);
            } else if let RpcBridgeDutyStatus::Withdrawal {
                withdrawal_request_txid,
                assigned_operator_idx,
            } = duty
            {
                info!(%withdrawal_request_txid, "Found withdrawal request duty for operator {}", assigned_operator_idx);
                withdrawal_requests.push(*withdrawal_request_txid);
            }
        }

        // Fetch deposit infos
        if deposit_requests.is_empty() {
            warn!("No deposits to fetch");
        } else {
            let deposit_infos =
                (get_deposit_infos(&bridge_rpc, &deposit_requests).await).unwrap_or_default();
            locked_state.deposits = deposit_infos;
        }

        // Fetch withdrawal infos
        if withdrawal_requests.is_empty() {
            warn!("No withdrawals to fetch");
        } else {
            let withdrawal_infos =
                (get_withdrawal_infos(&bridge_rpc, &withdrawal_requests).await).unwrap_or_default();
            locked_state.withdrawals = withdrawal_infos;
        }
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

/// Fetch bridge duties
async fn get_bridge_duties(
    bridge_client: &HttpClient,
) -> Result<Vec<RpcBridgeDutyStatus>, ClientError> {
    let duties: Vec<RpcBridgeDutyStatus> = match bridge_client
        .request("stratabridge_bridgeDuties", ((),))
        .await
    {
        Ok(data) => data,
        Err(e) => {
            error!(error = %e, "Failed to fetch bridge duties");
            return Err(e);
        }
    };

    Ok(duties)
}

/// Fetch deposit details
async fn get_deposit_infos(
    bridge_client: &HttpClient,
    deposit_requests: &[Txid],
) -> Result<Vec<DepositInfo>, ClientError> {
    let mut deposit_infos = Vec::new();
    for deposit_request_txid in deposit_requests.iter() {
        let deposit_request_outpoint = OutPoint {
            txid: *deposit_request_txid,
            vout: 0, // Assuming vout is always 0 for deposit requests
        };
        let rpc_info: RpcDepositStatus = match bridge_client
            .request("stratabridge_depositInfo", (deposit_request_outpoint,))
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
        deposit_infos.push(DepositInfo::from(rpc_info));
    }
    Ok(deposit_infos)
}

/// Fetch withdrawal/fullfillment details
async fn get_withdrawal_infos(
    bridge_client: &HttpClient,
    withdrawal_requests: &[Txid],
) -> Result<Vec<WithdrawalInfo>, ClientError> {
    let mut withdrawal_infos = Vec::new();
    for withdrawal_request_txid in withdrawal_requests.iter() {
        let withdrawal_request_outpoint = OutPoint {
            txid: *withdrawal_request_txid,
            vout: 0, // Assuming vout is always 0 for deposit requests
        };
        let rpc_info: RpcWithdrawalInfo = match bridge_client
            .request(
                "stratabridge_withdrawalInfo",
                (withdrawal_request_outpoint,),
            )
            .await
        {
            Ok(data) => data,
            Err(e) => {
                error!(error = %e, "Failed to fetch withdrawal info");
                return Err(e);
            }
        };

        withdrawal_infos.push(WithdrawalInfo::from_rpc(
            &rpc_info,
            *withdrawal_request_txid,
        ));
    }

    Ok(withdrawal_infos)
}

/// Fetch claim/reimbursement details
async fn get_reimbursements(
    bridge_rpc: &HttpClient,
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
        let reimb_info: RpcClaimInfo = match bridge_rpc
            .request("stratabridge_claimInfo", (txid.clone(),))
            .await
        {
            Ok(data) => data,
            Err(e) => {
                error!(error = %e, "Failed to fetch claim info");
                return Err(e);
            }
        };

        reimbursement_infos.push(ReimbursementInfo::from(&reimb_info));
    }

    Ok(reimbursement_infos)
}

/// Return latest bridge status
pub async fn get_bridge_status(state: SharedBridgeState) -> Json<BridgeStatus> {
    let data = state.read().await.clone();
    Json(data)
}
