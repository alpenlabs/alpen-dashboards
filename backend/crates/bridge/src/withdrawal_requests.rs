use tracing::warn;

use super::db::{traits::WithdrawalIndexerDb, types::DbWithdrawalRequestRow};

pub(crate) fn fetch_withdrawal_requests(
    withdrawal_index: &(impl WithdrawalIndexerDb + ?Sized),
    start_seq: u64,
    fetch_count: usize,
    batch_size: usize,
) -> Vec<DbWithdrawalRequestRow> {
    if fetch_count == 0 || batch_size == 0 {
        return Vec::new();
    }

    let mut requests = Vec::new();
    let mut next_seq = start_seq;

    while requests.len() < fetch_count {
        let limit = (fetch_count - requests.len()).min(batch_size);
        match withdrawal_index.fetch_withdrawal_requests_from(next_seq, limit) {
            Ok(batch) => {
                if batch.is_empty() {
                    break;
                }

                let Some(next) = batch.last().and_then(|row| row.seq.checked_add(1)) else {
                    requests.extend(batch);
                    break;
                };
                next_seq = next;
                requests.extend(batch);
            }
            Err(e) => {
                warn!(error = %e, "failed to fetch indexed withdrawal requests");
                break;
            }
        }
    }

    requests
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::db::{
        error::{DbError, DbResult, WithdrawalIndexConsistencyError},
        types::{DbIndexerState, DbWithdrawalEventIndex, DbWithdrawalRequest},
        withdrawal_index::{mock::MockWithdrawalIndexerDb, test_utils::make_withdrawal_event},
    };

    /// Mock index whose fetches fail after the first batch.
    #[derive(Default)]
    struct FailingAfterFirstFetch {
        inner: MockWithdrawalIndexerDb,
        fetches: AtomicUsize,
    }

    impl WithdrawalIndexerDb for FailingAfterFirstFetch {
        fn get_indexer_state(&self, task: &str) -> DbResult<Option<DbIndexerState>> {
            self.inner.get_indexer_state(task)
        }

        fn put_indexer_state(&self, task: &str, state: &DbIndexerState) -> DbResult<()> {
            self.inner.put_indexer_state(task, state)
        }

        fn insert_withdrawal_event(
            &self,
            requests: &[DbWithdrawalRequest],
        ) -> DbResult<DbWithdrawalEventIndex> {
            self.inner.insert_withdrawal_event(requests)
        }

        fn fetch_withdrawal_requests_from(
            &self,
            start_seq: u64,
            limit: usize,
        ) -> DbResult<Vec<DbWithdrawalRequestRow>> {
            if self.fetches.fetch_add(1, Ordering::Relaxed) == 0 {
                self.inner.fetch_withdrawal_requests_from(start_seq, limit)
            } else {
                Err(DbError::from(WithdrawalIndexConsistencyError::SeqOverflow(
                    start_seq,
                )))
            }
        }

        fn max_withdrawal_seq(&self) -> DbResult<Option<u64>> {
            self.inner.max_withdrawal_seq()
        }
    }

    #[test]
    fn fetches_withdrawal_requests_in_batches() {
        let db = MockWithdrawalIndexerDb::default();
        db.insert_withdrawal_event(&make_withdrawal_event(1, 3))
            .expect("insert requests");

        let rows = fetch_withdrawal_requests(&db, 0, 3, 2);

        assert_eq!(
            rows.into_iter().map(|row| row.seq).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
    }

    #[test]
    fn fetch_keeps_earlier_batches_when_a_later_batch_fails() {
        let db = FailingAfterFirstFetch::default();
        db.insert_withdrawal_event(&make_withdrawal_event(1, 3))
            .expect("insert requests");

        let rows = fetch_withdrawal_requests(&db, 0, 3, 2);

        assert_eq!(
            rows.into_iter().map(|row| row.seq).collect::<Vec<_>>(),
            vec![0, 1]
        );
    }

    #[test]
    fn fetch_returns_nothing_for_zero_count_or_batch_size() {
        let db = MockWithdrawalIndexerDb::default();
        db.insert_withdrawal_event(&make_withdrawal_event(1, 3))
            .expect("insert requests");

        assert!(fetch_withdrawal_requests(&db, 0, 0, 2).is_empty());
        assert!(fetch_withdrawal_requests(&db, 0, 3, 0).is_empty());
    }
}
