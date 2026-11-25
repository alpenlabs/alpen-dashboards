mod bridge_rpc;
mod cache;
mod context;
mod db;
mod l1;
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
    error::{BitcoinObserverConsistencyError, DbError, DbResult, WithdrawalIndexConsistencyError},
    traits::{BitcoinObserverDb, BridgeStatusDb, WithdrawalIndexerDb},
    types::{
        BitcoinObserverDbMetadata, BitcoinObserverState, BridgeStatusDbSnapshot, DbIndexerState,
        DbWithdrawalEventIndex, DbWithdrawalEventKey, DbWithdrawalRequest, DbWithdrawalRequestRow,
        StatusCursors,
    },
    BitcoinObserverDbSled, BridgeStatusDbSled, WithdrawalIndexerDbSled,
};
pub use l1::{
    deposits::{DepositError, DepositSpend},
    esplora::{EsploraClient, EsploraError},
    observations::{ObservedDeposit, ObservedDepositSpend, TransactionInclusion},
    observer::{run_bitcoin_observer, BitcoinObserverError, BitcoinObserverStatus},
    params::{BitcoinParams, BitcoinParamsError},
    rpc::BitcoinRpcError,
};
pub use status::{bridge_monitoring_task, get_bridge_status};
pub use types::{
    BridgeStatus, ReimbursementStatusCursor, WithdrawalInfo, WithdrawalPairing,
    WithdrawalPairingCursor, WithdrawalSeq, WithdrawalStatus, WithdrawalStatusCursor,
};
pub use withdrawal_indexer::task::run_withdrawal_indexer;
