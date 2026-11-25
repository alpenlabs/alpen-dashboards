//! Retrying Bitcoin Core access for canonical block and transaction observation.

use std::{env, fmt::Debug};

use async_trait::async_trait;
use bitcoin::{
    consensus::{deserialize, encode::Error as DecodeError},
    Block, BlockHash, Transaction, Txid,
};
use bitcoind_async_client::{error::ClientError, Auth, Client};
use reqwest::Url;
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};
use status_config::{BitcoinAuth, BitcoinClientConfig};
use strata_btc_types::BlockHashExt;
use strata_identifiers::L1BlockCommitment;
use strata_retry::{retry_with_backoff_async, RetryConfig};

use super::observations::TransactionInclusion;

#[derive(Debug, thiserror::Error)]
pub enum BitcoinRpcError {
    #[error("invalid Bitcoin RPC endpoint; use HTTP(S) and separate authentication")]
    Endpoint,
    #[error("Bitcoin RPC password environment variable is unavailable")]
    Credentials,
    #[error(transparent)]
    Client(#[from] ClientError),
    #[error("invalid Bitcoin transaction or block hex: {0}")]
    Hex(#[from] hex::FromHexError),
    #[error("invalid Bitcoin transaction or block: {0}")]
    Decode(#[from] DecodeError),
    #[error("Bitcoin block hash or Merkle root does not match the requested block")]
    BlockMismatch,
    #[error("Bitcoin transaction does not match the requested TXID")]
    TransactionMismatch,
    #[error("Bitcoin chain changed during transaction lookup")]
    ChainChanged,
    #[error("Bitcoin inclusion height or transaction position overflow")]
    Overflow,
}

#[derive(Debug, Deserialize)]
pub(crate) struct BitcoinChainInfo {
    pub(crate) blocks: u64,
    pub(crate) bestblockhash: BlockHash,
    pub(crate) chain: String,
}

#[derive(Debug)]
pub(crate) struct BitcoinTransaction {
    pub(crate) tx: Transaction,
    pub(crate) inclusion: TransactionInclusion,
}

#[derive(Debug, Deserialize)]
struct RawTransaction {
    hex: String,
    blockhash: Option<BlockHash>,
}

#[derive(Debug, Deserialize)]
struct BlockHeader {
    height: u64,
}

#[async_trait]
pub(crate) trait BitcoinSource: Send + Sync {
    async fn chain_info(&self) -> Result<BitcoinChainInfo, BitcoinRpcError>;
    async fn block_hash(&self, height: u64) -> Result<BlockHash, BitcoinRpcError>;
    async fn block(&self, height: u64) -> Result<Block, BitcoinRpcError>;
    /// Looks up a mined transaction and its canonical inclusion using the node's txindex.
    async fn transaction(&self, txid: Txid) -> Result<Option<BitcoinTransaction>, BitcoinRpcError>;
}

/// Bitcoin Core client with the dashboard's bounded retry policy.
#[derive(Clone)]
pub(crate) struct BitcoinRpcClient {
    client: Client,
    retry: RetryConfig,
}

impl BitcoinRpcClient {
    pub(crate) fn new(config: &BitcoinClientConfig) -> Result<Self, BitcoinRpcError> {
        let url = Url::parse(config.url()).map_err(|_| BitcoinRpcError::Endpoint)?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(BitcoinRpcError::Endpoint);
        }
        let auth = match config.auth() {
            BitcoinAuth::Cookie { path } => Auth::CookieFile(path.clone()),
            BitcoinAuth::UserPass {
                username,
                password_env,
            } => Auth::UserPass(
                username.clone(),
                env::var(password_env).map_err(|_| BitcoinRpcError::Credentials)?,
            ),
        };
        Ok(Self {
            // Retry in one place, including Core's warmup error (-28).
            client: Client::new(
                url.into(),
                auth,
                Some(0),
                None,
                Some(config.request_timeout_s()),
            )?,
            retry: config.retry_policy(),
        })
    }

    async fn call<T: DeserializeOwned + Debug>(
        &self,
        method: &str,
        params: &[Value],
    ) -> Result<T, BitcoinRpcError> {
        let backoff = self.retry.backoff();
        let result = retry_with_backoff_async(method, self.retry.max_retries, &backoff, || async {
            match self.client.call_raw(method, params).await {
                Err(error)
                    if error.is_retriable()
                        || matches!(
                            error,
                            ClientError::Server(-28 | -342, _) | ClientError::Status(408 | 429, _)
                        ) =>
                {
                    Err(error)
                }
                result => Ok(result),
            }
        })
        .await?;
        result.map_err(Into::into)
    }
}

#[async_trait]
impl BitcoinSource for BitcoinRpcClient {
    async fn chain_info(&self) -> Result<BitcoinChainInfo, BitcoinRpcError> {
        self.call("getblockchaininfo", &[]).await
    }

    async fn block_hash(&self, height: u64) -> Result<BlockHash, BitcoinRpcError> {
        self.call("getblockhash", &[json!(height)]).await
    }

    async fn block(&self, height: u64) -> Result<Block, BitcoinRpcError> {
        let hash = self.block_hash(height).await?;
        let raw: String = self.call("getblock", &[json!(hash), json!(0)]).await?;
        let block: Block = deserialize(&hex::decode(raw)?)?;
        if block.block_hash() != hash || !block.check_merkle_root() {
            return Err(BitcoinRpcError::BlockMismatch);
        }
        Ok(block)
    }

    async fn transaction(&self, txid: Txid) -> Result<Option<BitcoinTransaction>, BitcoinRpcError> {
        let raw: RawTransaction = match self
            .call("getrawtransaction", &[json!(txid), json!(true)])
            .await
        {
            Err(BitcoinRpcError::Client(error)) if error.is_tx_not_found() => return Ok(None),
            result => result?,
        };
        let Some(hash) = raw.blockhash else {
            return Ok(None);
        };
        let tx: Transaction = deserialize(&hex::decode(raw.hex)?)?;
        if tx.compute_txid() != txid {
            return Err(BitcoinRpcError::TransactionMismatch);
        }
        let header: BlockHeader = self
            .call("getblockheader", &[json!(hash), json!(true)])
            .await?;
        // Fetch the canonical block to establish both membership and transaction order.
        let block = self.block(header.height).await?;
        if block.block_hash() != hash {
            return Err(BitcoinRpcError::ChainChanged);
        }
        let position = block
            .txdata
            .iter()
            .position(|candidate| candidate == &tx)
            .ok_or(BitcoinRpcError::TransactionMismatch)?;
        if self.block_hash(header.height).await? != hash {
            return Err(BitcoinRpcError::ChainChanged);
        }
        Ok(Some(BitcoinTransaction {
            tx,
            inclusion: TransactionInclusion {
                block: L1BlockCommitment::new(
                    u32::try_from(header.height).map_err(|_| BitcoinRpcError::Overflow)?,
                    hash.to_l1_block_id(),
                ),
                position: u32::try_from(position).map_err(|_| BitcoinRpcError::Overflow)?,
            },
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::{Arc, Mutex},
        time::{SystemTime, UNIX_EPOCH},
    };

    use axum::{
        extract::State,
        http::{HeaderMap, StatusCode},
        routing::post,
        Json, Router,
    };
    use bitcoin::{blockdata::constants::genesis_block, consensus::serialize, Network};
    use bitcoind_async_client::{error::ClientError, Auth, Client};
    use serde_json::{json, Value};
    use status_utils::test_utils::{TestHttpServer, TestRpcServer};
    use strata_retry::RetryConfig;

    use super::{BitcoinRpcClient, BitcoinRpcError, BitcoinSource};

    fn make_client(url: &str) -> BitcoinRpcClient {
        BitcoinRpcClient {
            client: Client::new(
                url.into(),
                Auth::UserPass("test".into(), "password".into()),
                Some(0),
                None,
                Some(5),
            )
            .expect("client"),
            retry: RetryConfig {
                max_retries: 1,
                base_delay_ms: 0,
                max_delay_ms: 0,
                ..Default::default()
            },
        }
    }

    fn make_chain_info() -> Value {
        json!({"chain":"regtest", "blocks":0, "bestblockhash":genesis_block(Network::Regtest).block_hash()})
    }

    #[tokio::test]
    async fn bitcoin_rpc_recovers_after_node_warmup() {
        let server = TestRpcServer::new(vec![
            json!({"error":{"code":-28,"message":"warming up"}}),
            json!({"result":make_chain_info()}),
        ])
        .await;
        let client = make_client(server.url());
        assert_eq!(client.chain_info().await.expect("recovered node").blocks, 0);
        assert_eq!(server.attempts(), 2);
    }

    #[tokio::test]
    async fn bitcoin_rpc_limits_retries_and_returns_permanent_errors_immediately() {
        for (code, attempts) in [(-28, 2), (-8, 1)] {
            let server =
                TestRpcServer::new(vec![json!({"error":{"code":code,"message":"RPC error"}})])
                    .await;
            let error = make_client(server.url())
                .block_hash(100)
                .await
                .expect_err("RPC error");
            assert!(
                matches!(error, BitcoinRpcError::Client(ClientError::Server(actual, _)) if actual == code)
            );
            assert_eq!(server.attempts(), attempts);
        }
    }

    #[tokio::test]
    async fn bitcoin_rpc_decodes_blocks_and_rejects_a_different_hash() {
        let block = genesis_block(Network::Regtest);
        for expected in [
            block.block_hash(),
            genesis_block(Network::Bitcoin).block_hash(),
        ] {
            let server = TestRpcServer::new(vec![
                json!({"result":expected}),
                json!({"result":hex::encode(serialize(&block))}),
            ])
            .await;
            let result = make_client(server.url()).block(0).await;
            if expected == block.block_hash() {
                assert_eq!(result.expect("block"), block);
            } else {
                assert!(matches!(result, Err(BitcoinRpcError::BlockMismatch)));
            }
        }
    }

    #[tokio::test]
    async fn bitcoin_rpc_recovers_transaction_inclusion_and_rechecks_canonicality() {
        let block = genesis_block(Network::Regtest);
        let tx = &block.txdata[0];
        for canonical in [true, false] {
            let hash = block.block_hash();
            let final_hash = if canonical {
                hash
            } else {
                genesis_block(Network::Bitcoin).block_hash()
            };
            let server = TestRpcServer::new(vec![
                json!({"result":{"hex":hex::encode(serialize(tx)),"blockhash":hash}}),
                json!({"result":{"height":0}}),
                json!({"result":hash}),
                json!({"result":hex::encode(serialize(&block))}),
                json!({"result":final_hash}),
            ])
            .await;
            let result = make_client(server.url())
                .transaction(tx.compute_txid())
                .await;
            if canonical {
                let recovered = result.expect("lookup").expect("mined transaction");
                assert_eq!(recovered.tx, *tx);
                assert_eq!(recovered.inclusion.block.height, 0);
                assert_eq!(recovered.inclusion.position, 0);
            } else {
                assert!(matches!(result, Err(BitcoinRpcError::ChainChanged)));
            }
        }
    }

    #[tokio::test]
    async fn bitcoin_rpc_does_not_report_unmined_or_missing_transactions() {
        let tx = genesis_block(Network::Regtest).txdata[0].clone();
        for response in [
            json!({"error":{"code":-5,"message":"not found"}}),
            json!({"result":{"hex":hex::encode(serialize(&tx))}}),
        ] {
            let server = TestRpcServer::new(vec![response]).await;
            assert!(make_client(server.url())
                .transaction(tx.compute_txid())
                .await
                .expect("lookup")
                .is_none());
        }
    }

    #[tokio::test]
    async fn bitcoin_rpc_reloads_cookie_credentials_after_unauthorized_response() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let cookie =
            std::env::temp_dir().join(format!("bitcoin_rpc_{}_{unique}", std::process::id()));
        fs::write(&cookie, "test:first").expect("initial cookie");
        let expected = Arc::new(Mutex::new("Basic dGVzdDpmaXJzdA==".to_owned()));
        async fn reply(
            State(expected): State<Arc<Mutex<String>>>,
            headers: HeaderMap,
            Json(request): Json<Value>,
        ) -> (StatusCode, Json<Value>) {
            let authorized = headers
                .get("authorization")
                .and_then(|header| header.to_str().ok())
                .is_some_and(|header| header == *expected.lock().expect("credentials"));
            if !authorized {
                return (StatusCode::UNAUTHORIZED, Json(json!(null)));
            }
            (
                StatusCode::OK,
                Json(json!({"id":request["id"],"result":make_chain_info(),"error":null})),
            )
        }
        let server = TestHttpServer::new(
            Router::new()
                .route("/", post(reply))
                .with_state(Arc::clone(&expected)),
        )
        .await;
        let config = toml::from_str(&format!(
            "url = {:?}\nauth = {{ kind = \"cookie\", path = {:?} }}",
            server.url(),
            cookie
        ))
        .expect("config");
        let client = BitcoinRpcClient::new(&config).expect("cookie client");
        client.chain_info().await.expect("initial authentication");
        fs::write(&cookie, "test:second").expect("rotated cookie");
        *expected.lock().expect("credentials") = "Basic dGVzdDpzZWNvbmQ=".into();
        client.chain_info().await.expect("refresh cookie on 401");
        fs::remove_file(cookie).expect("remove cookie");
    }
}
