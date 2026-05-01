//! Withdrawal-intent log decoding.

use alloy_sol_types::SolEvent;
use alpen_reth_primitives::WithdrawalIntentEvent;
use strata_primitives::buf::Buf32;

use crate::db::types::DbWithdrawalRequest;

use super::{rpc::RpcLog, BRIDGEOUT_PRECOMPILE_ADDRESS};

/// Hard correctness bound on sub-units per `WithdrawalIntentEvent`, set by
/// `DbWithdrawalRequest::sub_idx: u32`: any value above this can't be stored
/// without overflowing the cast. A tighter operational ceiling would need a
/// protocol-defined constant or operator-tunable config — neither alpen nor
/// strata-bridge expose one, so we don't invent one dashboard-side.
const MAX_SUB_UNITS_PER_LOG: u64 = u32::MAX as u64;

#[derive(Debug, thiserror::Error)]
pub(crate) enum DecodeError {
    #[error("unexpected log address (expected {expected}, got {actual})")]
    UnexpectedAddress {
        expected: alloy_primitives::Address,
        actual: alloy_primitives::Address,
    },

    #[error("unexpected log topic[0] {0:?}")]
    UnexpectedSignature(alloy_primitives::B256),

    #[error("alloy decode: {0}")]
    AbiDecode(#[from] alloy_sol_types::Error),

    #[error(
        "withdrawal amount {amount} sats is not a positive multiple of {denomination_sats} sats"
    )]
    AmountNotMultiple { amount: u64, denomination_sats: u64 },

    #[error("withdrawal amount {amount} sats expands to more than {max_sub_units} sub-units")]
    AmountTooLarge { amount: u64, max_sub_units: u64 },
}

/// Decode `log` into N rows, where N = `amount / withdrawal_denomination_sats`.
///
/// Each row carries the same `(tx_hash, log_index)` and a distinct
/// `sub_idx ∈ 0..N`. Returns an error if the log doesn't belong to the
/// bridgeout precompile, the topic doesn't match the event signature, or the
/// amount isn't a positive exact multiple of the denomination.
pub(crate) fn decode(
    log: &RpcLog,
    withdrawal_denomination_sats: u64,
) -> Result<Vec<DbWithdrawalRequest>, DecodeError> {
    if log.address != BRIDGEOUT_PRECOMPILE_ADDRESS {
        return Err(DecodeError::UnexpectedAddress {
            expected: BRIDGEOUT_PRECOMPILE_ADDRESS,
            actual: log.address,
        });
    }
    match log.topics.first() {
        Some(t0) if *t0 == WithdrawalIntentEvent::SIGNATURE_HASH => {}
        Some(t0) => return Err(DecodeError::UnexpectedSignature(*t0)),
        None => {
            return Err(DecodeError::UnexpectedSignature(
                alloy_primitives::B256::ZERO,
            ))
        }
    }

    let evt = WithdrawalIntentEvent::decode_raw_log(log.topics.iter().copied(), &log.data)?;

    if evt.amount == 0 || evt.amount % withdrawal_denomination_sats != 0 {
        return Err(DecodeError::AmountNotMultiple {
            amount: evt.amount,
            denomination_sats: withdrawal_denomination_sats,
        });
    }
    let n = evt.amount / withdrawal_denomination_sats;
    if n > MAX_SUB_UNITS_PER_LOG {
        return Err(DecodeError::AmountTooLarge {
            amount: evt.amount,
            max_sub_units: MAX_SUB_UNITS_PER_LOG,
        });
    }

    let tx_hash = Buf32(log.transaction_hash.0);
    let destination = evt.destination.to_vec();
    let block_number = log.block_number;
    let log_index = log.log_index;
    let selected_operator = evt.selectedOperator;

    let mut out = Vec::with_capacity(n as usize);
    for sub_idx in 0..n {
        out.push(DbWithdrawalRequest {
            tx_hash,
            log_index,
            sub_idx: sub_idx as u32,
            amount_sats: withdrawal_denomination_sats,
            destination: destination.clone(),
            selected_operator,
            block_number,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{Bytes, LogData, B256};

    const TEST_WITHDRAWAL_DENOMINATION_SATS: u64 = 100_000_000;

    fn make_log(
        amount_sats: u64,
        selected_operator: u32,
        address: alloy_primitives::Address,
    ) -> RpcLog {
        let evt = WithdrawalIntentEvent {
            amount: amount_sats,
            selectedOperator: selected_operator,
            destination: Bytes::from(vec![0xAB; 22]),
        };
        let data = LogData::from(&evt);
        RpcLog {
            address,
            topics: data.topics().to_vec(),
            data: data.data.to_vec(),
            block_number: 1234,
            transaction_hash: B256::repeat_byte(0x77),
            log_index: 5,
        }
    }

    #[test]
    fn expands_into_sub_units() {
        let log = make_log(
            3 * TEST_WITHDRAWAL_DENOMINATION_SATS,
            2,
            BRIDGEOUT_PRECOMPILE_ADDRESS,
        );
        let rows = decode(&log, TEST_WITHDRAWAL_DENOMINATION_SATS).expect("decode");
        assert_eq!(rows.len(), 3);
        for (i, row) in rows.iter().enumerate() {
            assert_eq!(row.sub_idx, i as u32);
            assert_eq!(row.tx_hash.0, [0x77; 32]);
            assert_eq!(row.log_index, 5);
            assert_eq!(row.amount_sats, TEST_WITHDRAWAL_DENOMINATION_SATS);
            assert_eq!(row.selected_operator, 2);
            assert_eq!(row.destination, vec![0xAB; 22]);
            assert_eq!(row.block_number, 1234);
        }
    }

    #[test]
    fn single_sub_unit_for_one_denom() {
        let log = make_log(
            TEST_WITHDRAWAL_DENOMINATION_SATS,
            u32::MAX,
            BRIDGEOUT_PRECOMPILE_ADDRESS,
        );
        let rows = decode(&log, TEST_WITHDRAWAL_DENOMINATION_SATS).expect("decode");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].selected_operator, u32::MAX);
    }

    #[test]
    fn rejects_non_multiple_amount() {
        let log = make_log(
            TEST_WITHDRAWAL_DENOMINATION_SATS + 1,
            0,
            BRIDGEOUT_PRECOMPILE_ADDRESS,
        );
        assert!(matches!(
            decode(&log, TEST_WITHDRAWAL_DENOMINATION_SATS),
            Err(DecodeError::AmountNotMultiple { .. })
        ));
    }

    #[test]
    fn rejects_zero_amount() {
        let log = make_log(0, 0, BRIDGEOUT_PRECOMPILE_ADDRESS);
        assert!(matches!(
            decode(&log, TEST_WITHDRAWAL_DENOMINATION_SATS),
            Err(DecodeError::AmountNotMultiple { .. })
        ));
    }

    #[test]
    fn rejects_amount_exceeding_sub_idx_capacity() {
        // n = u32::MAX + 1 → would overflow `sub_idx: u32`. Decoder must
        // reject before allocating.
        let log = make_log(
            (MAX_SUB_UNITS_PER_LOG + 1) * TEST_WITHDRAWAL_DENOMINATION_SATS,
            0,
            BRIDGEOUT_PRECOMPILE_ADDRESS,
        );
        assert!(matches!(
            decode(&log, TEST_WITHDRAWAL_DENOMINATION_SATS),
            Err(DecodeError::AmountTooLarge { .. })
        ));
    }

    #[test]
    fn rejects_wrong_address() {
        let other = alloy_primitives::address!("0000000000000000000000000000000000000099");
        let log = make_log(TEST_WITHDRAWAL_DENOMINATION_SATS, 0, other);
        assert!(matches!(
            decode(&log, TEST_WITHDRAWAL_DENOMINATION_SATS),
            Err(DecodeError::UnexpectedAddress { .. })
        ));
    }

    #[test]
    fn rejects_wrong_topic() {
        let mut log = make_log(
            TEST_WITHDRAWAL_DENOMINATION_SATS,
            0,
            BRIDGEOUT_PRECOMPILE_ADDRESS,
        );
        log.topics[0] = B256::ZERO;
        assert!(matches!(
            decode(&log, TEST_WITHDRAWAL_DENOMINATION_SATS),
            Err(DecodeError::UnexpectedSignature(..))
        ));
    }
}
