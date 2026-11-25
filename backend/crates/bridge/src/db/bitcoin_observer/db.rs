//! Atomic Bitcoin observation writes and bounded per-block undo history.

use std::{collections::BTreeSet, ops::Bound, path::Path};

use anyhow::Context;
use bitcoin::OutPoint;
use sled::transaction::{ConflictableTransactionResult, TransactionError, TransactionResult};
use strata_identifiers::L1BlockCommitment;
use typed_sled::{
    error::Error as TypedSledError, transaction::SledTransactional, SledDb, SledTree,
};

use super::schema::{DepositSchema, ObserverStateSchema, UndoBlock, UndoSchema};
use crate::{
    db::{
        error::{BitcoinObserverConsistencyError, DbResult},
        traits::BitcoinObserverDb,
        types::{BitcoinObserverDbMetadata, BitcoinObserverState},
    },
    l1::observations::ObservedDeposit,
};

const OBSERVER_STATE_KEY: u8 = 0;

/// Sled-backed observations under datadir/bitcoin_observer.
/// Each block's observations, undo record and observer_state commit in one transaction.
pub struct BitcoinObserverDbSled {
    _db: SledDb,
    observer_state: SledTree<ObserverStateSchema>,
    deposits: SledTree<DepositSchema>,
    undo: SledTree<UndoSchema>,
}

impl BitcoinObserverDbSled {
    /// Open the observer database under datadir/bitcoin_observer.
    pub fn open(datadir: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = datadir.as_ref().join("bitcoin_observer");
        std::fs::create_dir_all(&path)
            .with_context(|| format!("create Bitcoin observer db dir {}", path.display()))?;
        let sled_db = sled::open(&path)
            .with_context(|| format!("open Bitcoin observer sled db at {}", path.display()))?;
        // typed-sled codec errors are not Send + Sync, so preserve their description at this boundary.
        Self::from_sled_db(sled_db)
            .map_err(|error| anyhow::anyhow!("initialize Bitcoin observer trees: {error}"))
    }

    #[cfg(test)]
    pub(crate) fn open_temporary() -> anyhow::Result<Self> {
        let sled_db = sled::Config::new()
            .temporary(true)
            .open()
            .context("open temporary Bitcoin observer database")?;
        Self::from_sled_db(sled_db).map_err(|error| {
            anyhow::anyhow!("initialize temporary Bitcoin observer trees: {error}")
        })
    }

    fn from_sled_db(sled_db: sled::Db) -> DbResult<Self> {
        let db = SledDb::new(sled_db)?;
        Ok(Self {
            observer_state: db.get_tree::<ObserverStateSchema>()?,
            deposits: db.get_tree::<DepositSchema>()?,
            undo: db.get_tree::<UndoSchema>()?,
            _db: db,
        })
    }
}

impl BitcoinObserverDb for BitcoinObserverDbSled {
    fn get_observer_state(&self) -> DbResult<Option<BitcoinObserverState>> {
        Ok(self.observer_state.get(&OBSERVER_STATE_KEY)?)
    }

    fn initialize(
        &self,
        metadata: &BitcoinObserverDbMetadata,
        anchor: Option<L1BlockCommitment>,
    ) -> DbResult<()> {
        map_tx_result((&self.observer_state,).transaction(|(observer_state,)| {
            if let Some(stored) = observer_state.get(&OBSERVER_STATE_KEY)? {
                if stored.metadata.genesis_hash != metadata.genesis_hash {
                    return abort_tx(BitcoinObserverConsistencyError::NetworkMismatch);
                }
                if stored.metadata.params_hash != metadata.params_hash {
                    return abort_tx(BitcoinObserverConsistencyError::ParamsMismatch);
                }
            } else {
                observer_state.insert(
                    &OBSERVER_STATE_KEY,
                    &BitcoinObserverState {
                        metadata: metadata.clone(),
                        last_processed_block: anchor,
                        revision: 0,
                    },
                )?;
            }
            Ok(())
        }))
    }

    fn get_deposit(&self, outpoint: OutPoint) -> DbResult<Option<ObservedDeposit>> {
        Ok(self.deposits.get(&outpoint)?)
    }

    fn get_deposits_from(
        &self,
        after: Option<OutPoint>,
        limit: usize,
    ) -> DbResult<Vec<ObservedDeposit>> {
        let start = after.map_or(Bound::Unbounded, Bound::Excluded);
        let mut rows = Vec::new();
        for entry in self.deposits.range((start, Bound::Unbounded))?.take(limit) {
            rows.push(entry?.1);
        }
        Ok(rows)
    }

    fn rollback_history(&self, limit: u32) -> DbResult<Vec<Option<L1BlockCommitment>>> {
        map_tx_result(
            (&self.observer_state, &self.undo).transaction(|(observer_state, undo)| {
                let Some(stored) = observer_state.get(&OBSERVER_STATE_KEY)? else {
                    return abort_tx(BitcoinObserverConsistencyError::ObserverStateMismatch);
                };
                let mut tip = stored.last_processed_block;
                let mut history = vec![tip];
                for _ in 0..limit {
                    let Some(block) = tip else {
                        break;
                    };
                    let Some(record) = undo.get(&block.height)? else {
                        break;
                    };
                    if record.block != block {
                        return abort_tx(BitcoinObserverConsistencyError::ObserverStateMismatch);
                    }
                    tip = record.parent;
                    history.push(tip);
                }
                Ok(history)
            }),
        )
    }

    fn apply_block(
        &self,
        parent: Option<L1BlockCommitment>,
        block: L1BlockCommitment,
        rows: &[ObservedDeposit],
        window: u32,
    ) -> DbResult<()> {
        map_tx_result(
            (&self.observer_state, &self.deposits, &self.undo).transaction(
                |(observer_state, deposits, undo)| {
                    let Some(mut stored) = observer_state.get(&OBSERVER_STATE_KEY)? else {
                        return abort_tx(BitcoinObserverConsistencyError::ObserverStateMismatch);
                    };
                    if stored.last_processed_block == Some(block) {
                        let Some(record) = undo.get(&block.height)? else {
                            return abort_tx(BitcoinObserverConsistencyError::MissingUndo);
                        };
                        if record.block != block
                            || record.parent != parent
                            || record.rows.len() != rows.len()
                        {
                            return abort_tx(
                                BitcoinObserverConsistencyError::ConflictingBlockReplay(block),
                            );
                        }
                        let mut expected_outpoints = record
                            .rows
                            .iter()
                            .map(|(outpoint, _)| *outpoint)
                            .collect::<BTreeSet<_>>();
                        for row in rows {
                            let outpoint = row.outpoint();
                            if !expected_outpoints.remove(&outpoint)
                                || deposits.get(&outpoint)?.as_ref() != Some(row)
                            {
                                return abort_tx(
                                    BitcoinObserverConsistencyError::ConflictingBlockReplay(block),
                                );
                            }
                        }
                        return Ok(());
                    }
                    if stored.last_processed_block != parent
                        || parent.map_or(Some(0), |p| p.height.checked_add(1)) != Some(block.height)
                        || window == 0
                    {
                        return abort_tx(BitcoinObserverConsistencyError::ObserverStateMismatch);
                    }
                    let mut previous = Vec::with_capacity(rows.len());
                    let mut seen = BTreeSet::new();
                    for row in rows {
                        let key = row.outpoint();
                        if !seen.insert(key) {
                            return abort_tx(
                                BitcoinObserverConsistencyError::ObserverStateMismatch,
                            );
                        }
                        previous.push((key, deposits.get(&key)?));
                        deposits.insert(&key, row)?;
                    }
                    undo.insert(
                        &block.height,
                        &UndoBlock {
                            parent,
                            block,
                            rows: previous,
                        },
                    )?;
                    if let Some(expired) = block.height.checked_sub(window) {
                        undo.remove(&expired)?;
                    }
                    stored.revision = stored.revision.checked_add(1).ok_or_else(|| {
                        TypedSledError::abort(BitcoinObserverConsistencyError::RevisionOverflow)
                    })?;
                    stored.last_processed_block = Some(block);
                    observer_state.insert(&OBSERVER_STATE_KEY, &stored)?;
                    Ok(())
                },
            ),
        )
    }

    fn rollback_tip(&self, expected: L1BlockCommitment) -> DbResult<()> {
        map_tx_result(
            (&self.observer_state, &self.deposits, &self.undo).transaction(
                |(observer_state, deposits, undo)| {
                    let Some(mut stored) = observer_state.get(&OBSERVER_STATE_KEY)? else {
                        return abort_tx(BitcoinObserverConsistencyError::ObserverStateMismatch);
                    };
                    if stored.last_processed_block != Some(expected) {
                        return abort_tx(BitcoinObserverConsistencyError::ObserverStateMismatch);
                    }
                    let Some(record) = undo.get(&expected.height)? else {
                        return abort_tx(BitcoinObserverConsistencyError::MissingUndo);
                    };
                    if record.block != expected {
                        return abort_tx(BitcoinObserverConsistencyError::ObserverStateMismatch);
                    }
                    for (key, previous) in record.rows {
                        match previous {
                            Some(row) => deposits.insert(&key, &row)?,
                            None => deposits.remove(&key)?,
                        }
                    }
                    undo.remove(&expected.height)?;
                    stored.revision = stored.revision.checked_add(1).ok_or_else(|| {
                        TypedSledError::abort(BitcoinObserverConsistencyError::RevisionOverflow)
                    })?;
                    stored.last_processed_block = record.parent;
                    observer_state.insert(&OBSERVER_STATE_KEY, &stored)?;
                    Ok(())
                },
            ),
        )
    }
}

fn abort_tx<T>(
    error: BitcoinObserverConsistencyError,
) -> ConflictableTransactionResult<T, TypedSledError> {
    Err(TypedSledError::abort(error).into())
}

fn map_tx_result<T>(result: TransactionResult<T, TypedSledError>) -> DbResult<T> {
    match result {
        Ok(value) => Ok(value),
        Err(TransactionError::Storage(error)) => Err(error.into()),
        Err(TransactionError::Abort(error)) => {
            match error.downcast_abort::<BitcoinObserverConsistencyError>() {
                Ok(error) => Err(error.into()),
                Err(error) => Err(error.into()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Barrier},
        thread,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;
    use crate::db::{
        bitcoin_observer::{
            mock::MockBitcoinObserverDb,
            test_utils::{
                assert_block_writes_and_rollback_are_atomic,
                assert_changed_scan_configuration_is_rejected,
                assert_conflicting_block_replay_is_rejected,
                assert_duplicate_rows_abort_the_entire_block,
                assert_paging_uses_exclusive_encoded_outpoint_order,
                assert_replay_is_idempotent_and_conflicting_blocks_are_rejected,
                assert_revision_tracks_writes_and_rollback, make_block, make_metadata,
                make_observation,
            },
        },
        error::DbError,
    };

    #[test]
    fn observer_db_pages_in_encoded_outpoint_order_sled() {
        assert_paging_uses_exclusive_encoded_outpoint_order(
            &BitcoinObserverDbSled::open_temporary().expect("open DB"),
        );
    }
    #[test]
    fn observer_db_pages_in_encoded_outpoint_order_mock() {
        assert_paging_uses_exclusive_encoded_outpoint_order(&MockBitcoinObserverDb::default());
    }
    #[test]
    fn observer_db_advances_revision_only_on_writes_and_rollback_sled() {
        assert_revision_tracks_writes_and_rollback(
            &BitcoinObserverDbSled::open_temporary().expect("open DB"),
        );
    }
    #[test]
    fn observer_db_advances_revision_only_on_writes_and_rollback_mock() {
        assert_revision_tracks_writes_and_rollback(&MockBitcoinObserverDb::default());
    }

    #[test]
    fn observer_db_restores_previous_rows_and_state_sled() {
        assert_block_writes_and_rollback_are_atomic(
            &BitcoinObserverDbSled::open_temporary().expect("open DB"),
        );
    }
    #[test]
    fn observer_db_restores_previous_rows_and_state_mock() {
        assert_block_writes_and_rollback_are_atomic(&MockBitcoinObserverDb::default());
    }
    #[test]
    fn observer_db_rejects_duplicate_rows_without_partial_writes_sled() {
        assert_duplicate_rows_abort_the_entire_block(
            &BitcoinObserverDbSled::open_temporary().expect("open DB"),
        );
    }
    #[test]
    fn observer_db_rejects_duplicate_rows_without_partial_writes_mock() {
        assert_duplicate_rows_abort_the_entire_block(&MockBitcoinObserverDb::default());
    }
    #[test]
    fn observer_db_accepts_identical_replay_and_rejects_conflicts_sled() {
        assert_replay_is_idempotent_and_conflicting_blocks_are_rejected(
            &BitcoinObserverDbSled::open_temporary().expect("open DB"),
        );
    }
    #[test]
    fn observer_db_accepts_identical_replay_and_rejects_conflicts_mock() {
        assert_replay_is_idempotent_and_conflicting_blocks_are_rejected(
            &MockBitcoinObserverDb::default(),
        );
    }
    #[test]
    fn observer_db_rejects_conflicting_block_replay_sled() {
        assert_conflicting_block_replay_is_rejected(
            &BitcoinObserverDbSled::open_temporary().expect("open DB"),
        );
    }
    #[test]
    fn observer_db_rejects_conflicting_block_replay_mock() {
        assert_conflicting_block_replay_is_rejected(&MockBitcoinObserverDb::default());
    }
    #[test]
    fn observer_db_rejects_changed_scan_configuration_sled() {
        assert_changed_scan_configuration_is_rejected(
            &BitcoinObserverDbSled::open_temporary().expect("open DB"),
        );
    }
    #[test]
    fn observer_db_rejects_changed_scan_configuration_mock() {
        assert_changed_scan_configuration_is_rejected(&MockBitcoinObserverDb::default());
    }
    #[test]
    fn observer_db_retains_only_the_configured_rollback_window() {
        let db = BitcoinObserverDbSled::open_temporary().expect("open DB");
        let anchor = make_block(0, 0);
        db.initialize(&make_metadata(), Some(anchor))
            .expect("initialize");
        let mut parent = anchor;
        for height in 1..=4 {
            let block = make_block(height, 0);
            db.apply_block(Some(parent), block, &[], 2).expect("block");
            parent = block;
        }
        assert_eq!(
            db.rollback_history(10).expect("bounded undo history"),
            vec![
                Some(make_block(4, 0)),
                Some(make_block(3, 0)),
                Some(make_block(2, 0))
            ]
        );
        db.rollback_tip(make_block(4, 0)).expect("undo block 4");
        db.rollback_tip(make_block(3, 0)).expect("undo block 3");
        assert!(matches!(
            db.rollback_tip(make_block(2, 0)),
            Err(DbError::BitcoinObserverConsistency(
                BitcoinObserverConsistencyError::MissingUndo
            ))
        ));
    }
    #[test]
    fn observer_db_restores_state_rows_and_undo_after_reopen() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("bitcoin_observer_{}_{unique}", std::process::id()));
        let anchor = make_block(0, 0);
        let first = make_block(1, 0);
        let request = make_observation(first);
        {
            let db = BitcoinObserverDbSled::open(&path).expect("open DB");
            db.initialize(&make_metadata(), Some(anchor))
                .expect("initialize");
            db.apply_block(Some(anchor), first, std::slice::from_ref(&request), 4)
                .expect("persist block");
        }
        {
            let db = BitcoinObserverDbSled::open(&path).expect("reopen DB");
            assert_eq!(
                db.get_deposit(request.outpoint())
                    .expect("persisted request"),
                Some(request.clone())
            );
            assert_eq!(
                db.get_observer_state()
                    .expect("state")
                    .expect("initialized")
                    .last_processed_block,
                Some(first)
            );
            assert_eq!(
                db.get_observer_state()
                    .expect("state")
                    .expect("initialized")
                    .revision,
                1
            );
            db.apply_block(Some(anchor), first, std::slice::from_ref(&request), 4)
                .expect("replay after restart");
            assert_eq!(
                db.get_observer_state()
                    .expect("state")
                    .expect("initialized")
                    .revision,
                1
            );
            db.rollback_tip(first).expect("persisted undo record");
            assert_eq!(
                db.get_observer_state()
                    .expect("state")
                    .expect("initialized")
                    .revision,
                2
            );
            assert!(db
                .get_deposit(request.outpoint())
                .expect("removed request")
                .is_none());
        }
        {
            // A second restart preserves the rollback revision, not just its earlier tip.
            let db = BitcoinObserverDbSled::open(&path).expect("reopen after rollback");
            let state = db
                .get_observer_state()
                .expect("state")
                .expect("initialized");
            assert_eq!(state.last_processed_block, Some(anchor));
            assert_eq!(state.revision, 2);
        }
        std::fs::remove_dir_all(path).expect("remove temporary observer DB");
    }
    #[test]
    fn observer_db_rejects_a_competing_writer_with_a_stale_parent() {
        let db = Arc::new(BitcoinObserverDbSled::open_temporary().expect("open DB"));
        let anchor = make_block(0, 0);
        db.initialize(&make_metadata(), Some(anchor))
            .expect("initialize");
        let barrier = Arc::new(Barrier::new(2));
        let writers = [0, 1].map(|branch| {
            let db = Arc::clone(&db);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                let block = make_block(1, branch);
                let row = make_observation(block);
                barrier.wait();
                db.apply_block(Some(anchor), block, &[row], 4).is_ok()
            })
        });
        let successes = writers
            .into_iter()
            .map(|writer| writer.join().expect("writer thread"))
            .filter(|ok| *ok)
            .count();
        assert_eq!(successes, 1);
        let state = db
            .get_observer_state()
            .expect("state")
            .expect("initialized");
        let key = make_observation(make_block(1, 0)).outpoint();
        assert_eq!(
            db.get_deposit(key)
                .expect("winner's request")
                .expect("row")
                .inclusion
                .block,
            state.last_processed_block.expect("winner's block")
        );
    }
}
