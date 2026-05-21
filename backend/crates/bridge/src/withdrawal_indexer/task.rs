//! Withdrawal indexer run loop.

use std::{sync::Arc, time::Duration};

use alloy_sol_types::SolEvent;
use alpen_reth_primitives::WithdrawalIntentEvent;
use anyhow::{anyhow, Result};
use status_config::WithdrawalIndexerConfig;
use strata_tasks::ShutdownGuard;
use tracing::{debug, info, warn};

use crate::db::{
    error::DbError, traits::WithdrawalIndexerDb, types::DbIndexerState,
    withdrawal_index::db::WithdrawalIndexerDbSled,
};

use super::{
    decoder::{self, DecodeError},
    rpc::{EthLogsClient, EthRpcError, JsonRpcEthClient, RpcLog},
    BRIDGEOUT_PRECOMPILE_ADDRESS, TASK_NAME,
};

#[derive(Debug, thiserror::Error)]
pub(crate) enum BatchError {
    #[error("duplicate log (tx {0:?}, log index {1})")]
    DuplicateLog(alloy_primitives::B256, u64),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum IndexerError {
    #[error("eth rpc: {0}")]
    Rpc(#[from] EthRpcError),

    #[error("decode: {0}")]
    Decode(#[from] DecodeError),

    #[error("batch: {0}")]
    Batch(#[from] BatchError),

    #[error("db: {0}")]
    Db(#[from] DbError),
}

/// Outcome of one indexer tick: did the scan reach `safe_head` (catching up
/// to the chain tip), or is there more history to backfill before the next
/// `poll_interval_s` sleep?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TickOutcome {
    /// Cursor reached `safe_head`; sleep `poll_interval_s` before next tick.
    CaughtUp,
    /// More backfill remains; loop immediately to the next batch.
    MoreWork,
}

/// Run the withdrawal indexer until `shutdown` is signalled.
///
/// Startup failures (DB open, RPC client) return an error so the caller
/// can treat the task as critical. Tick errors are logged and retried.
pub async fn run_withdrawal_indexer(
    db: Arc<WithdrawalIndexerDbSled>,
    cfg: WithdrawalIndexerConfig,
    shutdown: ShutdownGuard,
) -> Result<()> {
    let rpc = JsonRpcEthClient::new(cfg.eth_rpc_url())
        .map_err(|e| anyhow!("construct EVM RPC client: {e}"))?;
    run_with_rpc(db, rpc, cfg, shutdown).await
}

async fn run_with_rpc<D, R>(
    db: Arc<D>,
    rpc: R,
    cfg: WithdrawalIndexerConfig,
    shutdown: ShutdownGuard,
) -> Result<()>
where
    D: WithdrawalIndexerDb,
    R: EthLogsClient,
{
    info!(
        start_block = cfg.start_block(),
        finality_lag = cfg.finality_lag(),
        batch_size = cfg.batch_size(),
        "withdrawal indexer starting"
    );
    loop {
        // The indexer can stop mid-tick because writes are atomic per event
        // and replay is idempotent on restart. Process the tick result
        // inside the select arm rather than binding it: `IndexerError`
        // transitively wraps `Box<dyn Error>` (no `+ Send + Sync`) via
        // `typed_sled::CodecError`, so it must not survive into the next
        // await — only the `Send` `more_work` flag escapes.
        let mut more_work = false;
        tokio::select! {
            _ = shutdown.wait_for_shutdown() => break,
            result = tick(db.as_ref(), &rpc, &cfg) => match result {
                Ok(TickOutcome::MoreWork) => more_work = true,
                Ok(TickOutcome::CaughtUp) => {}
                Err(e) => warn!(error = %e, "withdrawal indexer tick failed; will retry"),
            }
        }

        if more_work {
            continue; // skip sleep, drain backfill
        }

        tokio::select! {
            _ = shutdown.wait_for_shutdown() => break,
            _ = tokio::time::sleep(Duration::from_secs(cfg.poll_interval_s())) => {}
        }
    }

    info!("withdrawal indexer shutting down");
    Ok(())
}

async fn tick<D, R>(
    db: &D,
    rpc: &R,
    cfg: &WithdrawalIndexerConfig,
) -> Result<TickOutcome, IndexerError>
where
    D: WithdrawalIndexerDb,
    R: EthLogsClient,
{
    let head = rpc.block_number().await?;
    let safe_head = head.saturating_sub(cfg.finality_lag());
    let from = match db.get_indexer_state(TASK_NAME)? {
        None => cfg.start_block(),
        Some(state) => state.last_scanned_block.saturating_add(1),
    };
    if safe_head < from {
        debug!(from, safe_head, head, "tip caught up; skipping");
        return Ok(TickOutcome::CaughtUp);
    }
    let to = (from + cfg.batch_size().saturating_sub(1)).min(safe_head);

    let mut logs = rpc
        .get_logs(
            from,
            to,
            BRIDGEOUT_PRECOMPILE_ADDRESS,
            WithdrawalIntentEvent::SIGNATURE_HASH,
        )
        .await?;
    logs.sort_by_key(|log| (log.block_number, log.log_index));
    debug!(from, to, log_count = logs.len(), "scanned range");

    if let Some(dup) = logs.windows(2).find(|w| {
        w[0].transaction_hash == w[1].transaction_hash && w[0].log_index == w[1].log_index
    }) {
        return Err(BatchError::DuplicateLog(dup[1].transaction_hash, dup[1].log_index).into());
    }

    for log in &logs {
        persist_log(db, log, cfg.withdrawal_denomination_sats())?;
    }

    db.put_indexer_state(
        TASK_NAME,
        &DbIndexerState {
            last_scanned_block: to,
        },
    )?;
    Ok(if to == safe_head {
        TickOutcome::CaughtUp
    } else {
        TickOutcome::MoreWork
    })
}

fn persist_log<D: WithdrawalIndexerDb>(
    db: &D,
    log: &RpcLog,
    withdrawal_denomination_sats: u64,
) -> Result<(), IndexerError> {
    let sub_units = decoder::decode(log, withdrawal_denomination_sats)?;
    db.insert_withdrawal_event(&sub_units)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::types::DbIndexerState;
    use crate::db::withdrawal_index::mock::MockWithdrawalIndexerDb;

    use alloy_primitives::{Bytes, LogData, B256};
    use std::sync::Mutex;

    const TEST_WITHDRAWAL_DENOMINATION_SATS: u64 = 100_000_000;

    /// In-process [`EthLogsClient`] driven by a queue of (head, logs) pairs.
    struct MockEthRpc {
        ticks: Mutex<Vec<(u64, Vec<RpcLog>)>>,
    }

    impl MockEthRpc {
        fn new(ticks: Vec<(u64, Vec<RpcLog>)>) -> Self {
            Self {
                ticks: Mutex::new(ticks),
            }
        }
    }

    impl EthLogsClient for MockEthRpc {
        async fn block_number(&self) -> Result<u64, EthRpcError> {
            let ticks = self.ticks.lock().expect("lock");
            Ok(ticks
                .first()
                .map(|(head, _)| *head)
                .expect("test must seed a tick"))
        }

        async fn get_logs(
            &self,
            _from: u64,
            _to: u64,
            _addr: alloy_primitives::Address,
            _topic0: B256,
        ) -> Result<Vec<RpcLog>, EthRpcError> {
            let mut ticks = self.ticks.lock().expect("lock");
            Ok(ticks
                .first_mut()
                .map(|(_, logs)| std::mem::take(logs))
                .unwrap_or_default())
        }
    }

    impl MockEthRpc {
        fn advance(&self) {
            let mut ticks = self.ticks.lock().expect("lock");
            if !ticks.is_empty() {
                ticks.remove(0);
            }
        }
    }

    fn make_log(amount: u64, tx_byte: u8, log_index: u64, block: u64) -> RpcLog {
        let evt = WithdrawalIntentEvent {
            amount,
            selectedOperator: 0,
            destination: Bytes::from(vec![0xCD; 22]),
        };
        let data = LogData::from(&evt);
        RpcLog {
            address: BRIDGEOUT_PRECOMPILE_ADDRESS,
            topics: data.topics().to_vec(),
            data: data.data.to_vec(),
            block_number: block,
            transaction_hash: B256::repeat_byte(tx_byte),
            log_index,
        }
    }

    fn cfg() -> WithdrawalIndexerConfig {
        let toml = r#"
eth_rpc_url = ""
batch_size = 1000
finality_lag = 0
start_block = 0
poll_interval_s = 0
withdrawal_denomination_sats = 100000000
"#;
        toml::from_str(toml).expect("parse cfg")
    }

    #[tokio::test]
    async fn empty_tip_no_writes() {
        let db = MockWithdrawalIndexerDb::default();
        // head = 0, finality_lag = 0 → safe_head = 0; first tick scans block 0.
        let rpc = MockEthRpc::new(vec![(0, vec![])]);
        tick(&db, &rpc, &cfg()).await.expect("tick");
        assert_eq!(db.max_withdrawal_seq().expect("max"), None);
        assert_eq!(
            db.get_indexer_state(TASK_NAME).expect("state"),
            Some(DbIndexerState {
                last_scanned_block: 0
            }),
            "cursor should advance after scanning block 0"
        );
    }

    #[tokio::test]
    async fn writes_sub_units_and_advances_cursor() {
        let db = MockWithdrawalIndexerDb::default();
        let rpc = MockEthRpc::new(vec![(
            10,
            vec![make_log(2 * TEST_WITHDRAWAL_DENOMINATION_SATS, 0xAA, 1, 5)],
        )]);
        tick(&db, &rpc, &cfg()).await.expect("tick");
        assert_eq!(db.max_withdrawal_seq().expect("max"), Some(1));
        let rows = db.fetch_withdrawal_requests_from(0, 2).expect("fetch rows");
        let row0 = &rows[0].request;
        let row1 = &rows[1].request;
        assert_eq!(row0.sub_idx, 0);
        assert_eq!(row1.sub_idx, 1);
        let state = db
            .get_indexer_state(TASK_NAME)
            .expect("state")
            .expect("present");
        assert_eq!(state.last_scanned_block, 10);
    }

    #[tokio::test]
    async fn restart_replay_is_idempotent() {
        let db = MockWithdrawalIndexerDb::default();
        let log = make_log(TEST_WITHDRAWAL_DENOMINATION_SATS, 0xBB, 2, 7);

        // First tick: scans 1..=10, persists the log, advances cursor to 10.
        let rpc = MockEthRpc::new(vec![(10, vec![log.clone()])]);
        tick(&db, &rpc, &cfg()).await.expect("tick 1");
        assert_eq!(db.max_withdrawal_seq().expect("max"), Some(0));

        // Simulate a crash that lost the cursor advance: rewind state to 0.
        db.put_indexer_state(
            TASK_NAME,
            &DbIndexerState {
                last_scanned_block: 0,
            },
        )
        .expect("rewind cursor");

        // Replay: same log returned again. persist_log must dedupe.
        rpc.advance();
        let rpc2 = MockEthRpc::new(vec![(10, vec![log])]);
        tick(&db, &rpc2, &cfg()).await.expect("tick 2");
        assert_eq!(
            db.max_withdrawal_seq().expect("max"),
            Some(0),
            "replay must not produce a duplicate seq"
        );
    }

    #[tokio::test]
    async fn logs_are_persisted_in_block_then_log_order() {
        let db = MockWithdrawalIndexerDb::default();
        let rpc = MockEthRpc::new(vec![(
            10,
            vec![
                make_log(TEST_WITHDRAWAL_DENOMINATION_SATS, 0xCC, 7, 6),
                make_log(TEST_WITHDRAWAL_DENOMINATION_SATS, 0xAA, 9, 5),
                make_log(TEST_WITHDRAWAL_DENOMINATION_SATS, 0xBB, 3, 5),
            ],
        )]);

        tick(&db, &rpc, &cfg()).await.expect("tick");

        let rows = db.fetch_withdrawal_requests_from(0, 3).expect("fetch rows");
        let seq0 = &rows[0].request;
        let seq1 = &rows[1].request;
        let seq2 = &rows[2].request;

        assert_eq!(seq0.tx_hash.0, [0xBB; 32]);
        assert_eq!(seq0.block_number, 5);
        assert_eq!(seq0.log_index, 3);

        assert_eq!(seq1.tx_hash.0, [0xAA; 32]);
        assert_eq!(seq1.block_number, 5);
        assert_eq!(seq1.log_index, 9);

        assert_eq!(seq2.tx_hash.0, [0xCC; 32]);
        assert_eq!(seq2.block_number, 6);
        assert_eq!(seq2.log_index, 7);
    }

    fn cfg_with(batch_size: u64) -> WithdrawalIndexerConfig {
        let toml = format!(
            r#"
eth_rpc_url = ""
batch_size = {batch_size}
finality_lag = 0
start_block = 0
poll_interval_s = 0
withdrawal_denomination_sats = 100000000
"#
        );
        toml::from_str(&toml).expect("parse cfg")
    }

    #[tokio::test]
    async fn tick_signals_more_work_during_backfill() {
        let db = MockWithdrawalIndexerDb::default();
        // safe_head = 100, batch_size = 50 → first tick scans [0, 49], cursor < safe_head.
        let rpc = MockEthRpc::new(vec![(100, vec![])]);
        let outcome = tick(&db, &rpc, &cfg_with(50)).await.expect("tick");
        assert_eq!(outcome, TickOutcome::MoreWork);
        assert_eq!(
            db.get_indexer_state(TASK_NAME)
                .expect("state")
                .expect("present")
                .last_scanned_block,
            49
        );
    }

    #[tokio::test]
    async fn rejects_duplicate_log_in_batch() {
        let db = MockWithdrawalIndexerDb::default();
        let dup = make_log(TEST_WITHDRAWAL_DENOMINATION_SATS, 0xAA, 5, 7);
        let rpc = MockEthRpc::new(vec![(10, vec![dup.clone(), dup])]);
        let err = tick(&db, &rpc, &cfg()).await.expect_err("expected error");
        assert!(matches!(
            err,
            IndexerError::Batch(BatchError::DuplicateLog(_, 5))
        ));
        assert_eq!(db.max_withdrawal_seq().expect("max"), None);
    }

    #[tokio::test]
    async fn tick_signals_caught_up_at_safe_head() {
        let db = MockWithdrawalIndexerDb::default();
        // safe_head = 10, batch_size = 1000 → tick reaches safe_head in one batch.
        let rpc = MockEthRpc::new(vec![(10, vec![])]);
        let outcome = tick(&db, &rpc, &cfg()).await.expect("tick");
        assert_eq!(outcome, TickOutcome::CaughtUp);
    }
}
