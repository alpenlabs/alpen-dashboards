use std::collections::BTreeMap;
use std::sync::RwLock;

use crate::db::{
    error::{DbError, DbResult, WithdrawalIndexConsistencyError},
    traits::WithdrawalIndexerDb,
};

use crate::db::types::{
    DbIndexerState, DbWithdrawalEventIndex, DbWithdrawalEventKey, DbWithdrawalRequest,
};

/// In-memory withdrawal-indexer database for tests.
#[derive(Debug, Default)]
pub(crate) struct MockWithdrawalIndexerDb {
    state: RwLock<BTreeMap<String, DbIndexerState>>,
    requests: RwLock<BTreeMap<u64, DbWithdrawalRequest>>,
    event_index: RwLock<BTreeMap<DbWithdrawalEventKey, DbWithdrawalEventIndex>>,
    assignments: RwLock<BTreeMap<u64, u32>>,
    seq_by_deposit_idx: RwLock<BTreeMap<u32, u64>>,
}

impl WithdrawalIndexerDb for MockWithdrawalIndexerDb {
    fn get_indexer_state(&self, task: &str) -> DbResult<Option<DbIndexerState>> {
        Ok(self
            .state
            .read()
            .expect("mock state lock poisoned")
            .get(task)
            .cloned())
    }

    fn put_indexer_state(&self, task: &str, state: &DbIndexerState) -> DbResult<()> {
        self.state
            .write()
            .expect("mock state lock poisoned")
            .insert(task.to_owned(), state.clone());
        Ok(())
    }

    fn insert_withdrawal_event(
        &self,
        event_requests: &[DbWithdrawalRequest],
    ) -> DbResult<DbWithdrawalEventIndex> {
        let first_request = event_requests
            .first()
            .ok_or(DbError::EmptyWithdrawalEvent)?;
        let expected_event_key = DbWithdrawalEventKey::from(first_request);
        for request in event_requests {
            let request_key = DbWithdrawalEventKey::from(request);
            if request_key != expected_event_key {
                return Err(DbError::WithdrawalEventKeyMismatch {
                    expected: expected_event_key,
                    got: request_key,
                });
            }
        }

        let count = u32::try_from(event_requests.len())
            .map_err(|_| DbError::TooManyWithdrawalEventRequests(event_requests.len()))?;
        let mut requests = self.requests.write().expect("mock requests lock poisoned");
        let mut event_index = self
            .event_index
            .write()
            .expect("mock event_index lock poisoned");

        if let Some(existing_index) = event_index.get(&expected_event_key).copied() {
            for (offset, request) in event_requests.iter().enumerate() {
                let seq = existing_index.first_seq.checked_add(offset as u64).ok_or(
                    WithdrawalIndexConsistencyError::SeqOverflow(existing_index.first_seq),
                )?;
                match requests.get(&seq) {
                    Some(existing_request) if existing_request == request => {}
                    _ => {
                        return Err(WithdrawalIndexConsistencyError::EventIndexInconsistent(
                            expected_event_key,
                            existing_index.first_seq,
                            existing_index.count,
                        )
                        .into());
                    }
                }
            }
            if existing_index.count == count {
                return Ok(existing_index);
            }
            return Err(WithdrawalIndexConsistencyError::EventIndexInconsistent(
                expected_event_key,
                existing_index.first_seq,
                existing_index.count,
            )
            .into());
        }

        let first_seq = requests.keys().next_back().map_or(Ok(0), |max| {
            max.checked_add(1)
                .ok_or(WithdrawalIndexConsistencyError::SeqOverflow(*max))
                .map_err(DbError::from)
        })?;
        let index = DbWithdrawalEventIndex { first_seq, count };

        for (offset, request) in event_requests.iter().enumerate() {
            let seq = first_seq
                .checked_add(offset as u64)
                .ok_or(WithdrawalIndexConsistencyError::SeqOverflow(first_seq))?;
            if requests.contains_key(&seq) {
                return Err(WithdrawalIndexConsistencyError::SeqOccupied(seq).into());
            }
            requests.insert(seq, request.clone());
        }
        event_index.insert(expected_event_key, index);
        Ok(index)
    }

    fn get_withdrawal_request(&self, seq: u64) -> DbResult<Option<DbWithdrawalRequest>> {
        Ok(self
            .requests
            .read()
            .expect("mock requests lock poisoned")
            .get(&seq)
            .cloned())
    }

    fn get_withdrawal_event_index(
        &self,
        key: &DbWithdrawalEventKey,
    ) -> DbResult<Option<DbWithdrawalEventIndex>> {
        Ok(self
            .event_index
            .read()
            .expect("mock event_index lock poisoned")
            .get(key)
            .copied())
    }

    fn max_withdrawal_seq(&self) -> DbResult<Option<u64>> {
        Ok(self
            .requests
            .read()
            .expect("mock requests lock poisoned")
            .keys()
            .next_back()
            .copied())
    }

    fn insert_pairing(&self, seq: u64, deposit_idx: u32) -> DbResult<()> {
        if !self
            .requests
            .read()
            .expect("mock requests lock poisoned")
            .contains_key(&seq)
        {
            return Err(WithdrawalIndexConsistencyError::MissingSeq(seq).into());
        }

        let mut assignments = self
            .assignments
            .write()
            .expect("mock assignments lock poisoned");
        let mut seq_by_deposit_idx = self
            .seq_by_deposit_idx
            .write()
            .expect("mock seq_by_deposit_idx lock poisoned");

        if let Some(existing_deposit_idx) = assignments.get(&seq).copied() {
            if existing_deposit_idx != deposit_idx {
                return Err(WithdrawalIndexConsistencyError::SeqPairingConflict(
                    seq,
                    existing_deposit_idx,
                    deposit_idx,
                )
                .into());
            }
        }
        if let Some(existing_seq) = seq_by_deposit_idx.get(&deposit_idx).copied() {
            if existing_seq != seq {
                return Err(WithdrawalIndexConsistencyError::DepositPairingConflict(
                    deposit_idx,
                    existing_seq,
                    seq,
                )
                .into());
            }
        }

        assignments.insert(seq, deposit_idx);
        seq_by_deposit_idx.insert(deposit_idx, seq);
        Ok(())
    }

    fn get_deposit_idx(&self, seq: u64) -> DbResult<Option<u32>> {
        Ok(self
            .assignments
            .read()
            .expect("mock assignments lock poisoned")
            .get(&seq)
            .copied())
    }

    fn get_seq_by_deposit_idx(&self, deposit_idx: u32) -> DbResult<Option<u64>> {
        Ok(self
            .seq_by_deposit_idx
            .read()
            .expect("mock seq_by_deposit_idx lock poisoned")
            .get(&deposit_idx)
            .copied())
    }

    fn list_unpaired_seqs(&self) -> DbResult<Vec<u64>> {
        let assignments = self
            .assignments
            .read()
            .expect("mock assignments lock poisoned");
        Ok(self
            .requests
            .read()
            .expect("mock requests lock poisoned")
            .keys()
            .copied()
            .filter(|seq| !assignments.contains_key(seq))
            .collect())
    }
}
