use bitcoin::OutPoint;
use strata_bridge_primitives::types::DepositIdx;
use strata_identifiers::L1BlockCommitment;

use crate::{
    db::{
        error::DbResult,
        types::{
            BitcoinObserverDbMetadata, BitcoinObserverState, BridgeStatusDbSnapshot,
            DbIndexerState, DbWithdrawalEventIndex, DbWithdrawalRequest, DbWithdrawalRequestRow,
        },
    },
    l1::observations::ObservedDeposit,
    types::{
        ReimbursementStatusCursor, WithdrawalInfo, WithdrawalPairing, WithdrawalPairingCursor,
        WithdrawalStatusCursor,
    },
};

/// Storage contract for indexed EVM withdrawal-intent events.
///
/// This trait owns the FIFO sequence of expanded `WithdrawalIntentEvent`
/// requests, idempotent insertion via the event-key reverse index, and the
/// indexer's [`DbIndexerState`].
pub trait WithdrawalIndexerDb: Send + Sync + 'static {
    /// Returns the stored state for an indexer task.
    fn get_indexer_state(&self, task: &str) -> DbResult<Option<DbIndexerState>>;

    /// Stores the state for an indexer task.
    fn put_indexer_state(&self, task: &str, state: &DbIndexerState) -> DbResult<()>;

    /// Inserts one EVM withdrawal-intent event expanded into FIFO requests.
    ///
    /// Replaying the same event is idempotent and returns the existing
    /// persisted sequence range.
    fn insert_withdrawal_event(
        &self,
        requests: &[DbWithdrawalRequest],
    ) -> DbResult<DbWithdrawalEventIndex>;

    /// Fetches indexed withdrawal requests in ascending FIFO order.
    ///
    /// The result starts at `start_seq` and contains at most `limit` rows.
    fn fetch_withdrawal_requests_from(
        &self,
        start_seq: u64,
        limit: usize,
    ) -> DbResult<Vec<DbWithdrawalRequestRow>>;

    /// Returns the largest indexed withdrawal sequence number.
    fn max_withdrawal_seq(&self) -> DbResult<Option<u64>>;
}

/// Storage contract for bridge status rows, pairings, and cursors.
pub trait BridgeStatusDb: Send + Sync + 'static {
    /// Loads all persisted status rows, pairings, and cursors.
    fn get_status_snapshot(&self) -> DbResult<BridgeStatusDbSnapshot>;

    /// Inserts or replaces one withdrawal status row.
    fn put_withdrawal_info(&self, deposit_idx: DepositIdx, info: &WithdrawalInfo) -> DbResult<()>;

    /// Deletes one withdrawal status row.
    fn del_withdrawal_info(&self, deposit_idx: DepositIdx) -> DbResult<bool>;

    /// Inserts or replaces withdrawal-to-deposit pairings.
    fn put_withdrawal_pairings(&self, pairings: &[WithdrawalPairing]) -> DbResult<()>;

    /// Deletes withdrawal-to-deposit pairing rows in `start..end`.
    fn del_withdrawal_pairings_range(&self, start: DepositIdx, end: DepositIdx) -> DbResult<()>;

    /// Stores the deposit-info polling cursor.
    fn put_deposit_info_cursor(&self, cursor: DepositIdx) -> DbResult<()>;

    /// Stores the withdrawal-pairing cursor.
    fn put_withdrawal_pairing_cursor(&self, cursor: WithdrawalPairingCursor) -> DbResult<()>;

    /// Stores the withdrawal-status polling cursor.
    fn put_withdrawal_status_cursor(&self, cursor: WithdrawalStatusCursor) -> DbResult<()>;

    /// Stores the reimbursement-status polling cursor.
    fn put_reimbursement_status_cursor(&self, cursor: ReimbursementStatusCursor) -> DbResult<()>;
}

/// Storage contract for a single Bitcoin observer writer and concurrent readers.
/// Each mutation includes observer state and must be durable before returning success.
pub trait BitcoinObserverDb: Send + Sync + 'static {
    /// Loads the stored scan metadata and last successfully processed Bitcoin block.
    fn get_observer_state(&self) -> DbResult<Option<BitcoinObserverState>>;

    /// Initializes fresh storage at the scan anchor, or validates existing metadata.
    /// Existing progress is preserved when metadata matches.
    fn initialize(
        &self,
        metadata: &BitcoinObserverDbMetadata,
        anchor: Option<L1BlockCommitment>,
    ) -> DbResult<()>;

    /// Returns a request and its observed spend by the request output.
    fn get_deposit(&self, outpoint: OutPoint) -> DbResult<Option<ObservedDeposit>>;

    /// Reads at most limit observations after an exclusive outpoint cursor, in encoded-key order.
    /// Callers must verify the observer revision across a multi-page read.
    fn get_deposits_from(
        &self,
        after: Option<OutPoint>,
        limit: usize,
    ) -> DbResult<Vec<ObservedDeposit>>;

    /// Returns the current tip followed by available rollback targets, newest first.
    fn rollback_history(&self, limit: u32) -> DbResult<Vec<Option<L1BlockCommitment>>>;

    /// Atomically stores changed rows, their undo record and the new last processed block.
    /// The block must extend parent. Replaying the current block is a no-op only
    /// when its parent and exact set of observation values match.
    fn apply_block(
        &self,
        parent: Option<L1BlockCommitment>,
        block: L1BlockCommitment,
        rows: &[ObservedDeposit],
        reorg_safe_depth: u32,
    ) -> DbResult<()>;

    /// Restores the previous rows and block, provided expected is still the stored tip.
    fn rollback_tip(&self, expected: L1BlockCommitment) -> DbResult<()>;
}
