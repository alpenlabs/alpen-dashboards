use alloy_primitives::{Address, B256, Bytes};
use bitcoin_bosd::Descriptor;
use chrono::NaiveDateTime;
use serde::{Deserialize, Deserializer};
use serde::de::Error;
use sqlx::{{Type, Sqlite, Decode, Encode}, sqlite::{SqliteTypeInfo, SqliteValueRef}};
use std::{convert::TryFrom, ops::Deref, str::FromStr, fmt::Display};

/// Represents a subset of an Ethereum log entry returned by `eth_getLogs`.
///
/// Only includes fields relevant to decoding `WithdrawalIntentEvent`.
/// Some fields, e.g. `logIndex`, `transactionIndex`, `removed`, and `blockHash`
/// are deliberately omitted as they are not used in the withdrawal indexer flow.
#[derive(Debug, Deserialize, Clone)]
pub struct LogEntry {
    /// The emitting contract address (should match `BRIDGEOUT_ADDRESS`)
    #[serde(deserialize_with = "from_hex_address")]
    pub address: Address,
    /// ABI-encoded event data
    #[serde(deserialize_with = "from_hex_bytes")]
    pub data: Bytes,
    /// Indexed event parameters; first topic must match `WITHDRAWAL_EVENT_SIG`
    #[serde(deserialize_with = "from_hex_b256_vec")]
    pub topics: Vec<B256>,

    /// Block number containing the log (used for indexing progress)
    #[serde(rename = "blockNumber", deserialize_with = "from_hex_u64")]
    pub block_number: u64,

    /// EVM transaction hash (i.e. the withdrawal request txid)
    #[serde(rename = "transactionHash", deserialize_with = "from_hex_b256")]
    pub transaction_hash: B256,
}

fn from_hex_u64<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let hex_str: String = Deserialize::deserialize(deserializer)?;
    u64::from_str_radix(hex_str.trim_start_matches("0x"), 16)
        .map_err(serde::de::Error::custom)
}

fn from_hex_address<'de, D>(deserializer: D) -> Result<Address, D::Error>
where
    D: Deserializer<'de>,
{
    let hex: String = Deserialize::deserialize(deserializer)?;
    let bytes = hex::decode(hex.trim_start_matches("0x"))
        .map_err(|e| D::Error::custom(format!("invalid hex: {e}")))?;
    Address::try_from(bytes.as_slice()).map_err(|e| D::Error::custom(format!("invalid address: {e}")))
}

/// Parses a hex string into `B256`
fn from_hex_b256<'de, D>(deserializer: D) -> Result<B256, D::Error>
where
    D: Deserializer<'de>,
{
    let hex: String = Deserialize::deserialize(deserializer)?;
    let bytes = hex::decode(hex.trim_start_matches("0x"))
        .map_err(|e| D::Error::custom(format!("invalid hex: {e}")))?;
    Ok(B256::from_slice(&bytes))
}

fn from_hex_bytes<'de, D>(deserializer: D) -> Result<Bytes, D::Error>
where
    D: Deserializer<'de>,
{
    let hex: String = Deserialize::deserialize(deserializer)?;
    let bytes = hex::decode(hex.trim_start_matches("0x"))
        .map_err(|e| D::Error::custom(format!("invalid hex: {e}")))?;
    Ok(Bytes::from(bytes))
}

fn from_hex_b256_vec<'de, D>(deserializer: D) -> Result<Vec<B256>, D::Error>
where
    D: Deserializer<'de>,
{
    let hex_vec: Vec<String> = Deserialize::deserialize(deserializer)?;
    hex_vec.into_iter().map(|s| from_hex_b256(serde::de::IntoDeserializer::into_deserializer(s)))
        .collect()
}

/// A decoded withdrawal intent extracted from a `LogEntry`.
///
/// This struct represents an intermediate representation used *after*
/// parsing the raw JSON logs (`LogEntry`).
#[derive(Debug, Clone)]
pub struct DecodedWithdrawalIntent {
    /// EVM transaction hash (i.e. the withdrawal request txid)
    pub txid: B256,
    /// Amount in satoshis
    pub amount: u64,
    /// Dynamic-sized bytes BOSD descriptor for the withdrawal destinations in L1.
    pub destination: Descriptor,
    /// Alpen block number where the event appeared
    pub block_number: u64,
}

/// Txid stored as a 32-byte BLOB in SQLite.
#[derive(Debug, Clone, PartialEq, Eq, Type)]
#[sqlx(transparent)]
pub(crate) struct DbTxid(Vec<u8>); // <- must be Vec<u8> or [u8;32] because
                                   //    those already implement Type+Encode+Decode

impl Deref for DbTxid {
    type Target = [u8];
    fn deref(&self) -> &Self::Target { &self.0 }
}

impl From<B256> for DbTxid {
    fn from(hash: B256) -> Self {
        Self(hash.to_vec())
    }
}

impl TryFrom<DbTxid> for B256 {
    type Error = anyhow::Error;

    fn try_from(txid: DbTxid) -> anyhow::Result<Self> {
        let bytes: [u8; 32] = txid
            .0
            .as_slice()
            .try_into()
            .map_err(|_| anyhow::anyhow!("txid must be 32 bytes, got {}", txid.0.len()))?;

        Ok(B256::from(bytes))
    }
}

/// ───── Amount in satoshis ──────────────────────────────────────────────
/// SQLite’s INTEGER is signed i64, so wrap that.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Type)]
#[sqlx(transparent)]
pub(crate) struct DbAmount(i64);

impl Deref for DbAmount {
    type Target = i64;
    fn deref(&self) -> &Self::Target { &self.0 }
}

impl From<u64> for DbAmount {
    fn from(v: u64) -> Self { Self(v as i64) }
}
impl From<DbAmount> for u64 {
    fn from(v: DbAmount) -> Self { v.0 as u64 }
}

/// ───── Destination descriptor (BOSD) ───────────────────────────────────
/// Kept as its canonical string form.
#[derive(Debug, Clone, PartialEq, Eq, Type)]
#[sqlx(transparent)]
pub(crate) struct DbDescriptor(String);

impl Deref for DbDescriptor {
    type Target = str;
    fn deref(&self) -> &Self::Target { &self.0 }
}

impl From<Descriptor> for DbDescriptor {
    fn from(d: Descriptor) -> Self { Self(d.to_string()) }
}
impl TryFrom<DbDescriptor> for Descriptor {
    type Error = anyhow::Error;
    fn try_from(db: DbDescriptor) -> anyhow::Result<Self> {
        Ok(db.0.parse()?)
    }
}

/// ───── Block number ────────────────────────────────────────────────────
#[derive(Debug, Copy, Clone, PartialEq, Eq, Type)]
#[sqlx(transparent)]
pub(crate) struct DbBlockNumber(i64);

impl Deref for DbBlockNumber {
    type Target = i64;
    fn deref(&self) -> &Self::Target { &self.0 }
}

impl TryFrom<u64> for DbBlockNumber {
    type Error = std::num::TryFromIntError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        i64::try_from(value).map(Self)
    }
}

impl From<i64> for DbBlockNumber {
    fn from(value: i64) -> Self {
        Self(value)
    }
}

impl From<DbBlockNumber> for u64 {
    fn from(value: DbBlockNumber) -> Self {
        value.0 as u64
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IndexerTaskId {
    WithdrawalRequests,
    // Add other task types here
}

impl Display for IndexerTaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            IndexerTaskId::WithdrawalRequests => "withdrawal_requests",
        };
        write!(f, "{}", s)
    }
}

impl FromStr for IndexerTaskId {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "withdrawal_requests" => Ok(IndexerTaskId::WithdrawalRequests),
            _ => Err(anyhow::anyhow!("Unknown task id: {}", s)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Type)]
#[sqlx(transparent)]
pub(crate) struct DbTaskId(pub(crate) IndexerTaskId);

impl Deref for DbTaskId {
    type Target = IndexerTaskId;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<IndexerTaskId> for DbTaskId {
    fn from(task_id: IndexerTaskId) -> Self {
        DbTaskId(task_id)
    }
}

impl Type<Sqlite> for DbTaskId {
    fn type_info() -> SqliteTypeInfo {
        <String as Type<Sqlite>>::type_info()
    }
}

impl<'r> Decode<'r, Sqlite> for DbTaskId {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let s: String = Decode::<'r, Sqlite>::decode(value)?;
        let inner = IndexerTaskId::from_str(&s)?;
        Ok(DbTaskId(inner))
    }
}

impl<'q> Encode<'q, Sqlite> for DbTaskId {
    fn encode_by_ref(
        &self,
        buf: &mut <Sqlite as sqlx::Database>::ArgumentBuffer<'q>,
    ) -> Result<sqlx::encode::IsNull, Box<dyn std::error::Error + Send + Sync>> {
        Encode::<Sqlite>::encode_by_ref(&self.0.to_string(), buf)
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Type)]
#[sqlx(transparent)]
pub(crate) struct DbTimestamp(NaiveDateTime);

impl Deref for DbTimestamp {
    type Target = NaiveDateTime;
    fn deref(&self) -> &Self::Target { &self.0 }
}

impl From<NaiveDateTime> for DbTimestamp {
    fn from(dt: NaiveDateTime) -> Self { Self(dt) }
}

impl From<DbTimestamp> for NaiveDateTime {
    fn from(db: DbTimestamp) -> Self { db.0 }
}
