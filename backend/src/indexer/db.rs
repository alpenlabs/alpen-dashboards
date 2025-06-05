use sqlx::{sqlite::SqlitePoolOptions, SqlitePool};
use std::time::Duration;

use crate::indexer::models::{WithdrawalRequest, IndexerState};
use crate::indexer::types::{DbTxid, DbAmount, DbDescriptor,
    DbBlockNumber, DbTaskId, DbTimestamp};

/// Initialize the SQLite connection pool.
pub async fn init_pool(database_url: &str) -> Result<SqlitePool, sqlx::Error> {
    SqlitePoolOptions::new()
        .acquire_timeout(Duration::from_secs(5))
        .connect(database_url)
        .await
}

/// Runs all necessary migrations for the indexer.
pub async fn run_migrations(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::migrate!("./migrations").run(pool).await?;
    Ok(())
}

/// Insert withdrawal request into the database.
pub async fn insert_withdrawal_request(
    pool: &SqlitePool,
    record: &WithdrawalRequest,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        INSERT OR IGNORE INTO withdrawal_requests (txid, amount, destination, block_number, timestamp)
        VALUES (?1, ?2, ?3, ?4, ?5)
        "#,
        record.txid,
        record.amount,
        record.destination,
        record.block_number,
        record.timestamp,
    )
    .execute(pool)
    .await?;

    Ok(())
}

/// Fetch a withdrawal request by its transaction ID.
pub async fn get_withdrawal_request_by_txid(
    pool: &SqlitePool,
    txid: &DbTxid,
) -> Result<Option<WithdrawalRequest>, sqlx::Error> {
    let record = sqlx::query_as!(
        WithdrawalRequest,
        r#"
        SELECT
            txid as "txid: DbTxid",
            amount as "amount: DbAmount",
            destination as "destination: DbDescriptor",
            block_number as "block_number: DbBlockNumber",
            timestamp as "timestamp: DbTimestamp"
        FROM withdrawal_requests
        WHERE txid = ?1
        "#,
        txid
    )
    .fetch_optional(pool)
    .await?;

    Ok(record)
}

/// Get the last scanned block for a specific indexer task id.
pub async fn get_indexer_state(
    pool: &SqlitePool,
    task_id: &DbTaskId,
) -> Result<IndexerState, sqlx::Error> {
    sqlx::query_as!(
        IndexerState,
        r#"
        SELECT task_id as "task_id!: DbTaskId", last_scanned_block, updated_at
        FROM indexer_state
        WHERE task_id = ?
        "#,
        task_id
)
    .fetch_one(pool)
    .await
}

/// Update the last scanned block for a specific indexer task id.
pub async fn update_indexer_state(
    pool: &SqlitePool,
    state: &IndexerState,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        UPDATE indexer_state
        SET last_scanned_block = ?, updated_at = CURRENT_TIMESTAMP
        WHERE task_id = ?
        "#,
        state.last_scanned_block,
        state.task_id
    )
    .execute(pool)
    .await?;
    Ok(())
}
