use axum::Json;
use bitcoin::{secp256k1::PublicKey, Txid};
use jsonrpsee::core::client::ClientT;
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
use tracing::{error, info, warn};

use crate::{config::BridgeMonitoringConfig, utils::create_rpc_client};

/// Bridge operator status
#[derive(Serialize, Deserialize, Clone, Debug)]
pub(crate) struct OperatorStatus {
    operator_id: String,
    operator_pk: PublicKey,
    status: RpcOperatorStatus,
}

impl OperatorStatus {
    pub(crate) fn new(
        operator_id: String,
        operator_pk: PublicKey,
        status: RpcOperatorStatus,
    ) -> Self {
        Self {
            operator_id,
            operator_pk,
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

/// Deposit information passed to status dashboard
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

/// Withdrawal information passed to status dashboard
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

/// Claim and reimbursement information passed to status dashboard
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

    loop {
        interval.tick().await;
        let mut locked_state = state.write().await;

        // Bridge operator status
        let mut operator_statuses = Vec::new();
        for (index, public_key_string) in config.bridge_rpc_urls().keys().enumerate() {
            let operator_id = format!("Alpen Labs #{}", index + 1);
            let pk_bytes = hex::decode(public_key_string).expect("decode to succeed");
            let operator_pk = PublicKey::from_slice(&pk_bytes).expect("conversion to succeed");
            let rpc_url = config
                .bridge_rpc_urls()
                .get(public_key_string)
                .expect("valid rpc url");
            let status = get_operator_status(rpc_url).await;

            operator_statuses.push(OperatorStatus::new(operator_id, operator_pk, status));
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
        let deposit_infos: Vec<DepositInfo> = get_deposits(config, chain_tip_height).await;
        locked_state.deposits = deposit_infos;

        // Withdrawals and fulfillments
        let withdrawal_infos: Vec<WithdrawalInfo> = get_withdrawals(config, chain_tip_height).await;
        locked_state.withdrawals = withdrawal_infos;

        // Claims and reimbursements
        let reimbursements: Vec<ReimbursementInfo> =
            get_reimbursements(config, chain_tip_height).await;
        locked_state.reimbursements = reimbursements;
    }
}

/// Fetch operator status
async fn get_operator_status(rpc_url: &str) -> RpcOperatorStatus {
    let rpc_client = create_rpc_client(rpc_url);

    if (rpc_client
        .request::<u64, _>("stratabridge_uptime", ((),))
        .await)
        .is_ok()
    {
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

/// Check whether txid has fewer confirmations than `max_confirmations`
async fn has_fewer_confirmations_than_max(
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
async fn get_deposit_requests(config: &BridgeMonitoringConfig) -> Vec<Txid> {
    for rpc_url in config.bridge_rpc_urls().values() {
        let rpc_client = create_rpc_client(rpc_url);

        match rpc_client
            .request::<Vec<Txid>, _>("stratabridge_depositRequests", ((),))
            .await
        {
            Ok(txids) if !txids.is_empty() => return txids,
            Ok(_) | Err(_) => {} // Try next operator
        }
    }

    warn!("No deposit requests found");
    Vec::new()
}

/// Fetch deposit details
async fn get_deposits(config: &BridgeMonitoringConfig, chain_tip_height: u64) -> Vec<DepositInfo> {
    let deposit_requests = get_deposit_requests(config).await;
    info!("Found deposit requests {}", deposit_requests.len());

    let mut deposit_infos: Vec<DepositInfo> = Vec::new();
    for deposit_request_txid in deposit_requests.iter() {
        let mut rpc_info = None;
        for rpc_url in config.bridge_rpc_urls().values() {
            let rpc_client = create_rpc_client(rpc_url);
            if let Ok(info) = rpc_client
                .request::<RpcDepositInfo, _>("stratabridge_depositInfo", (deposit_request_txid,))
                .await
            {
                rpc_info = Some(info);
                break;
            }
        }

        let Some(dep_info) = rpc_info else {
            error!(%deposit_request_txid, "Failed to fetch deposit info");
            continue;
        };

        // Filter based on number of confirmations
        match &dep_info.status {
            RpcDepositStatus::InProgress => {
                // Always include in-progress deposits
                deposit_infos.push(DepositInfo::from(&dep_info));
            }
            RpcDepositStatus::Failed { .. } | RpcDepositStatus::Complete { .. } => {
                let txid = match &dep_info.status {
                    RpcDepositStatus::Failed { .. } => dep_info.deposit_request_txid,
                    RpcDepositStatus::Complete { deposit_txid } => *deposit_txid,
                    RpcDepositStatus::InProgress => unreachable!(), // Already matched
                };

                if has_fewer_confirmations_than_max(
                    config.esplora_url(),
                    txid,
                    chain_tip_height,
                    config.max_tx_confirmations(),
                )
                .await
                {
                    deposit_infos.push(DepositInfo::from(&dep_info));
                }
            }
        }
    }

    if deposit_infos.is_empty() {
        warn!("No deposit infos found");
    }
    deposit_infos
}

/// Fetch withdrawal requests
async fn get_withdrawal_requests(config: &BridgeMonitoringConfig) -> Vec<Buf32> {
    for rpc_url in config.bridge_rpc_urls().values() {
        let rpc_client = create_rpc_client(rpc_url);

        match rpc_client
            .request::<Vec<Buf32>, _>("stratabridge_withdrawals", ((),))
            .await
        {
            Ok(txids) if !txids.is_empty() => return txids,
            Ok(_) | Err(_) => {} // Try next operator
        }
    }

    warn!("No withdrawal requests found");
    Vec::new()
}

/// Fetch withdrawal/fullfillment details
async fn get_withdrawals(
    config: &BridgeMonitoringConfig,
    chain_tip_height: u64,
) -> Vec<WithdrawalInfo> {
    let withdrawal_requests = get_withdrawal_requests(config).await;

    let mut withdrawal_infos: Vec<WithdrawalInfo> = Vec::new();
    for withdrawal_request_txid in withdrawal_requests.iter() {
        let mut rpc_info = None;
        for rpc_url in config.bridge_rpc_urls().values() {
            let rpc_client = create_rpc_client(rpc_url);
            if let Ok(info) = rpc_client
                .request::<RpcWithdrawalInfo, _>(
                    "stratabridge_withdrawalInfo",
                    (withdrawal_request_txid,),
                )
                .await
            {
                rpc_info = Some(info);
                break;
            }
        }

        let Some(wd_info) = rpc_info else {
            error!(%withdrawal_request_txid, "Failed to fetch withdrawal info");
            continue;
        };

        // Filter based on number of confirmations
        match &wd_info.status {
            RpcWithdrawalStatus::InProgress => {
                // Always include in-progress withdrawals
                withdrawal_infos.push(WithdrawalInfo::from(&wd_info));
            }
            RpcWithdrawalStatus::Complete { fulfillment_txid } => {
                if has_fewer_confirmations_than_max(
                    config.esplora_url(),
                    *fulfillment_txid,
                    chain_tip_height,
                    config.max_tx_confirmations(),
                )
                .await
                {
                    withdrawal_infos.push(WithdrawalInfo::from(&wd_info));
                }
            }
        }
    }

    if withdrawal_infos.is_empty() {
        warn!("No withdrawal infos found");
    }
    withdrawal_infos
}

/// Fetch claims
async fn get_claims(config: &BridgeMonitoringConfig) -> Vec<Txid> {
    for rpc_url in config.bridge_rpc_urls().values() {
        let rpc_client = create_rpc_client(rpc_url);

        match rpc_client
            .request::<Vec<Txid>, _>("stratabridge_claims", ((),))
            .await
        {
            Ok(txids) if !txids.is_empty() => return txids,
            Ok(_) | Err(_) => {} // Try next operator
        }
    }

    warn!("No claims found");
    Vec::new()
}

/// Fetch claim/reimbursement details
async fn get_reimbursements(
    config: &BridgeMonitoringConfig,
    chain_tip_height: u64,
) -> Vec<ReimbursementInfo> {
    let claims = get_claims(config).await;

    let mut reimbursement_infos = Vec::new();
    for claim_txid in claims.iter() {
        let mut rpc_info = None;
        for rpc_url in config.bridge_rpc_urls().values() {
            let rpc_client = create_rpc_client(rpc_url);
            if let Ok(info) = rpc_client
                .request::<RpcClaimInfo, _>("stratabridge_claimInfo", (claim_txid,))
                .await
            {
                rpc_info = Some(info);
                break;
            }
        }

        let Some(claim_info) = rpc_info else {
            error!(%claim_txid, "Failed to fetch deposit info");
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
                // Always include in-progress or challenged claims
                reimbursement_infos.push(ReimbursementInfo::from(&claim_info));
            }
            RpcReimbursementStatus::Cancelled | RpcReimbursementStatus::Complete { .. } => {
                let txid = match &claim_info.status {
                    RpcReimbursementStatus::Cancelled => claim_info.claim_txid,
                    RpcReimbursementStatus::Complete { payout_txid } => *payout_txid,
                    RpcReimbursementStatus::NotStarted
                    | RpcReimbursementStatus::InProgress { .. }
                    | RpcReimbursementStatus::Challenged { .. } => unreachable!(), // Already matched
                };
                if has_fewer_confirmations_than_max(
                    config.esplora_url(),
                    txid,
                    chain_tip_height,
                    config.max_tx_confirmations(),
                )
                .await
                {
                    reimbursement_infos.push(ReimbursementInfo::from(&claim_info));
                }
            }
        }
    }

    if reimbursement_infos.is_empty() {
        warn!("No reimbursement infos found");
    }
    reimbursement_infos
}

/// Return latest bridge status
pub async fn get_bridge_status(state: SharedBridgeState) -> Json<BridgeStatus> {
    let data = state.read().await.clone();
    Json(data)
}
