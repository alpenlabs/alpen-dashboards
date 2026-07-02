use std::collections::{BTreeMap, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};
use strata_bridge_primitives::types::DepositIdx;

use super::{
    db::types::DbBridgeStatusSnapshot,
    types::{
        DepositInfo, OperatorStatus, ReimbursementInfo, ReimbursementStatusCursor, WithdrawalInfo,
        WithdrawalPairing, WithdrawalPairingCursor, WithdrawalSeq, WithdrawalStatusCursor,
    },
};

/// Cache entry with timestamp and confirmation tracking
#[derive(Debug, Clone)]
pub(crate) struct CacheEntry<T> {
    pub(crate) data: T,
    pub(crate) confirmations: Option<u64>,
    pub(crate) last_updated: u64,
}

impl<T> CacheEntry<T> {
    pub(crate) fn new(data: T, confirmations: Option<u64>) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        Self {
            data,
            confirmations,
            last_updated: now,
        }
    }

    pub(crate) fn update(&mut self, data: T, confirmations: Option<u64>) {
        self.data = data;
        self.confirmations = confirmations;
        self.last_updated = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
    }
}

/// In-memory withdrawal-to-deposit pairings and their FIFO cursor.
#[derive(Debug, Default, Clone)]
pub(crate) struct WithdrawalPairingState {
    pairings: BTreeMap<DepositIdx, WithdrawalSeq>,
    cursor: WithdrawalPairingCursor,
}

/// In-memory cache for bridge monitoring data
#[derive(Debug, Default, Clone)]
pub(crate) struct BridgeStatusCache {
    deposits: HashMap<DepositIdx, CacheEntry<DepositInfo>>,
    deposit_info_cursor: DepositIdx,
    withdrawal_pairing: WithdrawalPairingState,
    withdrawal_status_cursor: WithdrawalStatusCursor,
    withdrawals: HashMap<DepositIdx, CacheEntry<WithdrawalInfo>>,
    reimbursement_status_cursor: ReimbursementStatusCursor,
    reimbursements: HashMap<DepositIdx, CacheEntry<ReimbursementInfo>>,
    operators: Vec<OperatorStatus>,
}

impl BridgeStatusCache {
    pub(crate) fn from_status_snapshot(snapshot: DbBridgeStatusSnapshot) -> Self {
        let mut cache = Self::default();

        cache.apply_withdrawal_updates(
            snapshot
                .withdrawals
                .into_iter()
                .map(|(deposit_idx, info)| (deposit_idx, info, None))
                .collect(),
        );
        cache.update_withdrawal_pairings(
            &snapshot.withdrawal_pairings,
            snapshot.cursors.withdrawal_pairing,
        );
        cache.set_deposit_info_cursor(snapshot.cursors.deposit_info);
        cache.set_withdrawal_status_cursor(snapshot.cursors.withdrawal_status);
        cache.set_reimbursement_status_cursor(snapshot.cursors.reimbursement_status);

        cache
    }

    pub(crate) fn deposit_info_cursor(&self) -> DepositIdx {
        self.deposit_info_cursor
    }

    pub(crate) fn set_deposit_info_cursor(&mut self, cursor: DepositIdx) {
        self.deposit_info_cursor = cursor;
    }

    pub(crate) fn withdrawal_pairing_cursor(&self) -> WithdrawalPairingCursor {
        self.withdrawal_pairing.cursor
    }

    pub(crate) fn withdrawal_pairings_from(
        &self,
        deposit_idx: DepositIdx,
    ) -> Vec<WithdrawalPairing> {
        self.withdrawal_pairing
            .pairings
            .range(deposit_idx..)
            .map(|(deposit_idx, withdrawal_seq)| {
                WithdrawalPairing::new(*deposit_idx, *withdrawal_seq)
            })
            .collect()
    }

    pub(crate) fn update_withdrawal_pairings(
        &mut self,
        pairings: &[WithdrawalPairing],
        cursor: WithdrawalPairingCursor,
    ) {
        self.withdrawal_pairing.pairings.extend(
            pairings
                .iter()
                .map(|pairing| (pairing.deposit_idx, pairing.withdrawal_seq)),
        );
        self.withdrawal_pairing.cursor = cursor;
    }

    pub(crate) fn purge_withdrawal_pairings_range(&mut self, start: DepositIdx, end: DepositIdx) {
        if start >= end {
            return;
        }

        let deposit_indices = self
            .withdrawal_pairing
            .pairings
            .range(start..end)
            .map(|(deposit_idx, _)| *deposit_idx)
            .collect::<Vec<_>>();
        for deposit_idx in deposit_indices {
            self.withdrawal_pairing.pairings.remove(&deposit_idx);
        }
    }

    pub(crate) fn withdrawal_status_cursor(&self) -> WithdrawalStatusCursor {
        self.withdrawal_status_cursor
    }

    pub(crate) fn set_withdrawal_status_cursor(&mut self, cursor: WithdrawalStatusCursor) {
        self.withdrawal_status_cursor = cursor;
    }

    pub(crate) fn reimbursement_status_cursor(&self) -> ReimbursementStatusCursor {
        self.reimbursement_status_cursor
    }

    pub(crate) fn set_reimbursement_status_cursor(&mut self, cursor: ReimbursementStatusCursor) {
        self.reimbursement_status_cursor = cursor;
    }

    /// Update deposit cache entry
    pub(crate) fn update_deposit(
        &mut self,
        deposit_idx: DepositIdx,
        info: DepositInfo,
        confirmations: Option<u64>,
    ) {
        if let Some(entry) = self.deposits.get_mut(&deposit_idx) {
            entry.update(info, confirmations);
        } else {
            self.deposits
                .insert(deposit_idx, CacheEntry::new(info, confirmations));
        }
    }

    /// Update withdrawal cache entry
    pub(crate) fn update_withdrawal(
        &mut self,
        deposit_idx: DepositIdx,
        info: WithdrawalInfo,
        confirmations: Option<u64>,
    ) {
        if let Some(entry) = self.withdrawals.get_mut(&deposit_idx) {
            entry.update(info, confirmations);
        } else {
            self.withdrawals
                .insert(deposit_idx, CacheEntry::new(info, confirmations));
        }
    }

    /// Update reimbursement cache entry
    pub(crate) fn update_reimbursement(
        &mut self,
        deposit_idx: DepositIdx,
        info: ReimbursementInfo,
        confirmations: Option<u64>,
    ) {
        if let Some(entry) = self.reimbursements.get_mut(&deposit_idx) {
            entry.update(info, confirmations);
        } else {
            self.reimbursements
                .insert(deposit_idx, CacheEntry::new(info, confirmations));
        }
    }

    /// Update operators
    pub(crate) fn update_operators(&mut self, operators: Vec<OperatorStatus>) {
        self.operators = operators;
    }

    /// Get all operators
    pub(crate) fn get_operators(&self) -> Vec<OperatorStatus> {
        self.operators.clone()
    }

    /// Batch update deposits
    pub(crate) fn apply_deposit_updates(
        &mut self,
        updates: Vec<(DepositIdx, DepositInfo, Option<u64>)>,
    ) {
        for (deposit_idx, info, confirmations) in updates {
            self.update_deposit(deposit_idx, info, confirmations);
        }
    }

    /// Batch update withdrawals
    pub(crate) fn apply_withdrawal_updates(
        &mut self,
        updates: Vec<(DepositIdx, WithdrawalInfo, Option<u64>)>,
    ) {
        for (deposit_idx, info, confirmations) in updates {
            self.update_withdrawal(deposit_idx, info, confirmations);
        }
    }

    /// Batch update reimbursements
    pub(crate) fn apply_reimbursement_updates(
        &mut self,
        updates: Vec<(DepositIdx, ReimbursementInfo, Option<u64>)>,
    ) {
        for (deposit_idx, info, confirmations) in updates {
            self.update_reimbursement(deposit_idx, info, confirmations);
        }
    }

    /// Filter deposits based on deposit index, row value, and confirmations.
    pub(crate) fn filter_deposits<F>(&self, filter: F) -> Vec<(DepositIdx, DepositInfo)>
    where
        F: Fn(DepositIdx, &DepositInfo, Option<u64>) -> bool,
    {
        self.deposits
            .iter()
            .filter(|(deposit_idx, entry)| filter(**deposit_idx, &entry.data, entry.confirmations))
            .map(|(deposit_idx, entry)| (*deposit_idx, entry.data))
            .collect()
    }

    /// Filter withdrawals based on deposit index, row value, and confirmations.
    pub(crate) fn filter_withdrawals<F>(&self, filter: F) -> Vec<(DepositIdx, WithdrawalInfo)>
    where
        F: Fn(DepositIdx, &WithdrawalInfo, Option<u64>) -> bool,
    {
        self.withdrawals
            .iter()
            .filter(|(deposit_idx, entry)| filter(**deposit_idx, &entry.data, entry.confirmations))
            .map(|(deposit_idx, entry)| (*deposit_idx, entry.data))
            .collect()
    }

    /// Filter reimbursements based on deposit index, row value, and confirmations.
    pub(crate) fn filter_reimbursements<F>(&self, filter: F) -> Vec<(DepositIdx, ReimbursementInfo)>
    where
        F: Fn(DepositIdx, &ReimbursementInfo, Option<u64>) -> bool,
    {
        self.reimbursements
            .iter()
            .filter(|(deposit_idx, entry)| filter(**deposit_idx, &entry.data, entry.confirmations))
            .map(|(deposit_idx, entry)| (*deposit_idx, entry.data))
            .collect()
    }

    /// Purge specific deposit entries
    pub(crate) fn purge_deposits(&mut self, deposits_to_purge: Vec<DepositIdx>) {
        for deposit_idx in deposits_to_purge {
            self.deposits.remove(&deposit_idx);
        }
    }

    /// Purge specific withdrawal entries
    pub(crate) fn purge_withdrawals(&mut self, withdrawals_to_purge: Vec<DepositIdx>) {
        for deposit_idx in withdrawals_to_purge {
            self.withdrawals.remove(&deposit_idx);
        }
    }

    /// Purge specific reimbursement entries
    pub(crate) fn purge_reimbursements(&mut self, reimbursements_to_purge: Vec<DepositIdx>) {
        for deposit_idx in reimbursements_to_purge {
            self.reimbursements.remove(&deposit_idx);
        }
    }
}
