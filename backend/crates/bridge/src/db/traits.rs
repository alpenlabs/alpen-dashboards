use strata_bridge_primitives::types::DepositIdx;

use crate::{
    db::{
        error::DbResult,
        types::{
            DbBridgeStatusSnapshot, DbIndexerState, DbWithdrawalEventIndex, DbWithdrawalRequest,
            DbWithdrawalRequestRow,
        },
    },
    types::{
        ReimbursementStatusCursor, WithdrawalInfo, WithdrawalPairingCursor, WithdrawalSeq,
        WithdrawalStatusCursor,
    },
};

/// Storage contract for indexed EVM withdrawal-intent events.
///
/// This trait owns the FIFO sequence of expanded `WithdrawalIntentEvent`
/// requests, idempotent insertion via the event-key reverse index, and the
/// indexer's [`DbIndexerState`].
pub(crate) trait WithdrawalIndexerDb: Send + Sync {
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
pub(crate) trait BridgeStatusDb: Send + Sync {
    /// Loads all persisted status rows, pairings, and cursors.
    fn get_status_snapshot(&self) -> DbResult<DbBridgeStatusSnapshot>;

    /// Inserts or replaces one withdrawal status row.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "wired into state in follow-up commits")
    )]
    fn put_withdrawal_info(&self, deposit_idx: DepositIdx, info: &WithdrawalInfo) -> DbResult<()>;

    /// Deletes one withdrawal status row.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "wired into state in follow-up commits")
    )]
    fn del_withdrawal_info(&self, deposit_idx: DepositIdx) -> DbResult<bool>;

    /// Inserts or replaces withdrawal-to-deposit pairings.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "wired into state in follow-up commits")
    )]
    fn put_withdrawal_pairings(&self, pairings: &[(DepositIdx, WithdrawalSeq)]) -> DbResult<()>;

    /// Deletes withdrawal-to-deposit pairing rows in `start..end`.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "wired into state in follow-up commits")
    )]
    fn del_withdrawal_pairings_range(&self, start: DepositIdx, end: DepositIdx) -> DbResult<()>;

    /// Stores the deposit-info polling cursor.
    fn put_deposit_info_cursor(&self, cursor: DepositIdx) -> DbResult<()>;

    /// Stores the withdrawal-pairing cursor.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "wired into state in follow-up commits")
    )]
    fn put_withdrawal_pairing_cursor(&self, cursor: WithdrawalPairingCursor) -> DbResult<()>;

    /// Stores the withdrawal-status polling cursor.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "wired into state in follow-up commits")
    )]
    fn put_withdrawal_status_cursor(&self, cursor: WithdrawalStatusCursor) -> DbResult<()>;

    /// Stores the reimbursement-status polling cursor.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "wired into state in follow-up commits")
    )]
    fn put_reimbursement_status_cursor(&self, cursor: ReimbursementStatusCursor) -> DbResult<()>;
}
