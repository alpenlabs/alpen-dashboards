use sqlx::{SqlitePool, Result};
use crate::types::DepositRecord;

/// Insert or update a deposit entry in the indexer DB
pub async fn upsert_deposit(db: &SqlitePool, record: &DepositRecord) -> Result<()> {
    sqlx::query!(
        r#"
        INSERT INTO deposits (
            deposit_outpoint,
            deposit_request_txid,
            deposit_txid,
            deposit_block_height,
            current_block_height,
            confirmation_depth,
            status,
            last_checked,
            alpen_address
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(deposit_outpoint) DO UPDATE SET
            deposit_request_txid = excluded.deposit_request_txid,
            deposit_txid = excluded.deposit_txid,
            deposit_block_height = excluded.deposit_block_height,
            current_block_height = excluded.current_block_height,
            confirmation_depth = excluded.confirmation_depth,
            status = excluded.status,
            last_checked = excluded.last_checked,
            alpen_address = excluded.alpen_address
        "#,
        record.deposit_outpoint,
        record.deposit_request_txid,
        record.deposit_txid.as_deref(),
        record.deposit_block_height,
        record.current_block_height,
        record.confirmation_depth,
        record.status,
        record.last_checked,
        record.alpen_address.as_deref()
    )
    .execute(db)
    .await?;

    Ok(())
}

/// Retrieve all current deposit entries
pub async fn get_all_deposits(db: &SqlitePool) -> Result<Vec<DepositRecord>> {
    let rows = sqlx::query_as!(
        DepositRecord,
        r#"
        SELECT
            deposit_outpoint,
            deposit_request_txid,
            deposit_txid,
            deposit_block_height,
            current_block_height,
            confirmation_depth,
            status,
            last_checked,
            alpen_address
        FROM deposits
        "#
    )
    .fetch_all(db)
    .await?;

    Ok(rows)
}
