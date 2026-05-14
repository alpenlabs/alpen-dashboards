use crate::db::{
    error::DbResult,
    types::{DbIndexerState, DbWithdrawalEventIndex, DbWithdrawalEventKey, DbWithdrawalRequest},
};

/// Storage contract for the EVM withdrawal-intent indexer.
///
/// Two concerns share this trait:
/// - the FIFO sequence of expanded `WithdrawalIntentEvent` requests (with
///   idempotent insertion via the event-key reverse index and a
///   `last_scanned_block` checkpoint), and
/// - the seq ↔ deposit_idx pairing populated once a withdrawal is matched
///   to a bridge deposit.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "pairing methods are consumed by the pairing task in a follow-up commit"
    )
)]
pub(crate) trait WithdrawalIndexerDb: Send + Sync {
    fn get_indexer_state(&self, task: &str) -> DbResult<Option<DbIndexerState>>;
    fn put_indexer_state(&self, task: &str, state: &DbIndexerState) -> DbResult<()>;

    fn insert_withdrawal_event(
        &self,
        requests: &[DbWithdrawalRequest],
    ) -> DbResult<DbWithdrawalEventIndex>;
    fn get_withdrawal_request(&self, seq: u64) -> DbResult<Option<DbWithdrawalRequest>>;
    fn get_withdrawal_event_index(
        &self,
        key: &DbWithdrawalEventKey,
    ) -> DbResult<Option<DbWithdrawalEventIndex>>;
    fn max_withdrawal_seq(&self) -> DbResult<Option<u64>>;

    fn insert_pairing(&self, seq: u64, deposit_idx: u32) -> DbResult<()>;
    fn get_deposit_idx(&self, seq: u64) -> DbResult<Option<u32>>;
    fn get_seq_by_deposit_idx(&self, deposit_idx: u32) -> DbResult<Option<u64>>;
    fn list_unpaired_seqs(&self) -> DbResult<Vec<u64>>;
}
