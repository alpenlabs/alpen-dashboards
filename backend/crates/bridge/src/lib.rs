mod bridge_rpc;
mod cache;
mod context;
mod db;
mod esplora;
mod state;
mod status;
#[cfg(test)]
mod test_utils;
mod types;
mod withdrawal_indexer;
mod withdrawal_requests;
mod withdrawal_status;

pub use context::BridgeMonitoringContext;
pub use db::{
    error::{DbError, DbResult, WithdrawalIndexConsistencyError},
    traits::{BridgeStatusDb, WithdrawalIndexerDb},
    types::{
        BridgeStatusDbSnapshot, DbIndexerState, DbWithdrawalEventIndex, DbWithdrawalEventKey,
        DbWithdrawalRequest, DbWithdrawalRequestRow, StatusCursors,
    },
    BridgeStatusDbSled, WithdrawalIndexerDbSled,
};
pub use status::{bridge_monitoring_task, get_bridge_status};
pub use types::{
    BridgeStatus, ReimbursementStatusCursor, WithdrawalInfo, WithdrawalPairing,
    WithdrawalPairingCursor, WithdrawalSeq, WithdrawalStatus, WithdrawalStatusCursor,
};
pub use withdrawal_indexer::task::run_withdrawal_indexer;
