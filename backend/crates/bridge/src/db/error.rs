use std::path::PathBuf;

use crate::db::types::DbWithdrawalEventKey;

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "withdrawal-index consistency errors are constructed by follow-up indexer/pairing commits"
    )
)]
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WithdrawalIndexConsistencyError {
    #[error("withdrawal sequence {0} does not exist")]
    MissingSeq(u64),

    #[error("withdrawal sequence overflow at {0}")]
    SeqOverflow(u64),

    #[error("withdrawal sequence {0} is already occupied")]
    SeqOccupied(u64),

    #[error("withdrawal event index inconsistent for {0:?}: first_seq {1}, count {2}")]
    EventIndexInconsistent(DbWithdrawalEventKey, u64, u32),

    #[error("withdrawal seq {0} already paired to deposit_idx {1}, requested {2}")]
    SeqPairingConflict(u64, u32, u32),

    #[error("deposit_idx {0} already paired to withdrawal seq {1}, requested {2}")]
    DepositPairingConflict(u32, u64, u64),
}

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("create data dir {0:?}: {1}")]
    CreateDataDir(PathBuf, #[source] std::io::Error),

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
    Consistency(#[from] WithdrawalIndexConsistencyError),

    #[error(transparent)]
    Sled(#[from] typed_sled::error::Error),
}

impl From<sled::Error> for DbError {
    fn from(value: sled::Error) -> Self {
        Self::Sled(value.into())
    }
}

pub type DbResult<T> = Result<T, DbError>;
