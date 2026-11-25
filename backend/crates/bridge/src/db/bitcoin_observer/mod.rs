//! Persistence for observed Bitcoin deposits and scan progress.

pub(crate) mod db;
#[cfg(test)]
pub(crate) mod mock;
pub(crate) mod schema;

#[cfg(test)]
pub(crate) mod test_utils;
