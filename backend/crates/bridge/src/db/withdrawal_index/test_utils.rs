//! Test fixtures and trait-generic assertion helpers for the withdrawal-indexer DB.

use strata_primitives::buf::Buf32;

use crate::db::traits::WithdrawalIndexerDb;

use crate::db::types::{
    DbIndexerState, DbWithdrawalEventIndex, DbWithdrawalRequest, DbWithdrawalRequestRow,
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

pub(crate) fn assert_withdrawal_event_roundtrip<D: WithdrawalIndexerDb>(db: &D) {
    let req_a = make_withdrawal_request(1);
    let req_b = DbWithdrawalRequest {
        sub_idx: 1,
        ..req_a.clone()
    };

    assert_eq!(
        db.insert_withdrawal_event(&[req_a.clone(), req_b.clone()])
            .expect("insert event"),
        DbWithdrawalEventIndex {
            first_seq: 0,
            count: 2
        }
    );

    assert_eq!(
        db.fetch_withdrawal_requests_from(0, 2)
            .expect("fetch request rows"),
        vec![
            DbWithdrawalRequestRow {
                seq: 0,
                request: req_a.clone()
            },
            DbWithdrawalRequestRow {
                seq: 1,
                request: req_b.clone()
            }
        ]
    );
    assert!(db
        .fetch_withdrawal_requests_from(99, 1)
        .expect("fetch missing rows")
        .is_empty());

    assert_eq!(db.max_withdrawal_seq().expect("max"), Some(1));

    let state = DbIndexerState {
        last_scanned_block: 4242,
    };
    db.put_indexer_state("withdrawal_index", &state)
        .expect("put state");
    assert_eq!(
        db.get_indexer_state("withdrawal_index").expect("get state"),
        Some(state)
    );
    assert_eq!(db.get_indexer_state("absent").expect("missing"), None);
}

pub(crate) fn assert_withdrawal_event_replay_is_idempotent<D: WithdrawalIndexerDb>(db: &D) {
    let req_a = make_withdrawal_request(4);
    let req_b = DbWithdrawalRequest {
        sub_idx: 1,
        ..req_a.clone()
    };
    let requests = [req_a.clone(), req_b.clone()];

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
        vec![
            DbWithdrawalRequestRow {
                seq: first_index.first_seq,
                request: req_a
            },
            DbWithdrawalRequestRow {
                seq: first_index.first_seq + 1,
                request: req_b
            }
        ]
    );
}
