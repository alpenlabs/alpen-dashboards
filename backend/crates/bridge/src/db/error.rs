use strata_identifiers::L1BlockCommitment;

use crate::db::types::DbWithdrawalEventKey;

/// Violated withdrawal-index invariant.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WithdrawalIndexConsistencyError {
    #[error("withdrawal sequence overflow at {0}")]
    SeqOverflow(u64),

    #[error("withdrawal sequence {0} is already occupied")]
    SeqOccupied(u64),

    #[error("withdrawal event index inconsistent for {0:?}: first_seq {1}, count {2}")]
    EventIndexInconsistent(DbWithdrawalEventKey, u64, u32),
}

/// Violated Bitcoin observer storage invariant.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BitcoinObserverConsistencyError {
    #[error("stored Bitcoin network genesis does not match the configured node")]
    NetworkMismatch,

    #[error("stored Bitcoin deposit parameters do not match the configured protocol parameters")]
    ParamsMismatch,

    #[error("Bitcoin observer state changed or block does not extend the last processed block")]
    ObserverStateMismatch,

    #[error("conflicting observations replayed for Bitcoin block {0}")]
    ConflictingBlockReplay(L1BlockCommitment),

    #[error("Bitcoin observer revision overflow")]
    RevisionOverflow,

    #[error("Bitcoin observer undo record is unavailable")]
    MissingUndo,
}

/// Error returned by bridge database implementations.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("withdrawal event contains no requests")]
    EmptyWithdrawalEvent,

    #[error("withdrawal event expands to too many requests: {0}")]
    TooManyWithdrawalEventRequests(usize),

    #[error("withdrawal event key mismatch: expected {expected:?}, got {got:?}")]
    WithdrawalEventKeyMismatch {
        expected: DbWithdrawalEventKey,
        got: DbWithdrawalEventKey,
    },

    #[error(transparent)]
    WithdrawalIndexConsistency(#[from] WithdrawalIndexConsistencyError),

    #[error(transparent)]
    BitcoinObserverConsistency(#[from] BitcoinObserverConsistencyError),

    #[error(transparent)]
    Sled(#[from] typed_sled::error::Error),
}

impl From<sled::Error> for DbError {
    fn from(value: sled::Error) -> Self {
        Self::Sled(value.into())
    }
}

/// Result of a bridge database operation.
pub type DbResult<T> = Result<T, DbError>;
