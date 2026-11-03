//! Test fixtures and trait-generic assertion helpers for the withdrawal-indexer DB.

use strata_primitives::buf::Buf32;

use crate::{
    db::{
        traits::WithdrawalIndexerDb,
        types::{
            DbIndexerState, DbWithdrawalEventIndex, DbWithdrawalRequest, DbWithdrawalRequestRow,
        },
    },
    withdrawal_indexer::TASK_NAME,
};

pub(crate) fn make_withdrawal_request(seed: u8) -> DbWithdrawalRequest {
    DbWithdrawalRequest {
        tx_hash: Buf32([seed; 32]),
        log_index: u64::from(seed),
        sub_idx: u32::from(seed),
        amount_sats: 100_000 * u64::from(seed),
        destination: vec![seed; 22],
        selected_operator: u32::from(seed) % 4,
        block_number: 1_000 + u64::from(seed),
    }
}

/// Requests of one withdrawal-intent event, one per sub-unit.
pub(crate) fn make_withdrawal_event(seed: u8, count: u32) -> Vec<DbWithdrawalRequest> {
    (0..count)
        .map(|sub_idx| DbWithdrawalRequest {
            sub_idx,
            ..make_withdrawal_request(seed)
        })
        .collect()
}

/// Rows for `requests` stored from `first_seq` onwards.
pub(crate) fn make_request_rows(
    first_seq: u64,
    requests: &[DbWithdrawalRequest],
) -> Vec<DbWithdrawalRequestRow> {
    (first_seq..)
        .zip(requests.iter().cloned())
        .map(|(seq, request)| DbWithdrawalRequestRow::new(seq, request))
        .collect()
}

pub(crate) fn assert_returns_inserted_events_and_state(db: &impl WithdrawalIndexerDb) {
    let requests = make_withdrawal_event(1, 2);

    assert_eq!(
        db.insert_withdrawal_event(&requests).expect("insert event"),
        DbWithdrawalEventIndex::new(0, 2)
    );

    assert_eq!(
        db.fetch_withdrawal_requests_from(0, 2)
            .expect("fetch request rows"),
        make_request_rows(0, &requests)
    );
    assert!(db
        .fetch_withdrawal_requests_from(99, 1)
        .expect("fetch missing rows")
        .is_empty());

    assert_eq!(db.max_withdrawal_seq().expect("max"), Some(1));

    let state = DbIndexerState::new(4242);
    db.put_indexer_state(TASK_NAME, &state).expect("put state");
    assert_eq!(
        db.get_indexer_state(TASK_NAME).expect("get state"),
        Some(state)
    );
    assert_eq!(db.get_indexer_state("absent").expect("missing"), None);
}

pub(crate) fn assert_withdrawal_event_replay_is_idempotent(db: &impl WithdrawalIndexerDb) {
    let requests = make_withdrawal_event(4, 2);

    let first_index = db
        .insert_withdrawal_event(&requests)
        .expect("insert event first time");
    let second_index = db
        .insert_withdrawal_event(&requests)
        .expect("replay same event");

    assert_eq!(second_index, first_index);
    assert_eq!(db.max_withdrawal_seq().expect("max"), Some(1));
    assert_eq!(
        db.fetch_withdrawal_requests_from(first_index.first_seq, 2)
            .expect("fetch request rows"),
        make_request_rows(first_index.first_seq, &requests)
    );
}
