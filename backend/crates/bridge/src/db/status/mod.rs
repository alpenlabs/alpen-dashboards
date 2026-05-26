//! Persistence for bridge-status rows, pairings, and cursors.

pub(crate) mod db;
pub(crate) mod schema;

#[cfg(test)]
pub(crate) mod mock;
