use sqlx::{sqlite::SqlitePoolOptions, SqlitePool};
use std::time::Duration;

#[derive(Debug, sqlx::FromRow)]
pub struct WithdrawalRequest {
    pub txid: String,
    pub amount: i64, // in sats
    pub destination: String,
    pub block_number: i64,
    pub timestamp: String, // optional, or use chrono::DateTime<Utc>
}

/// Initialize the SQLite connection pool.
pub async fn init_pool(database_url: &str) -> Result<SqlitePool, sqlx::Error> {
    SqlitePoolOptions::new()
        .connect_timeout(Duration::from_secs(5))
        .connect(database_url)
        .await
}

/// Insert withdrawal request into the database.
pub async fn insert_withdrawal_request(
    pool: &SqlitePool,
    txid: &str,
    amount: i64,
    destination: &str,
    block_number: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        INSERT OR IGNORE INTO withdrawal_requests (txid, amount, destination, block_number)
        VALUES (?, ?, ?, ?)
        "#,
        txid,
        amount,
        destination,
        block_number,
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Get the last scanned block for a specific indexer task id.
pub async fn get_last_scanned_block(pool: &SqlitePool, indexer_id: &str) -> Result<i64, sqlx::Error> {
    let row = sqlx::query_scalar!(
        r#"
        SELECT last_scanned_block
        FROM indexer_state
        WHERE id = ?
        "#,
        indexer_id
    )
    .fetch_optional(pool)
    .await?;

    // default to block 0 if not found
    Ok(row.unwrap_or(0))
}

/// Update the last scanned block for a specific indexer task id.
pub async fn update_last_scanned_block(
    pool: &SqlitePool,
    id: &str,
    block: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        UPDATE indexer_state
        SET last_scanned_block = ?, updated_at = CURRENT_TIMESTAMP
        WHERE id = ?
        "#,
        block,
        id
    )
    .execute(pool)
    .await?;

    Ok(())
}
