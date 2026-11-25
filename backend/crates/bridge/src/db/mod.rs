//! Persistence for the bridge crate.

pub(crate) mod bitcoin_observer;
pub(crate) mod error;
pub(crate) mod status;
pub(crate) mod traits;
pub(crate) mod types;
pub(crate) mod withdrawal_index;

pub use bitcoin_observer::db::BitcoinObserverDbSled;
pub use status::db::BridgeStatusDbSled;
pub use withdrawal_index::db::WithdrawalIndexerDbSled;
