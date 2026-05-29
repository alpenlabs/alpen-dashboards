//! Schema and codec definitions for the withdrawal-indexer trees.

use typed_sled::{
    codec::{CodecError, KeyCodec, ValueCodec},
    schema::TreeName,
    Schema,
};

use crate::db::types::{
    DbIndexerState, DbWithdrawalEventIndex, DbWithdrawalEventKey, DbWithdrawalRequest,
};

/// Indexer checkpoint, keyed by task name.
#[derive(Debug)]
pub(crate) struct IndexerStateSchema;

impl Schema for IndexerStateSchema {
    const TREE_NAME: TreeName = TreeName("indexer_state");
    type Key = String;
    type Value = DbIndexerState;
}

/// FIFO sequence of expanded withdrawal requests, keyed by sequence number.
#[derive(Debug)]
pub(crate) struct WithdrawalRequestSchema;

impl Schema for WithdrawalRequestSchema {
    const TREE_NAME: TreeName = TreeName("withdrawal_request");
    type Key = u64;
    type Value = DbWithdrawalRequest;
}

/// Reverse index for idempotent insertion: event key → persisted sequence range.
#[derive(Debug)]
pub(crate) struct WithdrawalEventIndexSchema;

impl Schema for WithdrawalEventIndexSchema {
    const TREE_NAME: TreeName = TreeName("withdrawal_event_index");
    type Key = DbWithdrawalEventKey;
    type Value = DbWithdrawalEventIndex;
}

// ---- Key codecs ----

impl KeyCodec<IndexerStateSchema> for String {
    fn encode_key(&self) -> Result<Vec<u8>, CodecError> {
        Ok(self.as_bytes().to_vec())
    }

    fn decode_key(buf: &[u8]) -> Result<Self, CodecError> {
        std::str::from_utf8(buf).map(|s| s.to_owned()).map_err(|e| {
            CodecError::DeserializationFailed {
                schema: IndexerStateSchema::TREE_NAME.0,
                source: e.into(),
            }
        })
    }
}

impl KeyCodec<WithdrawalEventIndexSchema> for DbWithdrawalEventKey {
    fn encode_key(&self) -> Result<Vec<u8>, CodecError> {
        let mut buf = Vec::with_capacity(32 + 8);
        buf.extend_from_slice(&self.tx_hash.0);
        buf.extend_from_slice(&self.log_index.to_be_bytes());
        Ok(buf)
    }

    fn decode_key(buf: &[u8]) -> Result<Self, CodecError> {
        const EXPECTED: usize = 32 + 8;
        if buf.len() != EXPECTED {
            return Err(CodecError::InvalidKeyLength {
                schema: WithdrawalEventIndexSchema::TREE_NAME.0,
                expected: EXPECTED,
                actual: buf.len(),
            });
        }
        let mut hash_bytes = [0u8; 32];
        hash_bytes.copy_from_slice(&buf[..32]);
        let mut log_bytes = [0u8; 8];
        log_bytes.copy_from_slice(&buf[32..]);
        Ok(DbWithdrawalEventKey {
            tx_hash: strata_primitives::buf::Buf32(hash_bytes),
            log_index: u64::from_be_bytes(log_bytes),
        })
    }
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

impl_json_value_codec!(IndexerStateSchema, DbIndexerState);
impl_json_value_codec!(WithdrawalRequestSchema, DbWithdrawalRequest);
impl_json_value_codec!(WithdrawalEventIndexSchema, DbWithdrawalEventIndex);
