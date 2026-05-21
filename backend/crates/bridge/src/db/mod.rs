//! Persistence for the bridge crate.

pub(crate) mod error;
pub(crate) mod traits;
pub(crate) mod types;
pub(crate) mod withdrawal_index;

pub use withdrawal_index::db::WithdrawalIndexerDbSled;
