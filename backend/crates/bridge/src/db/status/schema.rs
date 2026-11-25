//! Schema and codec definitions for the bridge-status DB trees.

use bitcoin::{
    consensus::{deserialize, serialize},
    OutPoint,
};
use typed_sled::{
    codec::{CodecError, KeyCodec, ValueCodec},
    schema::TreeName,
    Schema,
};

use strata_bridge_primitives::types::DepositIdx;

use crate::db::types::DepositAnswer;
use crate::types::{
    ReimbursementStatusCursor, WithdrawalInfo, WithdrawalPairingCursor, WithdrawalSeq,
    WithdrawalStatusCursor,
};

/// Last successful bridge answers keyed by request outpoint.
#[derive(Debug)]
pub(crate) struct DepositAnswerSchema;

impl Schema for DepositAnswerSchema {
    const TREE_NAME: TreeName = TreeName("deposit_answer");
    type Key = OutPoint;
    type Value = DepositAnswer;
}

impl KeyCodec<DepositAnswerSchema> for OutPoint {
    fn encode_key(&self) -> Result<Vec<u8>, CodecError> {
        Ok(serialize(self))
    }

    fn decode_key(bytes: &[u8]) -> Result<Self, CodecError> {
        deserialize(bytes).map_err(|error| CodecError::DeserializationFailed {
            schema: DepositAnswerSchema::TREE_NAME.0,
            source: error.into(),
        })
    }
}

/// Withdrawal status rows keyed by bridge deposit index.
#[derive(Debug)]
pub(crate) struct WithdrawalInfoSchema;

impl Schema for WithdrawalInfoSchema {
    const TREE_NAME: TreeName = TreeName("withdrawal_info");
    type Key = DepositIdx;
    type Value = WithdrawalInfo;
}

/// Withdrawal-to-deposit pairing rows keyed by bridge deposit index.
#[derive(Debug)]
pub(crate) struct WithdrawalPairingSchema;

impl Schema for WithdrawalPairingSchema {
    const TREE_NAME: TreeName = TreeName("withdrawal_pairing");
    type Key = DepositIdx;
    type Value = WithdrawalSeq;
}

/// Deposit-info cursor cell.
#[derive(Debug)]
pub(crate) struct DepositInfoCursorSchema;

impl Schema for DepositInfoCursorSchema {
    const TREE_NAME: TreeName = TreeName("deposit_info_cursor");
    type Key = u8;
    type Value = DepositIdx;
}

/// Withdrawal-pairing cursor cell.
#[derive(Debug)]
pub(crate) struct WithdrawalPairingCursorSchema;

impl Schema for WithdrawalPairingCursorSchema {
    const TREE_NAME: TreeName = TreeName("withdrawal_pairing_cursor");
    type Key = u8;
    type Value = WithdrawalPairingCursor;
}

/// Withdrawal-status cursor cell.
#[derive(Debug)]
pub(crate) struct WithdrawalStatusCursorSchema;

impl Schema for WithdrawalStatusCursorSchema {
    const TREE_NAME: TreeName = TreeName("withdrawal_status_cursor");
    type Key = u8;
    type Value = WithdrawalStatusCursor;
}

/// Reimbursement-status cursor cell.
#[derive(Debug)]
pub(crate) struct ReimbursementStatusCursorSchema;

impl Schema for ReimbursementStatusCursorSchema {
    const TREE_NAME: TreeName = TreeName("reimbursement_status_cursor");
    type Key = u8;
    type Value = ReimbursementStatusCursor;
}

// ---- Value codecs ----

macro_rules! impl_json_value_codec {
    ($schema:ty, $value:ty) => {
        impl ValueCodec<$schema> for $value {
            type Decoded = Self;

            fn encode_value(&self) -> Result<Vec<u8>, CodecError> {
                serde_json::to_vec(self).map_err(|e| CodecError::SerializationFailed {
                    schema: <$schema>::TREE_NAME.0,
                    source: e.into(),
                })
            }

            fn decode_value(data: sled::IVec) -> Result<Self::Decoded, CodecError> {
                serde_json::from_slice(data.as_ref()).map_err(|e| {
                    CodecError::DeserializationFailed {
                        schema: <$schema>::TREE_NAME.0,
                        source: e.into(),
                    }
                })
            }
        }
    };
}

impl_json_value_codec!(DepositAnswerSchema, DepositAnswer);
impl_json_value_codec!(WithdrawalInfoSchema, WithdrawalInfo);
impl_json_value_codec!(WithdrawalPairingSchema, WithdrawalSeq);
impl_json_value_codec!(DepositInfoCursorSchema, DepositIdx);
impl_json_value_codec!(WithdrawalPairingCursorSchema, WithdrawalPairingCursor);
impl_json_value_codec!(WithdrawalStatusCursorSchema, WithdrawalStatusCursor);
impl_json_value_codec!(ReimbursementStatusCursorSchema, ReimbursementStatusCursor);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deposit_answers_use_storage_names() {
        for (answer, bytes) in [
            (DepositAnswer::InProgress, br#""InProgress""#.as_slice()),
            (DepositAnswer::Failed, br#""Failed""#.as_slice()),
            (DepositAnswer::Complete, br#""Complete""#.as_slice()),
        ] {
            assert_eq!(answer.encode_value().expect("encode storage answer"), bytes);
            assert_eq!(
                DepositAnswer::decode_value(bytes.into()).expect("decode stored answer"),
                answer,
            );
        }
    }
}
