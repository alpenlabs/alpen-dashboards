use std::path::Path;

use sled::transaction::TransactionError;
use typed_sled::{error::Error as TSledError, transaction::SledTransactional, SledDb, SledTree};

use crate::db::{
    error::{DbError, DbResult, WithdrawalIndexConsistencyError},
    traits::WithdrawalIndexerDb,
    types::{DbIndexerState, DbWithdrawalEventIndex, DbWithdrawalEventKey, DbWithdrawalRequest},
};

use super::schema::{
    IndexerStateSchema, WithdrawalAssignmentSchema, WithdrawalEventIndexSchema,
    WithdrawalRequestSchema, WithdrawalSeqByDepositIdxSchema,
};

/// Aborts a sled transaction with an application-level consistency error.
///
/// The transaction abort payload must be `Send + Sync`; [`DbError`] is not,
/// because it can wrap typed-sled codec errors. Use the shared consistency
/// error type as the transactional abort payload and wrap it in [`DbError`]
/// after the transaction.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "used by withdrawal indexer and pairing DB writes in follow-up commits"
    )
)]
fn abort_tx<T>(
    err: WithdrawalIndexConsistencyError,
) -> sled::transaction::ConflictableTransactionResult<T, TSledError> {
    Err(TSledError::abort(err).into())
}

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "used by withdrawal indexer and pairing DB writes in follow-up commits"
    )
)]
fn map_tx_result<T>(result: sled::transaction::TransactionResult<T, TSledError>) -> DbResult<T> {
    match result {
        Ok(value) => Ok(value),
        Err(TransactionError::Abort(err)) => {
            match err.downcast_abort::<WithdrawalIndexConsistencyError>() {
                Ok(db_err) => Err(db_err.into()),
                Err(err) => Err(err.into()),
            }
        }
        Err(TransactionError::Storage(err)) => Err(err.into()),
    }
}

/// Sled-backed withdrawal-indexer database. Owns one [`SledDb`] handle and one
/// typed tree per concern (indexer state, FIFO request sequence, event index,
/// pairing).
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "wired in by the indexer task in a follow-up commit"
    )
)]
#[derive(Debug)]
pub(crate) struct WithdrawalIndexerDbSled {
    _db: SledDb,
    state: SledTree<IndexerStateSchema>,
    requests: SledTree<WithdrawalRequestSchema>,
    event_index: SledTree<WithdrawalEventIndexSchema>,
    assignments: SledTree<WithdrawalAssignmentSchema>,
    seq_by_deposit_idx: SledTree<WithdrawalSeqByDepositIdxSchema>,
}

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "wired in by the indexer task in a follow-up commit"
    )
)]
impl WithdrawalIndexerDbSled {
    /// Open the indexer database under `{datadir}/withdrawal_index`. Creates
    /// intermediate directories if they don't exist; sled itself only creates
    /// the leaf, so a fresh datadir without intermediate parents would fail.
    pub(crate) fn open(datadir: impl AsRef<Path>) -> DbResult<Self> {
        let path = datadir.as_ref().join("withdrawal_index");
        std::fs::create_dir_all(&path)
            .map_err(|source| DbError::CreateDataDir(path.clone(), source))?;
        let sled_db = sled::open(path)?;
        Self::from_sled_db(sled_db)
    }

    /// Open a temporary in-memory-like sled database deleted on drop.
    #[cfg(test)]
    pub fn open_temporary() -> DbResult<Self> {
        let sled_db = sled::Config::new().temporary(true).open()?;
        Self::from_sled_db(sled_db)
    }

    fn from_sled_db(sled_db: sled::Db) -> DbResult<Self> {
        let db = SledDb::new(sled_db)?;

        Ok(Self {
            state: db.get_tree::<IndexerStateSchema>()?,
            requests: db.get_tree::<WithdrawalRequestSchema>()?,
            event_index: db.get_tree::<WithdrawalEventIndexSchema>()?,
            assignments: db.get_tree::<WithdrawalAssignmentSchema>()?,
            seq_by_deposit_idx: db.get_tree::<WithdrawalSeqByDepositIdxSchema>()?,
            _db: db,
        })
    }
}

impl WithdrawalIndexerDb for WithdrawalIndexerDbSled {
    fn get_indexer_state(&self, task: &str) -> DbResult<Option<DbIndexerState>> {
        Ok(self.state.get(&task.to_owned())?)
    }

    fn put_indexer_state(&self, task: &str, state: &DbIndexerState) -> DbResult<()> {
        self.state.insert(&task.to_owned(), state)?;
        Ok(())
    }

    fn insert_withdrawal_event(
        &self,
        requests: &[DbWithdrawalRequest],
    ) -> DbResult<DbWithdrawalEventIndex> {
        let first_request = requests.first().ok_or(DbError::EmptyWithdrawalEvent)?;
        let expected_event_key = DbWithdrawalEventKey::from(first_request);
        for request in requests {
            let request_key = DbWithdrawalEventKey::from(request);
            if request_key != expected_event_key {
                return Err(DbError::WithdrawalEventKeyMismatch {
                    expected: expected_event_key,
                    got: request_key,
                });
            }
        }

        let count = u32::try_from(requests.len())
            .map_err(|_| DbError::TooManyWithdrawalEventRequests(requests.len()))?;
        let first_seq = self.max_withdrawal_seq()?.map_or(Ok(0), |max| {
            max.checked_add(1)
                .ok_or(WithdrawalIndexConsistencyError::SeqOverflow(max))
                .map_err(DbError::from)
        })?;
        let index = DbWithdrawalEventIndex { first_seq, count };

        // The indexer is single-writer. If another writer races anyway, the
        // transaction below catches the stale sequence with `SeqOccupied`.
        map_tx_result((&self.requests, &self.event_index).transaction(
            |(requests_tree, event_index_tree)| {
                if let Some(existing_index) = event_index_tree.get(&expected_event_key)? {
                    for (offset, request) in requests.iter().enumerate() {
                        let Some(seq) = existing_index.first_seq.checked_add(offset as u64) else {
                            return abort_tx(WithdrawalIndexConsistencyError::SeqOverflow(
                                existing_index.first_seq,
                            ));
                        };
                        match requests_tree.get(&seq)? {
                            Some(existing_request) if existing_request == *request => {}
                            _ => {
                                return abort_tx(
                                    WithdrawalIndexConsistencyError::EventIndexInconsistent(
                                        expected_event_key,
                                        existing_index.first_seq,
                                        existing_index.count,
                                    ),
                                );
                            }
                        }
                    }

                    if existing_index.count == count {
                        return Ok(existing_index);
                    }

                    return abort_tx(WithdrawalIndexConsistencyError::EventIndexInconsistent(
                        expected_event_key,
                        existing_index.first_seq,
                        existing_index.count,
                    ));
                }

                for (offset, request) in requests.iter().enumerate() {
                    let Some(seq) = first_seq.checked_add(offset as u64) else {
                        return abort_tx(WithdrawalIndexConsistencyError::SeqOverflow(first_seq));
                    };
                    if requests_tree.get(&seq)?.is_some() {
                        return abort_tx(WithdrawalIndexConsistencyError::SeqOccupied(seq));
                    }
                    requests_tree.insert(&seq, request)?;
                }
                event_index_tree.insert(&expected_event_key, &index)?;
                Ok(index)
            },
        ))
    }

    fn get_withdrawal_request(&self, seq: u64) -> DbResult<Option<DbWithdrawalRequest>> {
        Ok(self.requests.get(&seq)?)
    }

    fn get_withdrawal_event_index(
        &self,
        key: &DbWithdrawalEventKey,
    ) -> DbResult<Option<DbWithdrawalEventIndex>> {
        Ok(self.event_index.get(key)?)
    }

    fn max_withdrawal_seq(&self) -> DbResult<Option<u64>> {
        Ok(self.requests.last()?.map(|(seq, _)| seq))
    }

    fn insert_pairing(&self, seq: u64, deposit_idx: u32) -> DbResult<()> {
        map_tx_result(
            (&self.requests, &self.assignments, &self.seq_by_deposit_idx).transaction(
                |(requests_tree, assignments_tree, seq_by_deposit_idx_tree)| {
                    if requests_tree.get(&seq)?.is_none() {
                        return abort_tx(WithdrawalIndexConsistencyError::MissingSeq(seq));
                    }

                    if let Some(existing_deposit_idx) = assignments_tree.get(&seq)? {
                        if existing_deposit_idx != deposit_idx {
                            return abort_tx(WithdrawalIndexConsistencyError::SeqPairingConflict(
                                seq,
                                existing_deposit_idx,
                                deposit_idx,
                            ));
                        }
                    }

                    if let Some(existing_seq) = seq_by_deposit_idx_tree.get(&deposit_idx)? {
                        if existing_seq != seq {
                            return abort_tx(
                                WithdrawalIndexConsistencyError::DepositPairingConflict(
                                    deposit_idx,
                                    existing_seq,
                                    seq,
                                ),
                            );
                        }
                    }

                    assignments_tree.insert(&seq, &deposit_idx)?;
                    seq_by_deposit_idx_tree.insert(&deposit_idx, &seq)?;
                    Ok(())
                },
            ),
        )
    }

    fn get_deposit_idx(&self, seq: u64) -> DbResult<Option<u32>> {
        Ok(self.assignments.get(&seq)?)
    }

    fn get_seq_by_deposit_idx(&self, deposit_idx: u32) -> DbResult<Option<u64>> {
        Ok(self.seq_by_deposit_idx.get(&deposit_idx)?)
    }

    fn list_unpaired_seqs(&self) -> DbResult<Vec<u64>> {
        let mut unpaired = Vec::new();
        for entry in self.requests.iter() {
            let (seq, _) = entry?;
            if self.assignments.get(&seq)?.is_none() {
                unpaired.push(seq);
            }
        }
        Ok(unpaired)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;
    use crate::db::withdrawal_index::{
        mock::MockWithdrawalIndexerDb,
        test_utils::{
            assert_pairing_conflicts_are_rejected, assert_pairing_requires_existing_seq,
            assert_pairing_roundtrip, assert_withdrawal_event_replay_is_idempotent,
            assert_withdrawal_event_roundtrip, make_withdrawal_request,
        },
    };

    fn make_unique_db_path(test_name: &str) -> PathBuf {
        let now_nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("current time must be >= UNIX_EPOCH")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "withdrawal_index_db_{test_name}_{}_{}",
            std::process::id(),
            now_nanos
        ))
    }

    #[test]
    fn withdrawal_event_roundtrip_sled() {
        let db = WithdrawalIndexerDbSled::open_temporary().expect("open db");
        assert_withdrawal_event_roundtrip(&db);
    }

    #[test]
    fn withdrawal_event_roundtrip_mock() {
        assert_withdrawal_event_roundtrip(&MockWithdrawalIndexerDb::default());
    }

    #[test]
    fn withdrawal_event_replay_is_idempotent_sled() {
        let db = WithdrawalIndexerDbSled::open_temporary().expect("open db");
        assert_withdrawal_event_replay_is_idempotent(&db);
    }

    #[test]
    fn withdrawal_event_replay_is_idempotent_mock() {
        assert_withdrawal_event_replay_is_idempotent(&MockWithdrawalIndexerDb::default());
    }

    #[test]
    fn pairing_roundtrip_sled() {
        let db = WithdrawalIndexerDbSled::open_temporary().expect("open db");
        assert_pairing_roundtrip(&db);
    }

    #[test]
    fn pairing_roundtrip_mock() {
        assert_pairing_roundtrip(&MockWithdrawalIndexerDb::default());
    }

    #[test]
    fn pairing_conflicts_are_rejected_sled() {
        let db = WithdrawalIndexerDbSled::open_temporary().expect("open db");
        assert_pairing_conflicts_are_rejected(&db);
    }

    #[test]
    fn pairing_conflicts_are_rejected_mock() {
        assert_pairing_conflicts_are_rejected(&MockWithdrawalIndexerDb::default());
    }

    #[test]
    fn pairing_requires_existing_seq_sled() {
        let db = WithdrawalIndexerDbSled::open_temporary().expect("open db");
        assert_pairing_requires_existing_seq(&db);
    }

    #[test]
    fn pairing_requires_existing_seq_mock() {
        assert_pairing_requires_existing_seq(&MockWithdrawalIndexerDb::default());
    }

    #[test]
    fn rows_persist_across_reopen() {
        let path = make_unique_db_path("reopen");
        let req = make_withdrawal_request(1);
        let state = DbIndexerState {
            last_scanned_block: 12345,
        };

        {
            let db = WithdrawalIndexerDbSled::open(&path).expect("open db");
            db.insert_withdrawal_event(std::slice::from_ref(&req))
                .expect("insert event");
            db.insert_pairing(0, 42).expect("insert pairing");
            db.put_indexer_state("withdrawal_index", &state)
                .expect("put state");
        }

        {
            let reopened = WithdrawalIndexerDbSled::open(&path).expect("reopen db");
            assert_eq!(
                reopened.get_withdrawal_request(0).expect("get request"),
                Some(req)
            );
            assert_eq!(
                reopened.get_deposit_idx(0).expect("get deposit_idx"),
                Some(42)
            );
            assert_eq!(
                reopened
                    .get_seq_by_deposit_idx(42)
                    .expect("get seq by deposit_idx"),
                Some(0)
            );
            assert_eq!(
                reopened
                    .get_indexer_state("withdrawal_index")
                    .expect("get state"),
                Some(state)
            );
        }

        let _ = fs::remove_dir_all(path);
    }
}
