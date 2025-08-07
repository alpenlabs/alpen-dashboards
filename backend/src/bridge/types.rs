use bitcoin::{secp256k1::PublicKey, Txid};
use serde::{Deserialize, Serialize};
use strata_bridge_rpc::types::{
    RpcClaimInfo, RpcDepositInfo, RpcDepositStatus, RpcOperatorStatus, RpcReimbursementStatus,
    RpcWithdrawalInfo, RpcWithdrawalStatus,
};
use strata_primitives::buf::Buf32;

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
pub(crate) struct TxStatus {
    pub(crate) confirmed: bool,
    pub(crate) block_height: Option<u64>,
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug)]
pub(crate) enum DepositStatus {
    #[serde(rename = "In progress")]
    InProgress,
    Failed,
    Complete,
}

/// Deposit information passed to status dashboard
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
pub(crate) struct DepositInfo {
    pub(crate) deposit_request_txid: Txid,
    pub(crate) deposit_txid: Option<Txid>,
    pub(crate) status: DepositStatus,
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
#[derive(Serialize, Deserialize, Copy, Clone, Debug)]
pub(crate) enum WithdrawalStatus {
    #[serde(rename = "In progress")]
    InProgress,
    Complete,
}

/// Withdrawal information passed to status dashboard
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
pub(crate) struct WithdrawalInfo {
    pub(crate) withdrawal_request_txid: Buf32,
    pub(crate) fulfillment_txid: Option<Txid>,
    pub(crate) status: WithdrawalStatus,
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
#[derive(Serialize, Deserialize, Copy, Clone, Debug)]
pub(crate) enum ReimbursementStatus {
    #[serde(rename = "Not started")]
    NotStarted,
    #[serde(rename = "In progress")]
    InProgress,
    Challenged,
    Cancelled,
    Complete,
}

/// Challenge step for reimbursements
#[derive(Serialize, Deserialize, Copy, Clone, Debug)]
pub(crate) enum ChallengeStep {
    #[serde(rename = "N/A")]
    NotApplicable,
    Claim,
    Challenge,
    Assert,
}

/// Claim and reimbursement information passed to status dashboard
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
pub(crate) struct ReimbursementInfo {
    pub(crate) claim_txid: Txid,
    pub(crate) challenge_step: ChallengeStep,
    pub(crate) payout_txid: Option<Txid>,
    pub(crate) status: ReimbursementStatus,
}

impl From<&RpcClaimInfo> for ReimbursementInfo {
    fn from(rpc_info: &RpcClaimInfo) -> Self {
        match &rpc_info.status {
            RpcReimbursementStatus::NotStarted => Self {
                claim_txid: rpc_info.claim_txid,
                challenge_step: ChallengeStep::NotApplicable,
                payout_txid: None,
                status: ReimbursementStatus::NotStarted,
            },
            RpcReimbursementStatus::InProgress { .. } => Self {
                claim_txid: rpc_info.claim_txid,
                challenge_step: ChallengeStep::Claim,
                payout_txid: None,
                status: ReimbursementStatus::InProgress,
            },
            RpcReimbursementStatus::Challenged { .. } => Self {
                claim_txid: rpc_info.claim_txid,
                challenge_step: ChallengeStep::Challenge,
                payout_txid: None,
                status: ReimbursementStatus::Challenged,
            },
            RpcReimbursementStatus::Cancelled => Self {
                claim_txid: rpc_info.claim_txid,
                challenge_step: ChallengeStep::NotApplicable,
                payout_txid: None,
                status: ReimbursementStatus::Cancelled,
            },
            RpcReimbursementStatus::Complete { payout_txid } => Self {
                claim_txid: rpc_info.claim_txid,
                challenge_step: ChallengeStep::NotApplicable,
                payout_txid: Some(*payout_txid),
                status: ReimbursementStatus::Complete,
            },
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub(crate) struct BridgeStatus {
    pub(crate) operators: Vec<OperatorStatus>,
    pub(crate) deposits: Vec<DepositInfo>,
    pub(crate) withdrawals: Vec<WithdrawalInfo>,
    pub(crate) reimbursements: Vec<ReimbursementInfo>,
}

use std::sync::{atomic::AtomicBool, Arc};
use tokio::sync::{Notify, RwLock};

use super::cache::BridgeStatusCache;
use crate::config::BridgeMonitoringConfig;

/// Bridge monitoring context
pub struct BridgeMonitoringContext {
    pub(crate) status_cache: Arc<RwLock<BridgeStatusCache>>,
    pub(crate) config: BridgeMonitoringConfig,
    pub(crate) status_available: Arc<AtomicBool>,
    pub(crate) initial_status_query_complete: Arc<Notify>,
}

impl BridgeMonitoringContext {
    pub fn new(config: BridgeMonitoringConfig) -> Self {
        Self {
            status_cache: Arc::new(RwLock::new(BridgeStatusCache::new())),
            config,
            status_available: Arc::new(AtomicBool::new(false)),
            initial_status_query_complete: Arc::new(Notify::new()),
        }
    }
}
