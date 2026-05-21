//! Persistence for the EVM withdrawal-intent indexer: FIFO sequencing of
//! withdrawal-request events and indexer scan state.

pub(crate) mod db;
pub(crate) mod schema;

#[cfg(test)]
pub(crate) mod mock;
#[cfg(test)]
pub(crate) mod test_utils;
