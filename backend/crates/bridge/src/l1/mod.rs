//! Bitcoin L1 data access and deposit recognition.

pub(crate) mod deposits;
pub(crate) mod esplora;
pub(crate) mod observations;
pub(crate) mod observer;
pub(crate) mod params;
pub(crate) mod rpc;
#[cfg(test)]
pub(crate) mod test_utils;
