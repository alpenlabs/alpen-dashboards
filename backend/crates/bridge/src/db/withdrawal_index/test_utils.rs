//! Test fixtures and trait-generic assertion helpers for the withdrawal-indexer DB.

use strata_primitives::buf::Buf32;

use crate::db::{
    error::{DbError, WithdrawalIndexConsistencyError},
    traits::WithdrawalIndexerDb,
};

use crate::db::types::{
    DbIndexerState, DbWithdrawalEventIndex, DbWithdrawalEventKey, DbWithdrawalRequest,
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
        db.get_withdrawal_request(0).expect("get seq 0"),
        Some(req_a.clone())
    );
    assert_eq!(
        db.get_withdrawal_request(1).expect("get seq 1"),
        Some(req_b.clone())
    );
    assert_eq!(db.get_withdrawal_request(99).expect("missing"), None);

    let key = DbWithdrawalEventKey {
        tx_hash: req_a.tx_hash,
        log_index: req_a.log_index,
    };
    assert_eq!(
        db.get_withdrawal_event_index(&key).expect("lookup event"),
        Some(DbWithdrawalEventIndex {
            first_seq: 0,
            count: 2
        })
    );

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
        db.get_withdrawal_request(first_index.first_seq)
            .expect("first request"),
        Some(req_a)
    );
    assert_eq!(
        db.get_withdrawal_request(first_index.first_seq + 1)
            .expect("second request"),
        Some(req_b)
    );
}

pub(crate) fn assert_pairing_roundtrip<D: WithdrawalIndexerDb>(db: &D) {
    let req = make_withdrawal_request(3);
    db.insert_withdrawal_event(&[req]).expect("insert");

    assert_eq!(
        db.list_unpaired_seqs().expect("list unpaired"),
        vec![0],
        "newly inserted requests start unpaired"
    );

    db.insert_pairing(0, 7).expect("pair");
    assert_eq!(db.get_deposit_idx(0).expect("get deposit_idx"), Some(7));
    assert_eq!(
        db.get_seq_by_deposit_idx(7).expect("reverse lookup"),
        Some(0)
    );
    assert!(
        db.list_unpaired_seqs().expect("list unpaired").is_empty(),
        "paired requests no longer appear in the unpaired set"
    );
}

pub(crate) fn assert_pairing_conflicts_are_rejected<D: WithdrawalIndexerDb>(db: &D) {
    let req_a = make_withdrawal_request(5);
    let req_b = make_withdrawal_request(6);

    db.insert_withdrawal_event(&[req_a]).expect("insert first");
    db.insert_withdrawal_event(&[req_b]).expect("insert second");
    db.insert_pairing(0, 7).expect("pair first");

    assert!(matches!(
        db.insert_pairing(0, 8),
        Err(DbError::Consistency(
            WithdrawalIndexConsistencyError::SeqPairingConflict(0, 7, 8)
        ))
    ));
    assert!(matches!(
        db.insert_pairing(1, 7),
        Err(DbError::Consistency(
            WithdrawalIndexConsistencyError::DepositPairingConflict(7, 0, 1)
        ))
    ));
}

pub(crate) fn assert_pairing_requires_existing_seq<D: WithdrawalIndexerDb>(db: &D) {
    assert!(matches!(
        db.insert_pairing(99, 7),
        Err(DbError::Consistency(
            WithdrawalIndexConsistencyError::MissingSeq(99)
        ))
    ));

    assert_eq!(
        db.get_deposit_idx(99).expect("forward lookup"),
        None,
        "missing seq must not create an orphan forward pairing"
    );
    assert_eq!(
        db.get_seq_by_deposit_idx(7).expect("reverse lookup"),
        None,
        "missing seq must not create an orphan reverse pairing"
    );
}
