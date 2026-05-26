use std::collections::BTreeMap;
use std::sync::RwLock;

use strata_bridge_primitives::types::DepositIdx;

use crate::{
    db::{
        error::DbResult,
        traits::BridgeStatusDb,
        types::{DbBridgeStatusSnapshot, StatusCursors},
    },
    types::{
        ReimbursementStatusCursor, WithdrawalInfo, WithdrawalPairingCursor, WithdrawalSeq,
        WithdrawalStatusCursor,
    },
};

/// In-memory bridge-status database for tests.
#[derive(Debug, Default)]
pub(crate) struct MockBridgeStatusDb {
    withdrawals: RwLock<BTreeMap<DepositIdx, WithdrawalInfo>>,
    withdrawal_pairings: RwLock<BTreeMap<DepositIdx, WithdrawalSeq>>,
    deposit_info_cursor: RwLock<DepositIdx>,
    withdrawal_pairing_cursor: RwLock<WithdrawalPairingCursor>,
    withdrawal_status_cursor: RwLock<WithdrawalStatusCursor>,
    reimbursement_status_cursor: RwLock<ReimbursementStatusCursor>,
}

impl BridgeStatusDb for MockBridgeStatusDb {
    fn get_status_snapshot(&self) -> DbResult<DbBridgeStatusSnapshot> {
        Ok(DbBridgeStatusSnapshot {
            withdrawals: self
                .withdrawals
                .read()
                .expect("mock withdrawals lock poisoned")
                .iter()
                .map(|(deposit_idx, info)| (*deposit_idx, *info))
                .collect(),
            withdrawal_pairings: self
                .withdrawal_pairings
                .read()
                .expect("mock withdrawal_pairings lock poisoned")
                .iter()
                .map(|(deposit_idx, withdrawal_seq)| (*deposit_idx, *withdrawal_seq))
                .collect(),
            cursors: StatusCursors {
                deposit_info: *self
                    .deposit_info_cursor
                    .read()
                    .expect("mock deposit_info_cursor lock poisoned"),
                withdrawal_pairing: *self
                    .withdrawal_pairing_cursor
                    .read()
                    .expect("mock withdrawal_pairing_cursor lock poisoned"),
                withdrawal_status: *self
                    .withdrawal_status_cursor
                    .read()
                    .expect("mock withdrawal_status_cursor lock poisoned"),
                reimbursement_status: *self
                    .reimbursement_status_cursor
                    .read()
                    .expect("mock reimbursement_status_cursor lock poisoned"),
            },
        })
    }

    fn put_withdrawal_info(&self, deposit_idx: DepositIdx, info: &WithdrawalInfo) -> DbResult<()> {
        self.withdrawals
            .write()
            .expect("mock withdrawals lock poisoned")
            .insert(deposit_idx, *info);
        Ok(())
    }

    fn del_withdrawal_info(&self, deposit_idx: DepositIdx) -> DbResult<bool> {
        Ok(self
            .withdrawals
            .write()
            .expect("mock withdrawals lock poisoned")
            .remove(&deposit_idx)
            .is_some())
    }

    fn put_withdrawal_pairings(&self, pairings: &[(DepositIdx, WithdrawalSeq)]) -> DbResult<()> {
        self.withdrawal_pairings
            .write()
            .expect("mock withdrawal_pairings lock poisoned")
            .extend(pairings.iter().copied());
        Ok(())
    }

    fn del_withdrawal_pairings_range(&self, start: DepositIdx, end: DepositIdx) -> DbResult<()> {
        if start >= end {
            return Ok(());
        }

        let mut withdrawal_pairings = self
            .withdrawal_pairings
            .write()
            .expect("mock withdrawal_pairings lock poisoned");
        let deposit_indices = withdrawal_pairings
            .range(start..end)
            .map(|(deposit_idx, _)| *deposit_idx)
            .collect::<Vec<_>>();
        for deposit_idx in deposit_indices {
            withdrawal_pairings.remove(&deposit_idx);
        }
        Ok(())
    }

    fn put_deposit_info_cursor(&self, cursor: DepositIdx) -> DbResult<()> {
        *self
            .deposit_info_cursor
            .write()
            .expect("mock deposit_info_cursor lock poisoned") = cursor;
        Ok(())
    }

    fn put_withdrawal_pairing_cursor(&self, cursor: WithdrawalPairingCursor) -> DbResult<()> {
        *self
            .withdrawal_pairing_cursor
            .write()
            .expect("mock withdrawal_pairing_cursor lock poisoned") = cursor;
        Ok(())
    }

    fn put_withdrawal_status_cursor(&self, cursor: WithdrawalStatusCursor) -> DbResult<()> {
        *self
            .withdrawal_status_cursor
            .write()
            .expect("mock withdrawal_status_cursor lock poisoned") = cursor;
        Ok(())
    }

    fn put_reimbursement_status_cursor(&self, cursor: ReimbursementStatusCursor) -> DbResult<()> {
        *self
            .reimbursement_status_cursor
            .write()
            .expect("mock reimbursement_status_cursor lock poisoned") = cursor;
        Ok(())
    }
}
