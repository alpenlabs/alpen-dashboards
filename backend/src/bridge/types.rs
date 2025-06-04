use bitcoin::{secp256k1::PublicKey, OutPoint, Txid};
use serde::{Deserialize, Serialize};

use strata_bridge_rpc::types::{
    RpcClaimInfo, RpcDepositInfo, RpcDepositStatus, RpcOperatorStatus, RpcReimbursementStatus,
    RpcWithdrawalInfo, RpcWithdrawalStatus,
};


/// Bridge operator status
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct OperatorStatus {
    operator_id: String,
    operator_address: PublicKey,
    status: RpcOperatorStatus,
}

impl OperatorStatus {
    pub fn new  (operator_id: String, operator_address: PublicKey, status: RpcOperatorStatus) -> Self {
        Self {
            operator_id,
            operator_address,
            status,
        }
    }
}

/// Bridge deposit status
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

impl From<RpcDepositInfo> for DepositInfo {
    fn from(rpc_info: RpcDepositInfo) -> Self {
        match rpc_info.status {
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

/// Deposit to withdrawal mapping
#[derive(Debug)]
pub struct DepositToWithdrawal {
    deposit_outpoint: OutPoint,
    withdrawal_request_txid: Option<Txid>,
}

impl DepositToWithdrawal {
    pub fn new(deposit_outpoint: OutPoint, withdrawal_request_txid: Option<Txid>) -> Self {
        Self {
            deposit_outpoint,
            withdrawal_request_txid,
        }
    }

    pub fn deposit_outpoint(&self) -> &OutPoint {
        &self.deposit_outpoint
    }

    pub fn withdrawal_request_txid(&self) -> Option<Txid> {
        self.withdrawal_request_txid
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
            RpcReimbursementStatus::InProgress { challenge_step } => Self {
                claim_txid: rpc_info.claim_txid,
                challenge_step: format!("{:?}", challenge_step),
                payout_txid: None,
                status: ReimbursementStatus::InProgress,
            },
            RpcReimbursementStatus::Challenged { challenge_step } => Self {
                claim_txid: rpc_info.claim_txid,
                challenge_step: format!("{:?}", challenge_step),
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
    pub operators: Vec<OperatorStatus>,
    pub deposits: Vec<DepositInfo>,
    pub withdrawals: Vec<WithdrawalInfo>,
    pub reimbursements: Vec<ReimbursementInfo>,
}
