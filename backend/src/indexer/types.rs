
/// Withdrawal request
#[derive(Debug, Clone)]
pub struct WithdrawalRequest {
    /// Withdrawal requst transaction ID
    pub txid: String,
    /// Amount
    pub amount: i64,           // in sats
    pub destination: String,   // BOSD descriptor
    pub block_number: i64,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Represents the state of an indexer task
#[derive(Debug, Clone)]
pub struct IndexerState {
    /// Unique identifier for indexer task
    pub id: String,
    /// Last scanned block number for this indexer task
    pub last_scanned_block: i64,
    /// Timestamp of the last update
    pub updated_at: chrono::DateTime<chrono::Utc>,
}
