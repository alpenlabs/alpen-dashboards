use bitcoin::{PublicKey, Txid};
use serde::{Deserialize, Serialize};
use strata_bridge_primitives::types::DepositIdx;
use strata_bridge_rpc::types::{
    RpcClaimPhase, RpcDepositInfo, RpcDepositStatus, RpcOperatorStatus, RpcReimbursementStatus,
    RpcWithdrawalStatus,
};
use strata_primitives::buf::Buf32;

/// FIFO withdrawal-request sequence number.
pub type WithdrawalSeq = u64;

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

/// Cursor for withdrawal-to-deposit pairing progress.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WithdrawalPairingCursor {
    pub(crate) next_deposit_idx: DepositIdx,
    pub(crate) next_withdrawal_seq: WithdrawalSeq,
}

impl WithdrawalPairingCursor {
    pub fn new(next_deposit_idx: DepositIdx, next_withdrawal_seq: WithdrawalSeq) -> Self {
        Self {
            next_deposit_idx,
            next_withdrawal_seq,
        }
    }
}

/// Committed withdrawal-to-deposit pairing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WithdrawalPairing {
    pub(crate) deposit_idx: DepositIdx,
    pub(crate) withdrawal_seq: WithdrawalSeq,
}

impl WithdrawalPairing {
    pub fn new(deposit_idx: DepositIdx, withdrawal_seq: WithdrawalSeq) -> Self {
        Self {
            deposit_idx,
            withdrawal_seq,
        }
    }

    pub fn deposit_idx(&self) -> DepositIdx {
        self.deposit_idx
    }

    pub fn withdrawal_seq(&self) -> WithdrawalSeq {
        self.withdrawal_seq
    }
}

/// Cursor for bridge withdrawal status polling progress.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WithdrawalStatusCursor {
    pub(crate) next_deposit_idx: DepositIdx,
}

impl WithdrawalStatusCursor {
    pub fn new(next_deposit_idx: DepositIdx) -> Self {
        Self { next_deposit_idx }
    }
}

/// Cursor for bridge reimbursement status polling progress.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReimbursementStatusCursor {
    pub(crate) next_deposit_idx: DepositIdx,
}

impl ReimbursementStatusCursor {
    pub fn new(next_deposit_idx: DepositIdx) -> Self {
        Self { next_deposit_idx }
    }
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
pub enum WithdrawalStatus {
    #[serde(rename = "In progress")]
    InProgress,

    Complete,
}

/// Withdrawal information passed to status dashboard
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
pub struct WithdrawalInfo {
    pub(crate) withdrawal_request_txid: Buf32,
    pub(crate) fulfillment_txid: Option<Txid>,
    pub(crate) status: WithdrawalStatus,
}

impl WithdrawalInfo {
    pub fn new(
        withdrawal_request_txid: Buf32,
        fulfillment_txid: Option<Txid>,
        status: WithdrawalStatus,
    ) -> Self {
        Self {
            withdrawal_request_txid,
            fulfillment_txid,
            status,
        }
    }

    pub(crate) fn from_status(
        withdrawal_request_txid: Buf32,
        status: &RpcWithdrawalStatus,
    ) -> Self {
        match status {
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
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum ReimbursementStatus {
    #[serde(rename = "Not started")]
    NotStarted,

    #[serde(rename = "In progress")]
    InProgress,

    Slashed,

    Aborted,

    Complete,
}

/// Challenge step for reimbursements
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum ChallengeStep {
    #[serde(rename = "N/A")]
    NotApplicable,

    Claimed,

    Contested,

    #[serde(rename = "Bridge proof posted")]
    BridgeProofPosted,

    #[serde(rename = "Bridge proof timed out")]
    BridgeProofTimedout,

    #[serde(rename = "Counter proof posted")]
    CounterProofPosted,

    #[serde(rename = "All NACKed")]
    AllNackd,

    Acked,
}

/// Claim and reimbursement information passed to status dashboard
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct ReimbursementInfo {
    pub(crate) claim_txid: Txid,
    pub(crate) challenge_step: ChallengeStep,
    pub(crate) payout_txid: Option<Txid>,
    pub(crate) status: ReimbursementStatus,
}

impl ReimbursementInfo {
    pub(crate) fn from_status(status: &RpcReimbursementStatus) -> Option<Self> {
        match status {
            RpcReimbursementStatus::NotStarted => None,
            RpcReimbursementStatus::InProgress { claim_txid, phase } => Some(Self {
                claim_txid: *claim_txid,
                challenge_step: ChallengeStep::from(phase),
                payout_txid: None,
                status: ReimbursementStatus::InProgress,
            }),
            RpcReimbursementStatus::Slashed { claim_txid } => Some(Self {
                claim_txid: *claim_txid,
                challenge_step: ChallengeStep::NotApplicable,
                payout_txid: None,
                status: ReimbursementStatus::Slashed,
            }),
            RpcReimbursementStatus::Aborted { claim_txid } => Some(Self {
                claim_txid: *claim_txid,
                challenge_step: ChallengeStep::NotApplicable,
                payout_txid: None,
                status: ReimbursementStatus::Aborted,
            }),
            RpcReimbursementStatus::Complete {
                claim_txid,
                payout_txid,
            } => Some(Self {
                claim_txid: *claim_txid,
                challenge_step: ChallengeStep::NotApplicable,
                payout_txid: Some(*payout_txid),
                status: ReimbursementStatus::Complete,
            }),
        }
    }
}

impl From<&RpcClaimPhase> for ChallengeStep {
    fn from(value: &RpcClaimPhase) -> Self {
        match value {
            RpcClaimPhase::Claimed => Self::Claimed,
            RpcClaimPhase::Contested => Self::Contested,
            RpcClaimPhase::BridgeProofPosted => Self::BridgeProofPosted,
            RpcClaimPhase::BridgeProofTimedout => Self::BridgeProofTimedout,
            RpcClaimPhase::CounterProofPosted => Self::CounterProofPosted,
            RpcClaimPhase::AllNackd => Self::AllNackd,
            RpcClaimPhase::Acked => Self::Acked,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct BridgeStatus {
    pub(crate) operators: Vec<OperatorStatus>,
    pub(crate) deposits: Vec<DepositInfo>,
    pub(crate) withdrawals: Vec<WithdrawalInfo>,
    pub(crate) reimbursements: Vec<ReimbursementInfo>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::hashes::Hash;

    fn txid(byte: u8) -> Txid {
        Txid::from_byte_array([byte; 32])
    }

    #[test]
    fn reimbursement_info_from_status_maps_all_variants() {
        let claim_txid = txid(1);
        let payout_txid = txid(2);

        assert_eq!(
            ReimbursementInfo::from_status(&RpcReimbursementStatus::NotStarted),
            None
        );
        assert_eq!(
            ReimbursementInfo::from_status(&RpcReimbursementStatus::InProgress {
                claim_txid,
                phase: RpcClaimPhase::BridgeProofPosted,
            }),
            Some(ReimbursementInfo {
                claim_txid,
                challenge_step: ChallengeStep::BridgeProofPosted,
                payout_txid: None,
                status: ReimbursementStatus::InProgress,
            })
        );
        assert_eq!(
            ReimbursementInfo::from_status(&RpcReimbursementStatus::Slashed { claim_txid }),
            Some(ReimbursementInfo {
                claim_txid,
                challenge_step: ChallengeStep::NotApplicable,
                payout_txid: None,
                status: ReimbursementStatus::Slashed,
            })
        );
        assert_eq!(
            ReimbursementInfo::from_status(&RpcReimbursementStatus::Aborted { claim_txid }),
            Some(ReimbursementInfo {
                claim_txid,
                challenge_step: ChallengeStep::NotApplicable,
                payout_txid: None,
                status: ReimbursementStatus::Aborted,
            })
        );
        assert_eq!(
            ReimbursementInfo::from_status(&RpcReimbursementStatus::Complete {
                claim_txid,
                payout_txid,
            }),
            Some(ReimbursementInfo {
                claim_txid,
                challenge_step: ChallengeStep::NotApplicable,
                payout_txid: Some(payout_txid),
                status: ReimbursementStatus::Complete,
            })
        );
    }

    #[test]
    fn challenge_step_from_claim_phase_maps_all_variants() {
        let cases = [
            (RpcClaimPhase::Claimed, ChallengeStep::Claimed),
            (RpcClaimPhase::Contested, ChallengeStep::Contested),
            (
                RpcClaimPhase::BridgeProofPosted,
                ChallengeStep::BridgeProofPosted,
            ),
            (
                RpcClaimPhase::BridgeProofTimedout,
                ChallengeStep::BridgeProofTimedout,
            ),
            (
                RpcClaimPhase::CounterProofPosted,
                ChallengeStep::CounterProofPosted,
            ),
            (RpcClaimPhase::AllNackd, ChallengeStep::AllNackd),
            (RpcClaimPhase::Acked, ChallengeStep::Acked),
        ];

        for (rpc_phase, expected_step) in cases {
            assert_eq!(ChallengeStep::from(&rpc_phase), expected_step);
        }
    }

    #[test]
    fn deposit_info_carries_deposit_txid_only_when_complete() {
        let deposit_request_txid = txid(1);
        let deposit_txid = txid(2);
        let cases = [
            (
                RpcDepositStatus::InProgress,
                DepositStatus::InProgress,
                None,
            ),
            (
                RpcDepositStatus::Failed {
                    reason: "expired".to_owned(),
                },
                DepositStatus::Failed,
                None,
            ),
            (
                RpcDepositStatus::Complete { deposit_txid },
                DepositStatus::Complete,
                Some(deposit_txid),
            ),
        ];

        for (rpc_status, expected_status, expected_deposit_txid) in cases {
            let info = DepositInfo::from(&RpcDepositInfo {
                status: rpc_status,
                deposit_idx: 0,
                deposit_request_txid,
            });
            assert_eq!(
                std::mem::discriminant(&info.status),
                std::mem::discriminant(&expected_status)
            );
            assert_eq!(info.deposit_request_txid, deposit_request_txid);
            assert_eq!(info.deposit_txid, expected_deposit_txid);
        }
    }

    #[test]
    fn withdrawal_info_carries_fulfillment_txid_only_when_complete() {
        let withdrawal_request_txid = Buf32([1; 32]);
        let fulfillment_txid = txid(2);

        let in_progress =
            WithdrawalInfo::from_status(withdrawal_request_txid, &RpcWithdrawalStatus::InProgress);
        assert_eq!(in_progress.withdrawal_request_txid, withdrawal_request_txid);
        assert_eq!(in_progress.fulfillment_txid, None);
        assert!(matches!(in_progress.status, WithdrawalStatus::InProgress));

        let complete = WithdrawalInfo::from_status(
            withdrawal_request_txid,
            &RpcWithdrawalStatus::Complete { fulfillment_txid },
        );
        assert_eq!(complete.withdrawal_request_txid, withdrawal_request_txid);
        assert_eq!(complete.fulfillment_txid, Some(fulfillment_txid));
        assert!(matches!(complete.status, WithdrawalStatus::Complete));
    }
}
