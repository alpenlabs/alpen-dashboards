//! EVM JSON-RPC client used by the withdrawal indexer.

use alloy_primitives::{Address, B256};
use jsonrpsee::{
    core::client::{ClientT, Error as JsonRpcError},
    http_client::{HttpClient, HttpClientBuilder},
    rpc_params,
};
use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub(crate) enum EthRpcError {
    #[error("eth rpc transport: {0}")]
    Transport(#[from] JsonRpcError),

    #[error("invalid hex in {field}: {value}")]
    InvalidHex { field: &'static str, value: String },
}

/// Minimal log shape returned by `eth_getLogs`.
///
/// Only the fields the indexer reads are kept; deserialization tolerates the
/// extra fields RPC servers include (`removed`, `transactionIndex`, etc.).
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct RpcLog {
    pub(crate) address: Address,

    pub(crate) topics: Vec<B256>,

    #[serde(deserialize_with = "deserialize_hex_bytes")]
    pub(crate) data: Vec<u8>,

    #[serde(rename = "blockNumber", deserialize_with = "deserialize_hex_u64")]
    pub(crate) block_number: u64,

    #[serde(rename = "transactionHash")]
    pub(crate) transaction_hash: B256,

    #[serde(rename = "logIndex", deserialize_with = "deserialize_hex_u64")]
    pub(crate) log_index: u64,
}

/// Operations the indexer calls on the EVM JSON-RPC endpoint.
pub(crate) trait EthLogsClient: Send + Sync {
    async fn block_number(&self) -> Result<u64, EthRpcError>;

    async fn get_logs(
        &self,
        from_block: u64,
        to_block: u64,
        address: Address,
        topic0: B256,
    ) -> Result<Vec<RpcLog>, EthRpcError>;
}

/// jsonrpsee-backed [`EthLogsClient`] talking to alpen-reth.
#[derive(Debug)]
pub(crate) struct JsonRpcEthClient {
    inner: HttpClient,
}

impl JsonRpcEthClient {
    pub(crate) fn new(url: &str) -> Result<Self, EthRpcError> {
        let inner = HttpClientBuilder::default().build(url)?;
        Ok(Self { inner })
    }
}

impl EthLogsClient for JsonRpcEthClient {
    async fn block_number(&self) -> Result<u64, EthRpcError> {
        let raw: String = self.inner.request("eth_blockNumber", rpc_params![]).await?;
        parse_hex_u64("eth_blockNumber", &raw)
    }

    async fn get_logs(
        &self,
        from_block: u64,
        to_block: u64,
        address: Address,
        topic0: B256,
    ) -> Result<Vec<RpcLog>, EthRpcError> {
        let filter = serde_json::json!({
            "fromBlock": format!("0x{:x}", from_block),
            "toBlock":   format!("0x{:x}", to_block),
            "address":   address,
            "topics":    [topic0],
        });
        let logs: Vec<RpcLog> = self
            .inner
            .request("eth_getLogs", rpc_params![filter])
            .await?;
        Ok(logs)
    }
}

fn parse_hex_u64(field: &'static str, raw: &str) -> Result<u64, EthRpcError> {
    let stripped = raw.strip_prefix("0x").unwrap_or(raw);
    u64::from_str_radix(stripped, 16).map_err(|_| EthRpcError::InvalidHex {
        field,
        value: raw.to_owned(),
    })
}

fn deserialize_hex_u64<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = String::deserialize(deserializer)?;
    let stripped = raw.strip_prefix("0x").unwrap_or(&raw);
    u64::from_str_radix(stripped, 16).map_err(serde::de::Error::custom)
}

fn deserialize_hex_bytes<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = String::deserialize(deserializer)?;
    let stripped = raw.strip_prefix("0x").unwrap_or(&raw);
    if stripped.is_empty() {
        return Ok(Vec::new());
    }
    hex::decode(stripped).map_err(serde::de::Error::custom)
}
