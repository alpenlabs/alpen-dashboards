use std::path::Path;

use anyhow::Context;
use bitcoin::OutPoint;
use sled::transaction::{TransactionError, TransactionResult};
use strata_bridge_primitives::types::DepositIdx;
use typed_sled::{
    error::Error as TypedSledError, transaction::SledTransactional, SledDb, SledTree,
};

use crate::{
    db::{
        error::{DbError, DbResult},
        traits::BridgeStatusDb,
        types::{BridgeStatusDbSnapshot, DepositAnswer, StatusCursors},
    },
    types::{
        ReimbursementStatusCursor, WithdrawalInfo, WithdrawalPairing, WithdrawalPairingCursor,
        WithdrawalStatusCursor,
    },
};

use super::schema::{
    DepositAnswerSchema, DepositInfoCursorSchema, ReimbursementStatusCursorSchema,
    WithdrawalInfoSchema, WithdrawalPairingCursorSchema, WithdrawalPairingSchema,
    WithdrawalStatusCursorSchema,
};

const CURSOR_CELL_KEY: u8 = 0;

/// Sled-backed bridge-status database.
#[derive(Debug)]
pub struct BridgeStatusDbSled {
    _db: SledDb,
    deposit_answers: SledTree<DepositAnswerSchema>,
    withdrawals: SledTree<WithdrawalInfoSchema>,
    withdrawal_pairings: SledTree<WithdrawalPairingSchema>,
    deposit_info_cursor: SledTree<DepositInfoCursorSchema>,
    withdrawal_pairing_cursor: SledTree<WithdrawalPairingCursorSchema>,
    withdrawal_status_cursor: SledTree<WithdrawalStatusCursorSchema>,
    reimbursement_status_cursor: SledTree<ReimbursementStatusCursorSchema>,
}

impl BridgeStatusDbSled {
    /// Open the status database under `{datadir}/status`.
    pub fn open(datadir: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = datadir.as_ref().join("status");
        std::fs::create_dir_all(&path)
            .with_context(|| format!("create bridge status db dir {}", path.display()))?;
        let sled_db = sled::open(&path)
            .with_context(|| format!("open bridge status sled db at {}", path.display()))?;
        // typed-sled codec errors are not Send + Sync, so anyhow::Context cannot preserve them.
        Self::from_sled_db(sled_db)
            .map_err(|e| anyhow::anyhow!("initialize bridge status trees: {e}"))
    }

    /// Open a temporary in-memory-like sled database deleted on drop.
    #[cfg(test)]
    pub fn open_temporary() -> anyhow::Result<Self> {
        let sled_db = sled::Config::new()
            .temporary(true)
            .open()
            .context("open temporary bridge status sled db")?;
        // typed-sled codec errors are not Send + Sync, so anyhow::Context cannot preserve them.
        Self::from_sled_db(sled_db)
            .map_err(|e| anyhow::anyhow!("initialize temporary bridge status trees: {e}"))
    }

    fn from_sled_db(sled_db: sled::Db) -> DbResult<Self> {
        let db = SledDb::new(sled_db)?;

        Ok(Self {
            deposit_answers: db.get_tree::<DepositAnswerSchema>()?,
            withdrawals: db.get_tree::<WithdrawalInfoSchema>()?,
            withdrawal_pairings: db.get_tree::<WithdrawalPairingSchema>()?,
            deposit_info_cursor: db.get_tree::<DepositInfoCursorSchema>()?,
            withdrawal_pairing_cursor: db.get_tree::<WithdrawalPairingCursorSchema>()?,
            withdrawal_status_cursor: db.get_tree::<WithdrawalStatusCursorSchema>()?,
            reimbursement_status_cursor: db.get_tree::<ReimbursementStatusCursorSchema>()?,
            _db: db,
        })
    }

    fn status_cursors(&self) -> DbResult<StatusCursors> {
        Ok(StatusCursors::new(
            self.deposit_info_cursor
                .get(&CURSOR_CELL_KEY)?
                .unwrap_or_default(),
            self.withdrawal_pairing_cursor
                .get(&CURSOR_CELL_KEY)?
                .unwrap_or_default(),
            self.withdrawal_status_cursor
                .get(&CURSOR_CELL_KEY)?
                .unwrap_or_default(),
            self.reimbursement_status_cursor
                .get(&CURSOR_CELL_KEY)?
                .unwrap_or_default(),
        ))
    }
}

impl BridgeStatusDb for BridgeStatusDbSled {
    fn get_status_snapshot(&self) -> DbResult<BridgeStatusDbSnapshot> {
        Ok(BridgeStatusDbSnapshot::new(
            self.deposit_answers
                .iter()
                .map(|result| result.map_err(DbError::from))
                .collect::<DbResult<_>>()?,
            self.withdrawals
                .iter()
                .map(|result| result.map_err(DbError::from))
                .collect::<DbResult<_>>()?,
            self.withdrawal_pairings
                .iter()
                .map(|result| result.map_err(DbError::from))
                .map(|result| {
                    result.map(|(deposit_idx, withdrawal_seq)| {
                        WithdrawalPairing::new(deposit_idx, withdrawal_seq)
                    })
                })
                .collect::<DbResult<_>>()?,
            self.status_cursors()?,
        ))
    }

    fn put_deposit_answers(&self, answers: &[(OutPoint, DepositAnswer)]) -> DbResult<()> {
        if answers.is_empty() {
            return Ok(());
        }
        // Transactional inserts request a single flush at commit and propagate its error.
        let result: TransactionResult<(), TypedSledError> =
            (&self.deposit_answers,).transaction(|(deposits,)| {
                for (outpoint, answer) in answers {
                    deposits.insert(outpoint, answer)?;
                }
                Ok(())
            });
        match result {
            Ok(()) => Ok(()),
            Err(TransactionError::Storage(error)) => Err(error.into()),
            Err(TransactionError::Abort(error)) => Err(error.into()),
        }
    }

    fn put_withdrawal_info(&self, deposit_idx: DepositIdx, info: &WithdrawalInfo) -> DbResult<()> {
        self.withdrawals.insert(&deposit_idx, info)?;
        Ok(())
    }

    fn del_withdrawal_info(&self, deposit_idx: DepositIdx) -> DbResult<bool> {
        Ok(self.withdrawals.take(&deposit_idx)?.is_some())
    }

    fn put_withdrawal_pairings(&self, pairings: &[WithdrawalPairing]) -> DbResult<()> {
        for pairing in pairings {
            self.withdrawal_pairings
                .insert(&pairing.deposit_idx, &pairing.withdrawal_seq)?;
        }
        Ok(())
    }

    fn del_withdrawal_pairings_range(&self, start: DepositIdx, end: DepositIdx) -> DbResult<()> {
        if start >= end {
            return Ok(());
        }

        let deposit_indices = self
            .withdrawal_pairings
            .range(start..end)?
            .map(|result| {
                result
                    .map(|(deposit_idx, _)| deposit_idx)
                    .map_err(DbError::from)
            })
            .collect::<DbResult<Vec<_>>>()?;
        for deposit_idx in deposit_indices {
            self.withdrawal_pairings.remove(&deposit_idx)?;
        }
        Ok(())
    }

    fn put_deposit_info_cursor(&self, cursor: DepositIdx) -> DbResult<()> {
        self.deposit_info_cursor.insert(&CURSOR_CELL_KEY, &cursor)?;
        Ok(())
    }

    fn put_withdrawal_pairing_cursor(&self, cursor: WithdrawalPairingCursor) -> DbResult<()> {
        self.withdrawal_pairing_cursor
            .insert(&CURSOR_CELL_KEY, &cursor)?;
        Ok(())
    }

    fn put_withdrawal_status_cursor(&self, cursor: WithdrawalStatusCursor) -> DbResult<()> {
        self.withdrawal_status_cursor
            .insert(&CURSOR_CELL_KEY, &cursor)?;
        Ok(())
    }

    fn put_reimbursement_status_cursor(&self, cursor: ReimbursementStatusCursor) -> DbResult<()> {
        self.reimbursement_status_cursor
            .insert(&CURSOR_CELL_KEY, &cursor)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, fs};

    use bitcoin::{hashes::Hash, Txid};
    use strata_primitives::buf::Buf32;

    use super::*;
    use crate::{
        db::status::mock::MockBridgeStatusDb,
        l1::deposits::deposit_request_outpoint,
        test_utils::make_unique_db_path,
        types::{WithdrawalInfo, WithdrawalPairing, WithdrawalStatus},
    };

    fn make_txid(byte: u8) -> Txid {
        Txid::from_byte_array([byte; 32])
    }

    fn make_withdrawal_info(byte: u8, status: WithdrawalStatus) -> WithdrawalInfo {
        WithdrawalInfo {
            withdrawal_request_txid: Buf32([byte; 32]),
            fulfillment_txid: Some(make_txid(byte + 1)),
            status,
        }
    }

    fn assert_empty_snapshot(db: &impl BridgeStatusDb) {
        let snapshot = db.get_status_snapshot().expect("snapshot");
        assert!(snapshot.deposit_answers.is_empty());
        assert!(snapshot.withdrawals.is_empty());
        assert!(snapshot.withdrawal_pairings.is_empty());
        assert_eq!(snapshot.cursors, StatusCursors::default());
    }

    fn assert_snapshot_reflects_puts_and_deletes(db: &impl BridgeStatusDb) {
        let withdrawal = make_withdrawal_info(3, WithdrawalStatus::Complete);
        let cursors = StatusCursors {
            deposit_info: 11,
            withdrawal_pairing: WithdrawalPairingCursor {
                next_deposit_idx: 12,
                next_withdrawal_seq: 13,
            },
            withdrawal_status: WithdrawalStatusCursor {
                next_deposit_idx: 14,
            },
            reimbursement_status: ReimbursementStatusCursor {
                next_deposit_idx: 15,
            },
        };

        db.put_withdrawal_info(2, &withdrawal)
            .expect("put withdrawal");
        let pairings = [WithdrawalPairing::new(1, 7), WithdrawalPairing::new(2, 8)];
        db.put_withdrawal_pairings(&pairings).expect("put pairings");
        db.put_deposit_info_cursor(cursors.deposit_info)
            .expect("put deposit cursor");
        db.put_withdrawal_pairing_cursor(cursors.withdrawal_pairing)
            .expect("put pairing cursor");
        db.put_withdrawal_status_cursor(cursors.withdrawal_status)
            .expect("put withdrawal cursor");
        db.put_reimbursement_status_cursor(cursors.reimbursement_status)
            .expect("put reimbursement cursor");

        let snapshot = db.get_status_snapshot().expect("snapshot");
        assert_eq!(snapshot.withdrawals.len(), 1);
        assert_eq!(snapshot.withdrawals[0].0, 2);
        assert_eq!(
            snapshot.withdrawals[0].1.withdrawal_request_txid,
            Buf32([3; 32])
        );
        assert_eq!(snapshot.withdrawal_pairings, pairings);
        assert_eq!(snapshot.cursors, cursors);

        db.del_withdrawal_pairings_range(0, 2)
            .expect("del pairings range");
        assert!(db.del_withdrawal_info(2).expect("del withdrawal"));
        assert!(!db.del_withdrawal_info(2).expect("del missing withdrawal"));

        let snapshot = db.get_status_snapshot().expect("snapshot after deletes");
        assert!(snapshot.withdrawals.is_empty());
        assert_eq!(
            snapshot.withdrawal_pairings,
            vec![WithdrawalPairing::new(2, 8)]
        );
        assert_eq!(snapshot.cursors, cursors);
    }

    fn assert_pairing_range_delete_removes_only_rows_in_range(db: &impl BridgeStatusDb) {
        let pairings = [1, 2, 4, 5]
            .map(|deposit_idx| WithdrawalPairing::new(deposit_idx, u64::from(deposit_idx) * 10));
        db.put_withdrawal_pairings(&pairings).expect("put pairings");
        let remaining = vec![WithdrawalPairing::new(1, 10), WithdrawalPairing::new(5, 50)];

        db.del_withdrawal_pairings_range(2, 5)
            .expect("delete middle range");
        assert_eq!(
            db.get_status_snapshot()
                .expect("snapshot")
                .withdrawal_pairings,
            remaining
        );

        db.del_withdrawal_pairings_range(5, 5)
            .expect("delete empty range");
        db.del_withdrawal_pairings_range(6, 2)
            .expect("delete reversed range");
        assert_eq!(
            db.get_status_snapshot()
                .expect("snapshot")
                .withdrawal_pairings,
            remaining
        );
    }

    fn assert_deposit_answers_survive_cursor_updates(db: &impl BridgeStatusDb) {
        let request = deposit_request_outpoint(make_txid(1));
        let other_request = deposit_request_outpoint(make_txid(2));
        db.put_deposit_answers(&[
            (request, DepositAnswer::InProgress),
            (other_request, DepositAnswer::Complete),
        ])
        .expect("record answers");
        db.put_deposit_answers(&[(request, DepositAnswer::Failed)])
            .expect("replace answer");
        db.put_deposit_info_cursor(2).expect("advance cursor");
        db.put_deposit_answers(&[])
            .expect("no successful responses");
        let snapshot = db.get_status_snapshot().expect("snapshot");
        let answers = snapshot
            .deposit_answers
            .into_iter()
            .collect::<BTreeMap<_, _>>();
        assert_eq!(answers.len(), 2);
        assert!(matches!(answers.get(&request), Some(DepositAnswer::Failed)));
        assert!(matches!(
            answers.get(&other_request),
            Some(DepositAnswer::Complete)
        ));
        assert_eq!(snapshot.cursors.deposit_info, 2);
    }

    #[test]
    fn deposit_answers_survive_cursor_updates_sled() {
        let db = BridgeStatusDbSled::open_temporary().expect("open db");
        assert_deposit_answers_survive_cursor_updates(&db);
    }

    #[test]
    fn deposit_answers_survive_cursor_updates_mock() {
        assert_deposit_answers_survive_cursor_updates(&MockBridgeStatusDb::default());
    }

    #[test]
    fn new_status_db_returns_empty_snapshot_sled() {
        let db = BridgeStatusDbSled::open_temporary().expect("open db");
        assert_empty_snapshot(&db);
    }

    #[test]
    fn new_status_db_returns_empty_snapshot_mock() {
        assert_empty_snapshot(&MockBridgeStatusDb::default());
    }

    #[test]
    fn status_db_snapshot_reflects_puts_and_deletes_sled() {
        let db = BridgeStatusDbSled::open_temporary().expect("open db");
        assert_snapshot_reflects_puts_and_deletes(&db);
    }

    #[test]
    fn status_db_snapshot_reflects_puts_and_deletes_mock() {
        assert_snapshot_reflects_puts_and_deletes(&MockBridgeStatusDb::default());
    }

    #[test]
    fn pairing_range_delete_removes_only_rows_in_range_sled() {
        let db = BridgeStatusDbSled::open_temporary().expect("open db");
        assert_pairing_range_delete_removes_only_rows_in_range(&db);
    }

    #[test]
    fn pairing_range_delete_removes_only_rows_in_range_mock() {
        assert_pairing_range_delete_removes_only_rows_in_range(&MockBridgeStatusDb::default());
    }

    #[test]
    fn status_rows_persist_across_reopen() {
        let path = make_unique_db_path("reopen");
        let withdrawal = make_withdrawal_info(9, WithdrawalStatus::Complete);
        let deposit_info_cursor = 42;

        {
            let db = BridgeStatusDbSled::open(&path).expect("open db");
            db.put_withdrawal_info(7, &withdrawal)
                .expect("put withdrawal");
            db.put_deposit_info_cursor(deposit_info_cursor)
                .expect("put cursor");
        }

        {
            let db = BridgeStatusDbSled::open(&path).expect("reopen db");
            let snapshot = db.get_status_snapshot().expect("snapshot");
            assert_eq!(snapshot.withdrawals.len(), 1);
            assert_eq!(snapshot.withdrawals[0].0, 7);
            assert_eq!(
                snapshot.withdrawals[0].1.withdrawal_request_txid,
                Buf32([9; 32])
            );
            assert_eq!(snapshot.cursors.deposit_info, deposit_info_cursor);
        }

        let _ = fs::remove_dir_all(path);
    }
}
