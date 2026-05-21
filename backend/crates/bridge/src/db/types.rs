//! Types shared by DB traits and implementations.

use serde::{Deserialize, Serialize};
use strata_primitives::buf::Buf32;

/// Indexer checkpoint. Tracks the highest block number that has been fully
/// processed; the next scan resumes from `last_scanned_block + 1`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DbIndexerState {
    pub(crate) last_scanned_block: u64,
}

/// Identity of one `WithdrawalIntentEvent` emitted on the EVM chain.
///
/// `(tx_hash, log_index)` is unique per chain — used to deduplicate the same
/// event when re-scanned (e.g. after restart or overlap). `log_index` is the
/// EVM RPC `logIndex` for the block log, not a transaction-local index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub(crate) struct DbWithdrawalEventKey {
    pub(crate) tx_hash: Buf32,
    pub(crate) log_index: u64,
}

impl From<&DbWithdrawalRequest> for DbWithdrawalEventKey {
    fn from(value: &DbWithdrawalRequest) -> Self {
        Self {
            tx_hash: value.tx_hash,
            log_index: value.log_index,
        }
    }
}

/// Reverse index entry for a fully persisted withdrawal-intent event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DbWithdrawalEventIndex {
    pub(crate) first_seq: u64,
    pub(crate) count: u32,
}

/// One single-denom withdrawal request expanded from a withdrawal-intent event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DbWithdrawalRequest {
    /// EVM transaction hash that emitted the event.
    pub(crate) tx_hash: Buf32,

    /// EVM RPC `logIndex` for the block log.
    pub(crate) log_index: u64,

    /// Withdrawal sub-index from the event payload (`WithdrawalIntent.idx`).
    pub(crate) sub_idx: u32,

    /// Withdrawal amount in satoshis.
    pub(crate) amount_sats: u64,

    /// Destination descriptor bytes (BOSD).
    pub(crate) destination: Vec<u8>,

    /// Operator selected by the bridge for this withdrawal.
    pub(crate) selected_operator: u32,

    /// EVM block number that contained the event.
    pub(crate) block_number: u64,
}

/// Indexed withdrawal request row returned from the withdrawal-index DB.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DbWithdrawalRequestRow {
    pub(crate) seq: u64,
    pub(crate) request: DbWithdrawalRequest,
}
