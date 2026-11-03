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
    use super::*;
    use crate::db::withdrawal_index::{
        mock::MockWithdrawalIndexerDb, test_utils::make_withdrawal_event,
    };

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
}
