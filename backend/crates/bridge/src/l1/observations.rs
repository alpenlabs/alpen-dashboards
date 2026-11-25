//! Persisted Bitcoin evidence. Confirmation counts are derived from the canonical tip.

use bitcoin::{OutPoint, Transaction};
use serde::{Deserialize, Serialize};
use strata_asm_common::L1TxIndex;
use strata_asm_proto_bridge_v1_txs::deposit_request::DRT_OUTPUT_INDEX;
use strata_identifiers::L1BlockCommitment;

use super::deposits::DepositSpend;

/// Position of a transaction in its containing block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransactionInclusion {
    pub(crate) block: L1BlockCommitment,
    pub(crate) position: L1TxIndex,
}

impl TransactionInclusion {
    pub fn new(block: L1BlockCommitment, position: L1TxIndex) -> Self {
        Self { block, position }
    }
}

/// A classified request spend and its Bitcoin inclusion record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedDepositSpend {
    pub(crate) inclusion: TransactionInclusion,
    pub(crate) spend: DepositSpend,
}

impl ObservedDepositSpend {
    pub fn new(inclusion: TransactionInclusion, spend: DepositSpend) -> Self {
        Self { inclusion, spend }
    }
}

/// A request and its optional spend, identified by the request's output rather than a bridge index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedDeposit {
    pub(crate) request: Transaction,
    pub(crate) inclusion: TransactionInclusion,
    pub(crate) spend: Option<ObservedDepositSpend>,
}

impl ObservedDeposit {
    pub fn new(
        request: Transaction,
        inclusion: TransactionInclusion,
        spend: Option<ObservedDepositSpend>,
    ) -> Self {
        Self {
            request,
            inclusion,
            spend,
        }
    }

    pub fn outpoint(&self) -> OutPoint {
        OutPoint::new(self.request.compute_txid(), DRT_OUTPUT_INDEX as u32)
    }
}
