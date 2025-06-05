use crate::indexer::types::{DbTxid, DbAmount, DbDescriptor,
    DbBlockNumber, DbTaskId, DbTimestamp};

/// Withdrawal request stored in the database.
#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct WithdrawalRequest {
    /// EVM transaction hash (i.e. the withdrawal request txid)
    pub(crate) txid: DbTxid,

    /// Amount in satoshis
    pub(crate) amount: DbAmount,

    /// BOSD descriptor for withdrawal destination (serialized as hex or base64 for storage)
    pub(crate) destination: DbDescriptor,

    /// Alpen block number containing the withdrawal request
    pub(crate) block_number: DbBlockNumber,

    /// Timestamp of the withdrawal request record (insertion time)
    pub(crate) timestamp: DbTimestamp,
}

/// Represents the state of an indexer task
#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct IndexerState {
    /// Unique identifier for indexer task
    pub(crate) task_id: DbTaskId,
    /// Last scanned block number for this indexer task
    pub(crate) last_scanned_block: DbBlockNumber,
    /// Timestamp of the last update
    pub(crate) updated_at: DbTimestamp,
}
