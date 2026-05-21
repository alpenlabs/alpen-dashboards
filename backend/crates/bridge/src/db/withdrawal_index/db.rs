use std::path::Path;

use anyhow::Context;
use sled::transaction::TransactionError;
use typed_sled::{error::Error as TSledError, transaction::SledTransactional, SledDb, SledTree};

use crate::db::{
    error::{DbError, DbResult, WithdrawalIndexConsistencyError},
    traits::WithdrawalIndexerDb,
    types::{
        DbIndexerState, DbWithdrawalEventIndex, DbWithdrawalEventKey, DbWithdrawalRequest,
        DbWithdrawalRequestRow,
    },
};

use super::schema::{IndexerStateSchema, WithdrawalEventIndexSchema, WithdrawalRequestSchema};

/// Aborts a sled transaction with an application-level consistency error.
///
/// The transaction abort payload must be `Send + Sync`; [`DbError`] is not,
/// because it can wrap typed-sled codec errors. Use the shared consistency
/// error type as the transactional abort payload and wrap it in [`DbError`]
/// after the transaction.
fn abort_tx<T>(
    err: WithdrawalIndexConsistencyError,
) -> sled::transaction::ConflictableTransactionResult<T, TSledError> {
    Err(TSledError::abort(err).into())
}

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
/// typed tree per concern (indexer state, FIFO request sequence, event index).
#[derive(Debug)]
pub struct WithdrawalIndexerDbSled {
    _db: SledDb,
    state: SledTree<IndexerStateSchema>,
    requests: SledTree<WithdrawalRequestSchema>,
    event_index: SledTree<WithdrawalEventIndexSchema>,
}

impl WithdrawalIndexerDbSled {
    /// Open the indexer database under `{datadir}/withdrawal_index`. Creates
    /// intermediate directories if they don't exist; sled itself only creates
    /// the leaf, so a fresh datadir without intermediate parents would fail.
    pub fn open(datadir: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = datadir.as_ref().join("withdrawal_index");
        std::fs::create_dir_all(&path)
            .with_context(|| format!("create withdrawal index dir {}", path.display()))?;
        let sled_db = sled::open(&path)
            .with_context(|| format!("open withdrawal index sled db at {}", path.display()))?;
        // typed-sled codec errors are not Send + Sync, so anyhow::Context cannot preserve them.
        Self::from_sled_db(sled_db)
            .map_err(|e| anyhow::anyhow!("initialize withdrawal index trees: {e}"))
    }

    /// Open a temporary in-memory-like sled database deleted on drop.
    #[cfg(test)]
    pub fn open_temporary() -> anyhow::Result<Self> {
        let sled_db = sled::Config::new()
            .temporary(true)
            .open()
            .context("open temporary withdrawal index sled db")?;
        // typed-sled codec errors are not Send + Sync, so anyhow::Context cannot preserve them.
        Self::from_sled_db(sled_db)
            .map_err(|e| anyhow::anyhow!("initialize temporary withdrawal index trees: {e}"))
    }

    fn from_sled_db(sled_db: sled::Db) -> DbResult<Self> {
        let db = SledDb::new(sled_db)?;

        Ok(Self {
            state: db.get_tree::<IndexerStateSchema>()?,
            requests: db.get_tree::<WithdrawalRequestSchema>()?,
            event_index: db.get_tree::<WithdrawalEventIndexSchema>()?,
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

    fn fetch_withdrawal_requests_from(
        &self,
        start_seq: u64,
        limit: usize,
    ) -> DbResult<Vec<DbWithdrawalRequestRow>> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let mut rows = Vec::new();
        for entry in self.requests.range(start_seq..)?.take(limit) {
            let (seq, request) = entry?;
            rows.push(DbWithdrawalRequestRow { seq, request });
        }
        Ok(rows)
    }

    fn max_withdrawal_seq(&self) -> DbResult<Option<u64>> {
        Ok(self.requests.last()?.map(|(seq, _)| seq))
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
            assert_withdrawal_event_replay_is_idempotent, assert_withdrawal_event_roundtrip,
            make_withdrawal_request,
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
            db.put_indexer_state("withdrawal_index", &state)
                .expect("put state");
        }

        {
            let reopened = WithdrawalIndexerDbSled::open(&path).expect("reopen db");
            assert_eq!(
                reopened
                    .fetch_withdrawal_requests_from(0, 1)
                    .expect("fetch request"),
                vec![crate::db::types::DbWithdrawalRequestRow {
                    seq: 0,
                    request: req.clone()
                }]
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
