//! Bitcoin L1 data access and deposit recognition.

// The observer commit adds the production consumers of these parsers.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) mod deposits;
pub(crate) mod esplora;
#[cfg(test)]
pub(crate) mod test_utils;
