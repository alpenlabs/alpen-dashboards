//! EVM `WithdrawalIntentEvent` indexer: tails the bridgeout precompile, expands
//! each event into one row per withdrawal sub-unit, and writes them in FIFO
//! order via [`WithdrawalIndexerDb`].

use alloy_primitives::{address, Address};

pub(crate) mod decoder;
pub(crate) mod rpc;
pub(crate) mod task;

/// Indexer task name used as the key in the `IndexerStateSchema` tree.
pub(crate) const TASK_NAME: &str = "withdrawal_index";

/// Address of the EVM bridgeout precompile (matches alpen-reth's
/// `BRIDGEOUT_PRECOMPILE_ADDRESS`).
pub(crate) const BRIDGEOUT_PRECOMPILE_ADDRESS: Address =
    address!("5400000000000000000000000000000000000001");
