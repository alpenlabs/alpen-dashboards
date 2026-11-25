//! Storage contract checks shared by the Sled and mock observer implementations.

use bitcoin::{
    blockdata::constants::genesis_block,
    consensus::serialize,
    hashes::{sha256, Hash},
    Network,
};
use strata_btc_types::BlockHashExt;
use strata_identifiers::L1BlockCommitment;

use crate::{
    db::{
        error::{BitcoinObserverConsistencyError, DbError},
        traits::BitcoinObserverDb,
        types::BitcoinObserverDbMetadata,
    },
    l1::{
        deposits::DepositSpend,
        observations::{ObservedDeposit, ObservedDepositSpend, TransactionInclusion},
        test_utils::{DepositFixture, TEST_DEPOSIT_INDEX},
    },
};

pub(super) fn make_metadata() -> BitcoinObserverDbMetadata {
    BitcoinObserverDbMetadata {
        genesis_hash: genesis_block(Network::Regtest).block_hash(),
        params_hash: sha256::Hash::hash(b"observer test parameters"),
    }
}

pub(super) fn make_block(height: u32, branch: u8) -> L1BlockCommitment {
    let mut bytes = height.to_be_bytes().to_vec();
    bytes.push(branch);
    let hash = bitcoin::BlockHash::from_raw_hash(bitcoin::hashes::sha256d::Hash::hash(&bytes));
    L1BlockCommitment::new(height, hash.to_l1_block_id())
}

pub(super) fn make_observation(block: L1BlockCommitment) -> ObservedDeposit {
    ObservedDeposit {
        request: DepositFixture::new().request_tx().clone(),
        inclusion: TransactionInclusion { block, position: 0 },
        spend: None,
    }
}

pub(super) fn assert_paging_uses_exclusive_encoded_outpoint_order(db: &impl BitcoinObserverDb) {
    let anchor = make_block(0, 0);
    let block = make_block(1, 0);
    db.initialize(&make_metadata(), Some(anchor))
        .expect("initialize");
    assert!(db
        .get_deposits_from(None, 2)
        .expect("empty page")
        .is_empty());

    // Insert in transaction order; storage pages must instead follow encoded outpoints.
    let mut rows = (0..5)
        .map(|seed| ObservedDeposit {
            request: DepositFixture::with_request_seed(seed).request_tx().clone(),
            inclusion: TransactionInclusion {
                block,
                position: u32::from(seed),
            },
            spend: None,
        })
        .collect::<Vec<_>>();
    db.apply_block(Some(anchor), block, &rows, 4)
        .expect("request block");
    rows.sort_by_cached_key(|row| serialize(&row.outpoint()));

    for limit in [0, 1, 2, rows.len(), rows.len() + 1] {
        assert_eq!(
            db.get_deposits_from(None, limit).expect("first page"),
            rows[..limit.min(rows.len())]
        );
        for (index, row) in rows.iter().enumerate() {
            let remaining = &rows[index + 1..];
            assert_eq!(
                db.get_deposits_from(Some(row.outpoint()), limit)
                    .expect("exclusive page"),
                remaining[..limit.min(remaining.len())]
            );
        }
    }

    // A cursor need not identify a stored row: resume strictly after its encoded key.
    let mut missing = rows[2].outpoint();
    missing.vout += 1;
    assert!(db
        .get_deposit(missing)
        .expect("absent cursor row")
        .is_none());
    assert_eq!(
        db.get_deposits_from(Some(missing), rows.len())
            .expect("page after absent cursor"),
        rows[3..]
    );
}

pub(super) fn assert_revision_tracks_writes_and_rollback(db: &impl BitcoinObserverDb) {
    let metadata = make_metadata();
    let anchor = make_block(0, 0);
    db.initialize(&metadata, Some(anchor)).expect("initialize");
    let state = || {
        db.get_observer_state()
            .expect("state")
            .expect("initialized")
    };
    assert_eq!(state().revision, 0);

    let first = make_block(1, 0);
    let request = make_observation(first);
    db.apply_block(Some(anchor), first, std::slice::from_ref(&request), 4)
        .expect("request block");
    let applied = state();
    assert_eq!(applied.revision, 1);
    db.apply_block(Some(anchor), first, &[request], 4)
        .expect("identical replay");
    db.initialize(&metadata, Some(anchor))
        .expect("reinitialize");
    assert_eq!(state(), applied);

    // Empty blocks also change the snapshot used to calculate confirmations.
    let second = make_block(2, 0);
    db.apply_block(Some(first), second, &[], 4)
        .expect("empty block");
    assert_eq!(state().revision, 2);
    db.rollback_tip(second).expect("rollback empty block");
    assert_eq!(state().revision, 3);
    assert_eq!(state().last_processed_block, Some(first));

    // Returning to the same tip must not reuse the old revision: paged readers
    // need to detect a rollback/reapply even when their two tip reads agree.
    db.apply_block(Some(first), second, &[], 4)
        .expect("reapply block");
    assert_eq!(state().revision, 4);
    assert_eq!(state().last_processed_block, Some(second));
    let before_rejection = state();
    assert!(matches!(
        db.apply_block(Some(first), make_block(2, 1), &[], 4),
        Err(DbError::BitcoinObserverConsistency(
            BitcoinObserverConsistencyError::ObserverStateMismatch
        ))
    ));
    assert_eq!(state(), before_rejection);
}

pub(super) fn assert_block_writes_and_rollback_are_atomic(db: &impl BitcoinObserverDb) {
    let metadata = make_metadata();
    let anchor = make_block(0, 0);
    db.initialize(&metadata, Some(anchor))
        .expect("initialize observer state");
    let first = make_block(1, 0);
    let request = make_observation(first);
    db.apply_block(Some(anchor), first, std::slice::from_ref(&request), 4)
        .expect("request block");

    // Replacing a request with its observed spend must be undone as one block transition.
    let second = make_block(2, 0);
    let mut completed = request.clone();
    completed.spend = Some(ObservedDepositSpend {
        inclusion: TransactionInclusion {
            block: second,
            position: 0,
        },
        spend: DepositSpend::Deposit {
            txid: DepositFixture::new().deposit_tx().compute_txid(),
            deposit_idx: TEST_DEPOSIT_INDEX,
        },
    });
    db.apply_block(Some(first), second, &[completed], 4)
        .expect("deposit block");
    assert!(db
        .get_deposit(request.outpoint())
        .expect("completed request")
        .expect("row")
        .spend
        .is_some());
    db.rollback_tip(second).expect("undo spend");
    assert_eq!(
        db.get_deposit(request.outpoint())
            .expect("restored request"),
        Some(request.clone())
    );
    assert_eq!(
        db.get_observer_state()
            .expect("state")
            .expect("initialized")
            .last_processed_block,
        Some(first)
    );

    db.rollback_tip(first).expect("undo request");
    assert!(db
        .get_deposit(request.outpoint())
        .expect("orphaned request")
        .is_none());
    assert_eq!(
        db.get_observer_state()
            .expect("state")
            .expect("initialized")
            .last_processed_block,
        Some(anchor)
    );
}

pub(super) fn assert_duplicate_rows_abort_the_entire_block(db: &impl BitcoinObserverDb) {
    let anchor = make_block(0, 0);
    db.initialize(&make_metadata(), Some(anchor))
        .expect("initialize");
    let first = make_block(1, 0);
    let request = make_observation(first);
    let result = db.apply_block(Some(anchor), first, &[request.clone(), request.clone()], 4);
    assert!(matches!(
        result,
        Err(DbError::BitcoinObserverConsistency(
            BitcoinObserverConsistencyError::ObserverStateMismatch
        ))
    ));

    // A failure after the first insert must leave neither that insert nor an advanced state.
    assert!(db
        .get_deposit(request.outpoint())
        .expect("row after abort")
        .is_none());
    assert_eq!(
        db.get_observer_state()
            .expect("state")
            .expect("initialized")
            .last_processed_block,
        Some(anchor)
    );
    assert_eq!(
        db.rollback_history(4).expect("history after abort"),
        vec![Some(anchor)]
    );
}

pub(super) fn assert_replay_is_idempotent_and_conflicting_blocks_are_rejected(
    db: &impl BitcoinObserverDb,
) {
    let metadata = make_metadata();
    let anchor = make_block(0, 0);
    db.initialize(&metadata, Some(anchor)).expect("initialize");
    let first = make_block(1, 0);
    let request = make_observation(first);
    db.apply_block(Some(anchor), first, std::slice::from_ref(&request), 4)
        .expect("first block");
    db.apply_block(Some(anchor), first, std::slice::from_ref(&request), 4)
        .expect("identical replay");
    assert_eq!(
        db.rollback_history(4).expect("one undo record"),
        vec![Some(first), Some(anchor)]
    );
    assert!(matches!(
        db.apply_block(Some(anchor), make_block(1, 1), &[], 4),
        Err(DbError::BitcoinObserverConsistency(
            BitcoinObserverConsistencyError::ObserverStateMismatch
        ))
    ));
    assert_eq!(
        db.get_deposit(request.outpoint()).expect("unchanged row"),
        Some(request)
    );
}

pub(super) fn assert_changed_scan_configuration_is_rejected(db: &impl BitcoinObserverDb) {
    let metadata = make_metadata();
    let anchor = make_block(0, 0);
    db.initialize(&metadata, Some(anchor)).expect("initialize");

    let mut changed_network = metadata.clone();
    changed_network.genesis_hash = genesis_block(Network::Bitcoin).block_hash();
    assert!(matches!(
        db.initialize(&changed_network, Some(anchor)),
        Err(DbError::BitcoinObserverConsistency(
            BitcoinObserverConsistencyError::NetworkMismatch
        ))
    ));

    let mut changed_params = metadata.clone();
    changed_params.params_hash = sha256::Hash::hash(b"changed observer parameters");
    assert!(matches!(
        db.initialize(&changed_params, Some(anchor)),
        Err(DbError::BitcoinObserverConsistency(
            BitcoinObserverConsistencyError::ParamsMismatch
        ))
    ));

    assert_eq!(
        db.get_observer_state()
            .expect("state")
            .expect("initialized")
            .metadata,
        metadata
    );
}

pub(super) fn assert_conflicting_block_replay_is_rejected(db: &impl BitcoinObserverDb) {
    let anchor = make_block(0, 0);
    let block = make_block(1, 0);
    db.initialize(&make_metadata(), Some(anchor))
        .expect("initialize");
    let first = make_observation(block);
    let mut second = first.clone();
    second.request = DepositFixture::with_request_seed(4).request_tx().clone();
    second.inclusion.position = 1;
    let original = [first.clone(), second.clone()];
    db.apply_block(Some(anchor), block, &original, 4)
        .expect("original block");

    // Row order is incidental; replay must preserve the exact set of observations.
    db.apply_block(Some(anchor), block, &[second.clone(), first.clone()], 4)
        .expect("same observations reordered");
    let state = db.get_observer_state().expect("original state");
    let history = db.rollback_history(4).expect("original history");

    let mut changed = first.clone();
    changed.inclusion.position += 2;
    let mut unexpected = second.clone();
    unexpected.request = DepositFixture::with_request_seed(5).request_tx().clone();
    for replay in [
        vec![first.clone(), unexpected.clone()],
        vec![first.clone(), second.clone(), unexpected],
        vec![changed, second.clone()],
        vec![first.clone()],
        vec![],
        vec![first.clone(), first.clone()],
    ] {
        assert!(
            matches!(db.apply_block(Some(anchor), block, &replay, 4), Err(DbError::BitcoinObserverConsistency(BitcoinObserverConsistencyError::ConflictingBlockReplay(conflict))) if conflict == block)
        );
    }
    assert!(
        matches!(db.apply_block(Some(make_block(0, 1)), block, &original, 4), Err(DbError::BitcoinObserverConsistency(BitcoinObserverConsistencyError::ConflictingBlockReplay(conflict))) if conflict == block)
    );

    // Rejected deliveries cannot alter rows, scan progress or undo history.
    assert_eq!(
        db.get_deposit(first.outpoint()).expect("first row"),
        Some(first)
    );
    assert_eq!(
        db.get_deposit(second.outpoint()).expect("second row"),
        Some(second)
    );
    assert_eq!(db.get_observer_state().expect("unchanged state"), state);
    assert_eq!(db.rollback_history(4).expect("unchanged history"), history);
}
