use crate::db::types::DbWithdrawalEventKey;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum WithdrawalIndexConsistencyError {
    #[error("withdrawal sequence overflow at {0}")]
    SeqOverflow(u64),

    #[error("withdrawal sequence {0} is already occupied")]
    SeqOccupied(u64),

    #[error("withdrawal event index inconsistent for {0:?}: first_seq {1}, count {2}")]
    EventIndexInconsistent(DbWithdrawalEventKey, u64, u32),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum DbError {
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

pub(crate) type DbResult<T> = Result<T, DbError>;
