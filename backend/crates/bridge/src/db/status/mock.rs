use std::collections::BTreeMap;
use std::sync::RwLock;

use strata_bridge_primitives::types::DepositIdx;

use crate::{
    db::{
        error::DbResult,
        traits::BridgeStatusDb,
        types::{BridgeStatusDbSnapshot, StatusCursors},
    },
    types::{
        ReimbursementStatusCursor, WithdrawalInfo, WithdrawalPairing, WithdrawalPairingCursor,
        WithdrawalSeq, WithdrawalStatusCursor,
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

impl MockBridgeStatusDb {
    /// Creates a database holding the records in `snapshot`.
    pub(crate) fn from_snapshot(snapshot: BridgeStatusDbSnapshot) -> Self {
        Self {
            withdrawals: RwLock::new(snapshot.withdrawals.into_iter().collect()),
            withdrawal_pairings: RwLock::new(
                snapshot
                    .withdrawal_pairings
                    .into_iter()
                    .map(|pairing| (pairing.deposit_idx, pairing.withdrawal_seq))
                    .collect(),
            ),
            deposit_info_cursor: RwLock::new(snapshot.cursors.deposit_info),
            withdrawal_pairing_cursor: RwLock::new(snapshot.cursors.withdrawal_pairing),
            withdrawal_status_cursor: RwLock::new(snapshot.cursors.withdrawal_status),
            reimbursement_status_cursor: RwLock::new(snapshot.cursors.reimbursement_status),
        }
    }
}

impl BridgeStatusDb for MockBridgeStatusDb {
    fn get_status_snapshot(&self) -> DbResult<BridgeStatusDbSnapshot> {
        Ok(BridgeStatusDbSnapshot::new(
            self.withdrawals
                .read()
                .expect("mock withdrawals lock poisoned")
                .iter()
                .map(|(deposit_idx, info)| (*deposit_idx, *info))
                .collect(),
            self.withdrawal_pairings
                .read()
                .expect("mock withdrawal_pairings lock poisoned")
                .iter()
                .map(|(deposit_idx, withdrawal_seq)| {
                    WithdrawalPairing::new(*deposit_idx, *withdrawal_seq)
                })
                .collect(),
            StatusCursors::new(
                *self
                    .deposit_info_cursor
                    .read()
                    .expect("mock deposit_info_cursor lock poisoned"),
                *self
                    .withdrawal_pairing_cursor
                    .read()
                    .expect("mock withdrawal_pairing_cursor lock poisoned"),
                *self
                    .withdrawal_status_cursor
                    .read()
                    .expect("mock withdrawal_status_cursor lock poisoned"),
                *self
                    .reimbursement_status_cursor
                    .read()
                    .expect("mock reimbursement_status_cursor lock poisoned"),
            ),
        ))
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

    fn put_withdrawal_pairings(&self, pairings: &[WithdrawalPairing]) -> DbResult<()> {
        self.withdrawal_pairings
            .write()
            .expect("mock withdrawal_pairings lock poisoned")
            .extend(
                pairings
                    .iter()
                    .map(|pairing| (pairing.deposit_idx, pairing.withdrawal_seq)),
            );
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
