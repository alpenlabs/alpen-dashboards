use crate::db::{
    error::DbResult,
    types::{DbIndexerState, DbWithdrawalEventIndex, DbWithdrawalRequest, DbWithdrawalRequestRow},
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
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "used when bridge status consumes indexed withdrawals"
        )
    )]
    fn fetch_withdrawal_requests_from(
        &self,
        start_seq: u64,
        limit: usize,
    ) -> DbResult<Vec<DbWithdrawalRequestRow>>;

    /// Returns the largest indexed withdrawal sequence number.
    fn max_withdrawal_seq(&self) -> DbResult<Option<u64>>;
}
