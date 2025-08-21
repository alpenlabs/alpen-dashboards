use bitcoin::Txid;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use strata_primitives::buf::Buf32;

use super::types::{
    DepositInfo, DepositStatus, OperatorStatus, ReimbursementInfo, ReimbursementStatus,
    WithdrawalInfo, WithdrawalStatus,
};

/// Cache entry with timestamp and confirmation tracking
#[derive(Debug, Clone)]
pub(crate) struct CacheEntry<T> {
    pub(crate) data: T,
    pub(crate) confirmations: u64,
    pub(crate) last_updated: u64,
}

impl<T> CacheEntry<T> {
    pub(crate) fn new(data: T, confirmations: u64) -> Self {
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

    pub(crate) fn update(&mut self, data: T, confirmations: u64) {
        self.data = data;
        self.confirmations = confirmations;
        self.last_updated = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
    }
}

/// In-memory cache for bridge monitoring data
#[derive(Debug, Default, Clone)]
pub(crate) struct BridgeStatusCache {
    deposits: HashMap<Txid, CacheEntry<DepositInfo>>,
    withdrawals: HashMap<Buf32, CacheEntry<WithdrawalInfo>>,
    reimbursements: HashMap<Txid, CacheEntry<ReimbursementInfo>>,
    operators: Vec<OperatorStatus>,
}

impl BridgeStatusCache {
    /// Update deposit cache entry
    pub(crate) fn update_deposit(&mut self, txid: Txid, info: DepositInfo, confirmations: u64) {
        if let Some(entry) = self.deposits.get_mut(&txid) {
            entry.update(info, confirmations);
        } else {
            self.deposits
                .insert(txid, CacheEntry::new(info, confirmations));
        }
    }

    /// Update withdrawal cache entry
    pub(crate) fn update_withdrawal(
        &mut self,
        request_id: Buf32,
        info: WithdrawalInfo,
        confirmations: u64,
    ) {
        if let Some(entry) = self.withdrawals.get_mut(&request_id) {
            entry.update(info, confirmations);
        } else {
            self.withdrawals
                .insert(request_id, CacheEntry::new(info, confirmations));
        }
    }

    /// Update reimbursement cache entry
    pub(crate) fn update_reimbursement(
        &mut self,
        txid: Txid,
        info: ReimbursementInfo,
        confirmations: u64,
    ) {
        if let Some(entry) = self.reimbursements.get_mut(&txid) {
            entry.update(info, confirmations);
        } else {
            self.reimbursements
                .insert(txid, CacheEntry::new(info, confirmations));
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
    pub(crate) fn apply_deposit_updates(&mut self, updates: Vec<(Txid, DepositInfo, u64)>) {
        for (txid, info, confirmations) in updates {
            self.update_deposit(txid, info, confirmations);
        }
    }

    /// Batch update withdrawals
    pub(crate) fn apply_withdrawal_updates(&mut self, updates: Vec<(Buf32, WithdrawalInfo, u64)>) {
        for (request_id, info, confirmations) in updates {
            self.update_withdrawal(request_id, info, confirmations);
        }
    }

    /// Batch update reimbursements
    pub(crate) fn apply_reimbursement_updates(
        &mut self,
        updates: Vec<(Txid, ReimbursementInfo, u64)>,
    ) {
        for (txid, info, confirmations) in updates {
            self.update_reimbursement(txid, info, confirmations);
        }
    }

    /// Filter deposits based on status condition
    pub(crate) fn filter_deposits<F>(&self, filter: F) -> Vec<(Txid, DepositInfo)>
    where
        F: Fn(&DepositStatus) -> bool,
    {
        self.deposits
            .iter()
            .filter(|(_, entry)| filter(&entry.data.status))
            .map(|(txid, entry)| (*txid, entry.data))
            .collect()
    }

    /// Filter withdrawals based on status condition
    pub(crate) fn filter_withdrawals<F>(&self, filter: F) -> Vec<(Buf32, WithdrawalInfo)>
    where
        F: Fn(&WithdrawalStatus) -> bool,
    {
        self.withdrawals
            .iter()
            .filter(|(_, entry)| filter(&entry.data.status))
            .map(|(request_id, entry)| (*request_id, entry.data))
            .collect()
    }

    /// Filter reimbursements based on status condition
    pub(crate) fn filter_reimbursements<F>(&self, filter: F) -> Vec<(Txid, ReimbursementInfo)>
    where
        F: Fn(&ReimbursementStatus) -> bool,
    {
        self.reimbursements
            .iter()
            .filter(|(_, entry)| filter(&entry.data.status))
            .map(|(txid, entry)| (*txid, entry.data))
            .collect()
    }

    /// Purge specific deposit entries
    pub(crate) fn purge_deposits(&mut self, deposits_to_purge: Vec<Txid>) {
        for txid in deposits_to_purge {
            self.deposits.remove(&txid);
        }
    }

    /// Purge specific withdrawal entries
    pub(crate) fn purge_withdrawals(&mut self, withdrawals_to_purge: Vec<Buf32>) {
        for request_id in withdrawals_to_purge {
            self.withdrawals.remove(&request_id);
        }
    }

    /// Purge specific reimbursement entries
    pub(crate) fn purge_reimbursements(&mut self, reimbursements_to_purge: Vec<Txid>) {
        for txid in reimbursements_to_purge {
            self.reimbursements.remove(&txid);
        }
    }
}
