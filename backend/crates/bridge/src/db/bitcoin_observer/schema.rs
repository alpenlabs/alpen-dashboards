//! Schema and codec definitions for Bitcoin observations and rollback history.

use bitcoin::{
    consensus::{deserialize, serialize},
    OutPoint,
};
use serde::{Deserialize, Serialize};
use strata_identifiers::L1BlockCommitment;
use typed_sled::{
    codec::{CodecError, KeyCodec, ValueCodec},
    schema::TreeName,
    Schema,
};

use crate::{db::types::BitcoinObserverState, l1::observations::ObservedDeposit};

#[derive(Debug)]
pub(crate) struct ObserverStateSchema;
impl Schema for ObserverStateSchema {
    const TREE_NAME: TreeName = TreeName("bitcoin_observer_state");
    type Key = u8;
    type Value = BitcoinObserverState;
}

#[derive(Debug)]
pub(crate) struct DepositSchema;
impl Schema for DepositSchema {
    const TREE_NAME: TreeName = TreeName("deposit_observation");
    type Key = OutPoint;
    type Value = ObservedDeposit;
}

#[derive(Debug)]
pub(crate) struct UndoSchema;
impl Schema for UndoSchema {
    const TREE_NAME: TreeName = TreeName("block_undo");
    type Key = u32;
    type Value = UndoBlock;
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct UndoBlock {
    pub(crate) parent: Option<L1BlockCommitment>,
    pub(crate) block: L1BlockCommitment,
    pub(crate) rows: Vec<(OutPoint, Option<ObservedDeposit>)>,
}

impl KeyCodec<DepositSchema> for OutPoint {
    fn encode_key(&self) -> Result<Vec<u8>, CodecError> {
        Ok(serialize(self))
    }
    fn decode_key(bytes: &[u8]) -> Result<Self, CodecError> {
        deserialize(bytes).map_err(|error| CodecError::DeserializationFailed {
            schema: DepositSchema::TREE_NAME.0,
            source: error.into(),
        })
    }
}

macro_rules! impl_json_value_codec {
    ($schema:ty, $value:ty) => {
        impl ValueCodec<$schema> for $value {
            type Decoded = Self;

            fn encode_value(&self) -> Result<Vec<u8>, CodecError> {
                serde_json::to_vec(self).map_err(|error| CodecError::SerializationFailed {
                    schema: <$schema>::TREE_NAME.0,
                    source: error.into(),
                })
            }

            fn decode_value(bytes: sled::IVec) -> Result<Self, CodecError> {
                serde_json::from_slice(bytes.as_ref()).map_err(|error| {
                    CodecError::DeserializationFailed {
                        schema: <$schema>::TREE_NAME.0,
                        source: error.into(),
                    }
                })
            }
        }
    };
}

impl_json_value_codec!(ObserverStateSchema, BitcoinObserverState);
impl_json_value_codec!(DepositSchema, ObservedDeposit);
impl_json_value_codec!(UndoSchema, UndoBlock);

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::db::bitcoin_observer::test_utils::{make_block, make_metadata};

    #[test]
    fn observer_state_decodes_without_a_revision() {
        let metadata = make_metadata();
        let tip = make_block(4, 0);
        // Existing JSON records contain progress and metadata, but no revision field.
        let bytes = serde_json::to_vec(&json!({
            "metadata": metadata,
            "last_processed_block": tip,
        }))
        .expect("encode previous state layout");
        let state =
            <BitcoinObserverState as ValueCodec<ObserverStateSchema>>::decode_value(bytes.into())
                .expect("decode previous state layout");
        assert_eq!(state, BitcoinObserverState::new(metadata, Some(tip)));
    }
}
