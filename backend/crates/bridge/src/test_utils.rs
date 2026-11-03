//! Shared constants, config and row factories for bridge tests.

use bitcoin::{hashes::Hash, Txid};
pub(crate) use status_config::test_utils::TEST_MAX_CONFIRMATIONS;
use status_config::test_utils::{make_bridge_config, LOCAL_ESPLORA_URL};
use strata_retry::RetryConfig;

use crate::{
    db::withdrawal_index::test_utils::make_withdrawal_request,
    types::{DepositInfo, DepositStatus, WithdrawalInfo, WithdrawalStatus},
};

pub(crate) fn make_retry_config() -> RetryConfig {
    make_bridge_config(LOCAL_ESPLORA_URL, &[], Some(3)).retry_policy()
}

pub(crate) fn make_deposit_info(status: DepositStatus) -> DepositInfo {
    DepositInfo {
        deposit_request_txid: Txid::from_byte_array([1; 32]),
        deposit_txid: matches!(status, DepositStatus::Complete)
            .then_some(Txid::from_byte_array([2; 32])),
        status,
    }
}

pub(crate) fn make_withdrawal_info(seed: u8, status: WithdrawalStatus) -> WithdrawalInfo {
    WithdrawalInfo {
        withdrawal_request_txid: make_withdrawal_request(seed).tx_hash,
        fulfillment_txid: matches!(status, WithdrawalStatus::Complete)
            .then_some(Txid::from_byte_array([seed.wrapping_add(1); 32])),
        status,
    }
}
