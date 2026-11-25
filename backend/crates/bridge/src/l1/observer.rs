//! Canonical Bitcoin scanning shared by startup, catch-up and live observation.

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use bitcoin::{
    blockdata::constants::genesis_block, hashes::Hash, Block, BlockHash, Network, OutPoint,
    Transaction,
};
use status_config::{BitcoinConfig, BitcoinObserverConfig};
use strata_btc_types::{BlockHashExt, L1BlockIdBitcoinExt};
use strata_identifiers::L1BlockCommitment;
use strata_tasks::ShutdownGuard;
use tokio::sync::watch;
use tracing::{debug, info, warn};

use super::{
    deposits::DepositSpend,
    observations::{ObservedDeposit, ObservedDepositSpend, TransactionInclusion},
    params::BitcoinParams,
    rpc::{BitcoinRpcClient, BitcoinRpcError, BitcoinSource},
};
use crate::db::{
    error::{BitcoinObserverConsistencyError, DbError},
    traits::BitcoinObserverDb,
    types::BitcoinObserverDbMetadata,
};

#[derive(Debug, thiserror::Error)]
pub enum BitcoinObserverError {
    #[error(transparent)]
    Rpc(#[from] BitcoinRpcError),
    #[error(transparent)]
    Db(#[from] DbError),
    #[error("Bitcoin RPC node is on a different network than the parameter file")]
    NetworkMismatch,
    #[error(
        "Bitcoin reorganization exceeds the available undo history; an explicit rescan is required"
    )]
    DeepReorg,
    #[error("Bitcoin chain changed during scanning; retry reconciliation")]
    ChainChanged,
    #[error("Bitcoin scan height or transaction position overflow")]
    Overflow,
    #[error("stored deposit no longer validates under its historical parameters")]
    InvalidStoredRequest,
    #[error("deposit request {0} is not yet available on the canonical chain")]
    RequestUnavailable(OutPoint),
}

impl From<BitcoinObserverConsistencyError> for BitcoinObserverError {
    fn from(error: BitcoinObserverConsistencyError) -> Self {
        Self::Db(error.into())
    }
}

impl BitcoinObserverError {
    fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Rpc(_) | Self::DeepReorg | Self::ChainChanged | Self::RequestUnavailable(_)
        )
    }
}

/// Persisted observation progress and the result of the latest poll.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BitcoinObserverStatus {
    pub tip: Option<L1BlockCommitment>,
    pub revision: u64,
    pub caught_up: bool,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScanOutcome {
    CaughtUp,
    MoreWork,
}

/// Runs the Bitcoin deposit observer until shutdown.
pub async fn run_bitcoin_observer(
    db: Arc<impl BitcoinObserverDb>,
    bitcoin: BitcoinConfig,
    config: BitcoinObserverConfig,
    params: BitcoinParams,
    max_tx_confirmations: u64,
    status: watch::Sender<BitcoinObserverStatus>,
    shutdown: ShutdownGuard,
) -> Result<(), BitcoinObserverError> {
    let rpc = BitcoinRpcClient::new(bitcoin.client())?;
    info!("Bitcoin deposit observer starting");
    loop {
        let more_work = tokio::select! {
            _ = shutdown.wait_for_shutdown() => break,
            result = poll(db.as_ref(), &rpc, &config, &params, max_tx_confirmations, &status) => match result {
                Ok(ScanOutcome::MoreWork) => true,
                Ok(ScanOutcome::CaughtUp) => false,
                Err(error) if error.is_retryable() => {
                    warn!(%error, "Bitcoin observer will retry on the next poll");
                    false
                }
                Err(error) => return Err(error),
            }
        };
        if more_work {
            tokio::task::yield_now().await;
            continue;
        }
        tokio::select! {
            _ = shutdown.wait_for_shutdown() => break,
            _ = tokio::time::sleep(Duration::from_secs(config.poll_interval_s())) => {},
        }
    }
    info!("Bitcoin deposit observer stopped");
    Ok(())
}

async fn poll<D: BitcoinObserverDb, R: BitcoinSource>(
    db: &D,
    rpc: &R,
    config: &BitcoinObserverConfig,
    params: &BitcoinParams,
    max_tx_confirmations: u64,
    status: &watch::Sender<BitcoinObserverStatus>,
) -> Result<ScanOutcome, BitcoinObserverError> {
    let result = reconcile(db, rpc, config, params, max_tx_confirmations, status).await;
    if let Err(error) = &result {
        publish_status(db, status, false, Some(error.to_string()))?;
    }
    result
}

fn publish_status(
    db: &impl BitcoinObserverDb,
    status: &watch::Sender<BitcoinObserverStatus>,
    caught_up: bool,
    last_error: Option<String>,
) -> Result<(), BitcoinObserverError> {
    let state = db.get_observer_state()?;
    status.send_replace(BitcoinObserverStatus {
        tip: state.as_ref().and_then(|state| state.last_processed_block),
        revision: state.map_or(0, |state| state.revision),
        caught_up,
        last_error,
    });
    Ok(())
}

/// Starts at or before the oldest unspent request still visible under the recovery and failure cutoffs.
fn initial_scan_height(
    tip: u64,
    params: &BitcoinParams,
    config: &BitcoinObserverConfig,
    max_tx_confirmations: u64,
) -> Result<u64, BitcoinObserverError> {
    let lookback = u64::from(params.recovery_delay)
        .max(u64::from(config.reorg_safe_depth()))
        .checked_add(max_tx_confirmations)
        .ok_or(BitcoinObserverError::Overflow)?;
    // A lookback beyond genesis starts at genesis; it is not a height underflow.
    Ok(params.genesis_height.max(tip.saturating_sub(lookback)))
}

async fn reconcile<D: BitcoinObserverDb, R: BitcoinSource>(
    db: &D,
    rpc: &R,
    config: &BitcoinObserverConfig,
    params: &BitcoinParams,
    max_tx_confirmations: u64,
    status: &watch::Sender<BitcoinObserverStatus>,
) -> Result<ScanOutcome, BitcoinObserverError> {
    let info = rpc.chain_info().await?;
    let expected_chain = match params.network {
        Network::Bitcoin => "main",
        Network::Testnet => "test",
        Network::Testnet4 => "testnet4",
        Network::Signet => "signet",
        Network::Regtest => "regtest",
    };
    if info.chain != expected_chain {
        return Err(BitcoinObserverError::NetworkMismatch);
    }
    let genesis_hash = rpc.block_hash(0).await?;
    if genesis_hash != genesis_block(params.network).block_hash() {
        return Err(BitcoinObserverError::NetworkMismatch);
    }
    let metadata = BitcoinObserverDbMetadata {
        genesis_hash,
        params_hash: params.params_hash,
    };
    let stored_state = db.get_observer_state()?;
    let anchor = match stored_state {
        Some(state) => state.last_processed_block,
        None => {
            let start_height =
                initial_scan_height(info.blocks, params, config, max_tx_confirmations)?;
            // If protocol genesis is still ahead, anchor at the current Bitcoin tip.
            let anchor_height = start_height.saturating_sub(1).min(info.blocks);
            if start_height == 0 {
                None
            } else {
                let height =
                    u32::try_from(anchor_height).map_err(|_| BitcoinObserverError::Overflow)?;
                let hash = rpc.block_hash(anchor_height).await?.to_l1_block_id();
                Some(L1BlockCommitment::new(height, hash))
            }
        }
    };
    db.initialize(&metadata, anchor)?;
    publish_status(db, status, false, None)?;

    // Find a canonical ancestor before mutating anything: deep reorgs leave storage intact.
    let history = db.rollback_history(config.reorg_safe_depth())?;
    let mut ancestor = None;
    for (depth, candidate) in history.iter().enumerate() {
        let canonical = match candidate {
            Some(block) if u64::from(block.height) <= info.blocks => {
                rpc.block_hash(u64::from(block.height))
                    .await?
                    .to_l1_block_id()
                    == block.blkid
            }
            Some(_) => false,
            None => true,
        };
        if canonical {
            ancestor = Some(depth);
            break;
        }
    }
    let depth = ancestor.ok_or(BitcoinObserverError::DeepReorg)?;
    for block in history.iter().take(depth).flatten() {
        db.rollback_tip(*block)?;
        publish_status(db, status, false, None)?;
    }
    let mut tip = db
        .get_observer_state()?
        .ok_or(BitcoinObserverConsistencyError::ObserverStateMismatch)?
        .last_processed_block;
    for _ in 0..config.blocks_per_batch() {
        let height = match tip {
            Some(block) => u64::from(block.height)
                .checked_add(1)
                .ok_or(BitcoinObserverError::Overflow)?,
            None => 0,
        };
        if height > info.blocks {
            break;
        }
        let block = rpc.block(height).await?;
        let parent_hash = tip.map_or(BlockHash::all_zeros(), |block| block.blkid.to_block_hash());
        if block.header.prev_blockhash != parent_hash {
            return Err(BitcoinObserverError::ChainChanged);
        }
        let commitment = L1BlockCommitment::new(
            u32::try_from(height).map_err(|_| BitcoinObserverError::Overflow)?,
            block.block_hash().to_l1_block_id(),
        );
        let rows = recognize_block(db, rpc, params, commitment, &block).await?;
        // Recovery may involve several RPC calls. Recheck the scanned branch before committing.
        if rpc.block_hash(height).await?.to_l1_block_id() != commitment.blkid {
            return Err(BitcoinObserverError::ChainChanged);
        }
        db.apply_block(tip, commitment, &rows, config.reorg_safe_depth())?;
        tip = Some(commitment);
        publish_status(db, status, false, None)?;
    }
    let current = rpc.chain_info().await?;
    let caught_up = tip.is_some_and(|tip| {
        u64::from(tip.height) == current.blocks
            && tip.blkid == current.bestblockhash.to_l1_block_id()
    });
    publish_status(db, status, caught_up, None)?;
    Ok(if caught_up {
        ScanOutcome::CaughtUp
    } else {
        ScanOutcome::MoreWork
    })
}

async fn recognize_block<D: BitcoinObserverDb, R: BitcoinSource>(
    db: &D,
    rpc: &R,
    params: &BitcoinParams,
    block: L1BlockCommitment,
    data: &Block,
) -> Result<Vec<ObservedDeposit>, BitcoinObserverError> {
    if u64::from(block.height) < params.genesis_height {
        return Ok(Vec::new());
    }
    let mut changes = BTreeMap::new();
    let parser = params.parser_at(u64::from(block.height));
    let discovery_parser = params.discovery_parser();
    for (position, tx) in data.txdata.iter().enumerate() {
        let inclusion = TransactionInclusion {
            block,
            position: u32::try_from(position).map_err(|_| BitcoinObserverError::Overflow)?,
        };
        if let Some(parser) = &parser {
            match parser.parse_deposit_request(tx) {
                Ok(Some(request)) => {
                    changes.insert(
                        request.outpoint(),
                        ObservedDeposit {
                            request: tx.clone(),
                            inclusion,
                            spend: None,
                        },
                    );
                }
                Ok(None) => {}
                Err(error) => {
                    debug!(txid = %tx.compute_txid(), %error, "transaction is not a valid deposit request")
                }
            }
        }
        // A DT can spend a request older than the first-sync lookback.
        if let Some(parser) = &discovery_parser {
            match parser.parse_deposit_spent_request(tx) {
                Ok(Some(outpoint))
                    if !changes.contains_key(&outpoint) && db.get_deposit(outpoint)?.is_none() =>
                {
                    if let Some(row) = recover_request(rpc, params, outpoint, tx, inclusion).await?
                    {
                        changes.insert(outpoint, row);
                    }
                }
                Err(error) => {
                    debug!(txid = %tx.compute_txid(), %error, "transaction is not a valid deposit candidate")
                }
                _ => {}
            }
        }
        for input in &tx.input {
            let mut observed = match changes.get(&input.previous_output) {
                Some(row) => row.clone(),
                None => match db.get_deposit(input.previous_output)? {
                    Some(row) => row,
                    None => continue,
                },
            };
            let parser = params
                .parser_at(u64::from(observed.inclusion.block.height))
                .ok_or(BitcoinObserverError::InvalidStoredRequest)?;
            let request = parser
                .parse_deposit_request(&observed.request)
                .map_err(|_| BitcoinObserverError::InvalidStoredRequest)?
                .ok_or(BitcoinObserverError::InvalidStoredRequest)?;
            let spend = match parser.parse_deposit_request_spend(&request, tx) {
                Ok(Some(spend)) => spend,
                Ok(None) => return Err(BitcoinObserverError::InvalidStoredRequest),
                Err(error) => {
                    debug!(txid = %tx.compute_txid(), %error, "deposit request consumed by an unrecognized spend");
                    DepositSpend::Other {
                        txid: tx.compute_txid(),
                    }
                }
            };
            observed.spend = Some(ObservedDepositSpend { spend, inclusion });
            changes.insert(input.previous_output, observed);
        }
    }
    Ok(changes.into_values().collect())
}

async fn recover_request(
    rpc: &impl BitcoinSource,
    params: &BitcoinParams,
    outpoint: OutPoint,
    deposit: &Transaction,
    inclusion: TransactionInclusion,
) -> Result<Option<ObservedDeposit>, BitcoinObserverError> {
    let recovered = rpc
        .transaction(outpoint.txid)
        .await?
        .ok_or(BitcoinObserverError::RequestUnavailable(outpoint))?;
    let request_block = recovered.inclusion.block;
    if request_block.height > inclusion.block.height
        || (request_block.height == inclusion.block.height
            && recovered.inclusion.position >= inclusion.position)
        || rpc
            .block_hash(u64::from(request_block.height))
            .await?
            .to_l1_block_id()
            != request_block.blkid
    {
        return Err(BitcoinObserverError::ChainChanged);
    }
    if u64::from(request_block.height) < params.genesis_height {
        return Ok(None);
    }
    let Some(parser) = params.parser_at(u64::from(request_block.height)) else {
        return Ok(None);
    };
    let request = match parser.parse_deposit_request(&recovered.tx) {
        Ok(Some(request)) if request.outpoint() == outpoint => request,
        result => {
            debug!(%outpoint, ?result, "recovered transaction is not a valid deposit request");
            return Ok(None);
        }
    };
    match parser.parse_deposit_request_spend(&request, deposit) {
        Ok(Some(spend @ DepositSpend::Deposit { .. })) => Ok(Some(ObservedDeposit {
            request: recovered.tx,
            inclusion: recovered.inclusion,
            spend: Some(ObservedDepositSpend { spend, inclusion }),
        })),
        result => {
            debug!(%outpoint, ?result, "deposit candidate does not validate against its recovered request");
            Ok(None)
        }
    }
}
#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::Arc,
        time::{SystemTime, UNIX_EPOCH},
    };

    use bitcoin::{blockdata::constants::genesis_block, Network, ScriptBuf};
    use bitcoind_async_client::error::ClientError;
    use status_config::BitcoinObserverConfig;
    use strata_btc_types::BlockHashExt;
    use strata_identifiers::L1BlockCommitment;
    use tokio::sync::watch;
    use toml::{Table, Value as TomlValue};

    use super::{
        initial_scan_height, poll, reconcile, BitcoinObserverError, BitcoinObserverStatus,
        ScanOutcome,
    };
    use crate::{
        db::{bitcoin_observer::mock::MockBitcoinObserverDb, traits::BitcoinObserverDb},
        l1::{
            deposits::{DepositParams, DepositSpend},
            observations::ObservedDeposit,
            params::BitcoinParams,
            rpc::{BitcoinRpcClient, BitcoinRpcError, BitcoinSource},
            test_utils::{
                make_deposit_params, make_keypair, DepositFixture, MockBitcoinSource,
                TEST_DEPOSIT_INDEX,
            },
        },
    };

    struct ObserverFixture {
        deposits: DepositFixture,
        db: MockBitcoinObserverDb,
        rpc: Arc<MockBitcoinSource>,
        config: BitcoinObserverConfig,
        params: BitcoinParams,
        status: watch::Sender<BitcoinObserverStatus>,
    }

    impl ObserverFixture {
        fn new() -> Self {
            let params = make_deposit_params(1);
            let deposits = DepositFixture::with_params(params.clone());
            Self {
                deposits,
                db: MockBitcoinObserverDb::default(),
                rpc: Arc::new(MockBitcoinSource::new()),
                config: toml::from_str("").expect("default observer config"),
                params: BitcoinParams::for_test(Network::Regtest, 1, params),
                status: watch::channel(BitcoinObserverStatus::default()).0,
            }
        }
        fn with_blocks_per_batch(mut self, blocks_per_batch: u32) -> Self {
            let mut settings = Table::try_from(&self.config).expect("observer settings");
            settings.insert(
                "blocks_per_batch".into(),
                TomlValue::Integer(i64::from(blocks_per_batch)),
            );
            self.config = settings.try_into().expect("batch-size override");
            self
        }

        fn with_reorg_safe_depth(mut self, reorg_safe_depth: u32) -> Self {
            let mut settings = Table::try_from(&self.config).expect("observer settings");
            settings.insert(
                "reorg_safe_depth".into(),
                TomlValue::Integer(i64::from(reorg_safe_depth)),
            );
            self.config = settings.try_into().expect("reorg-depth override");
            self
        }

        async fn poll(&self) -> Result<ScanOutcome, BitcoinObserverError> {
            poll(
                &self.db,
                self.rpc.as_ref(),
                &self.config,
                &self.params,
                6,
                &self.status,
            )
            .await
        }
        fn observed(&self) -> Option<ObservedDeposit> {
            self.db
                .get_deposit(self.deposits.request().outpoint())
                .expect("observed deposit")
        }
        fn tip(&self) -> L1BlockCommitment {
            self.db
                .get_observer_state()
                .expect("observer state")
                .expect("initialized state")
                .last_processed_block
                .expect("tip")
        }
    }

    #[tokio::test]
    async fn observer_stores_request_before_any_bridge_index_exists() {
        let fixture = ObserverFixture::new();
        let block = fixture
            .rpc
            .append(vec![fixture.deposits.request_tx().clone()]);
        assert_eq!(
            fixture.poll().await.expect("first scan"),
            ScanOutcome::CaughtUp
        );

        // A mined request has a stable outpoint and inclusion record, with no invented bridge index.
        let row = fixture.observed().expect("request discovered");
        assert_eq!(row.outpoint(), fixture.deposits.request().outpoint());
        assert_eq!(row.inclusion.block, block);
        assert_eq!(row.inclusion.position, 0);
        assert!(row.spend.is_none());
        assert_eq!(fixture.tip(), block);
    }

    #[tokio::test]
    async fn observer_links_deposit_spend_to_the_same_request() {
        let fixture = ObserverFixture::new();
        fixture
            .rpc
            .append(vec![fixture.deposits.request_tx().clone()]);
        fixture.poll().await.expect("request scan");
        let deposit = fixture.deposits.deposit_tx();
        let block = fixture.rpc.append(vec![deposit.clone()]);
        fixture.poll().await.expect("deposit scan");

        let row = fixture.observed().expect("same request row");
        let spend = row.spend.expect("deposit spend");
        assert_eq!(
            spend.spend,
            DepositSpend::Deposit {
                txid: deposit.compute_txid(),
                deposit_idx: TEST_DEPOSIT_INDEX
            }
        );
        assert_eq!(spend.inclusion.block, block);
        assert_eq!(row.request, *fixture.deposits.request_tx());
    }

    #[tokio::test]
    async fn observer_links_request_and_spend_in_the_same_block() {
        let fixture = ObserverFixture::new();
        fixture.rpc.append(vec![
            fixture.deposits.request_tx().clone(),
            fixture.deposits.deposit_tx(),
        ]);
        fixture.poll().await.expect("same-block scan");
        let row = fixture.observed().expect("request");
        assert_eq!(row.inclusion.position, 0);
        assert_eq!(row.spend.expect("same-block spend").inclusion.position, 1);
    }

    #[tokio::test]
    async fn observer_resumes_from_persisted_state_in_bounded_batches() {
        let fixture = ObserverFixture::new().with_blocks_per_batch(1);
        fixture
            .rpc
            .append(vec![fixture.deposits.request_tx().clone()]);
        let last = fixture.rpc.append(vec![fixture.deposits.deposit_tx()]);
        assert_eq!(
            fixture.poll().await.expect("first batch has more work"),
            ScanOutcome::MoreWork
        );
        assert!(fixture.observed().expect("request").spend.is_none());

        // A new reconciliation reads progress from storage; it does not replay block 1.
        assert_eq!(
            fixture.poll().await.expect("restart/catch-up batch"),
            ScanOutcome::CaughtUp
        );
        assert_eq!(fixture.tip(), last);
        assert_eq!(fixture.rpc.fetched_blocks(), vec![1, 2]);
        assert_eq!(
            fixture.poll().await.expect("idempotent caught-up poll"),
            ScanOutcome::CaughtUp
        );
        assert_eq!(fixture.rpc.fetched_blocks(), vec![1, 2]);
    }

    #[tokio::test]
    async fn observer_reorg_replaces_a_deposit_spend_with_a_reclaim() {
        let fixture = ObserverFixture::new();
        fixture
            .rpc
            .append(vec![fixture.deposits.request_tx().clone()]);
        fixture.rpc.append(vec![fixture.deposits.deposit_tx()]);
        fixture.poll().await.expect("original branch");

        // The competing branch consumes the same request via its recovery path.
        fixture.rpc.fork_after(1);
        let mut reclaim = fixture.deposits.spend_tx();
        fixture.deposits.sign_reclaim(&mut reclaim, None);
        let replacement = fixture.rpc.append(vec![reclaim.clone()]);
        fixture.poll().await.expect("reconcile replacement branch");
        let spend = fixture
            .observed()
            .expect("retained request")
            .spend
            .expect("replacement spend");
        assert_eq!(
            spend.spend,
            DepositSpend::Reclaim {
                txid: reclaim.compute_txid()
            }
        );
        assert_eq!(spend.inclusion.block, replacement);
        assert_eq!(fixture.tip(), replacement);
    }

    #[tokio::test]
    async fn observer_reorg_removes_orphaned_requests() {
        let fixture = ObserverFixture::new();
        fixture
            .rpc
            .append(vec![fixture.deposits.request_tx().clone()]);
        fixture.poll().await.expect("original request");
        fixture.rpc.fork_after(0);
        let replacement = fixture.rpc.append(Vec::new());
        fixture.poll().await.expect("replacement block");
        assert!(fixture.observed().is_none());
        assert_eq!(fixture.tip(), replacement);
    }

    #[tokio::test]
    async fn observer_deep_reorg_returns_error_without_partial_rollback() {
        let fixture = ObserverFixture::new().with_reorg_safe_depth(2);
        fixture
            .rpc
            .append(vec![fixture.deposits.request_tx().clone()]);
        for _ in 0..3 {
            fixture.rpc.append(Vec::new());
        }
        fixture.poll().await.expect("original branch");
        let original_tip = fixture.tip();
        let original_row = fixture.observed();

        fixture.rpc.fork_after(0);
        fixture.rpc.append(vec![fixture.deposits.deposit_tx()]);
        for _ in 0..3 {
            fixture.rpc.append(Vec::new());
        }
        assert!(matches!(
            fixture.poll().await,
            Err(BitcoinObserverError::DeepReorg)
        ));
        assert_eq!(fixture.tip(), original_tip);
        assert_eq!(fixture.observed(), original_row);
    }

    #[tokio::test]
    async fn observer_rpc_failure_keeps_the_last_successfully_persisted_block() {
        let fixture = ObserverFixture::new();
        let first = fixture
            .rpc
            .append(vec![fixture.deposits.request_tx().clone()]);
        fixture.rpc.append(vec![fixture.deposits.deposit_tx()]);
        fixture.rpc.fail_at(Some(2));
        assert!(matches!(
            fixture.poll().await,
            Err(BitcoinObserverError::Rpc(_))
        ));
        assert_eq!(fixture.tip(), first);
        assert!(fixture.observed().expect("saved request").spend.is_none());

        fixture.rpc.fail_at(None);
        fixture.poll().await.expect("retry from block 2");
        assert!(fixture.observed().expect("saved request").spend.is_some());
    }

    #[tokio::test]
    async fn observer_uses_the_requests_original_covenant_after_operator_rotation() {
        let mut fixture = ObserverFixture::new();
        let replacement = DepositParams::new(
            fixture.deposits.params().magic(),
            make_keypair(4).x_only_public_key().0,
            fixture.deposits.params().deposit_amount(),
            fixture.deposits.params().recovery_delay(),
        )
        .expect("replacement covenant parameters");
        fixture.params = fixture.params.with_test_covenant_period(2, replacement);
        fixture
            .rpc
            .append(vec![fixture.deposits.request_tx().clone()]);
        fixture.rpc.append(vec![fixture.deposits.deposit_tx()]);
        fixture
            .poll()
            .await
            .expect("spend under historical covenant");
        assert!(matches!(
            fixture
                .observed()
                .expect("request")
                .spend
                .expect("deposit spend")
                .spend,
            DepositSpend::Deposit {
                deposit_idx: TEST_DEPOSIT_INDEX,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn observer_discovers_deposits_through_the_bitcoin_rpc_client() {
        let fixture = ObserverFixture::new();
        let expected = fixture
            .rpc
            .append(vec![fixture.deposits.request_tx().clone()]);
        let server = fixture.rpc.serve().await;
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let cookie =
            std::env::temp_dir().join(format!("observer_rpc_{}_{unique}", std::process::id()));
        fs::write(&cookie, "test:password").expect("RPC cookie");
        let config = toml::from_str(&format!(
            "url = {:?}\nauth = {{ kind = \"cookie\", path = {:?} }}",
            server.url(),
            cookie
        ))
        .expect("RPC configuration");
        let client = BitcoinRpcClient::new(&config).expect("RPC client");
        fs::remove_file(cookie).expect("remove RPC cookie");
        assert_eq!(
            reconcile(
                &fixture.db,
                &client,
                &fixture.config,
                &fixture.params,
                6,
                &fixture.status
            )
            .await
            .expect("HTTP-backed observation"),
            ScanOutcome::CaughtUp,
        );
        assert_eq!(
            fixture
                .observed()
                .expect("discovered request")
                .inclusion
                .block,
            expected
        );
        assert_eq!(fixture.tip(), expected);
    }

    #[tokio::test]
    async fn observer_records_an_unrecognized_spend_instead_of_leaving_a_request_unspent() {
        let fixture = ObserverFixture::new();
        fixture
            .rpc
            .append(vec![fixture.deposits.request_tx().clone()]);
        let mut other = fixture.deposits.deposit_tx();
        other.output[0].script_pubkey = ScriptBuf::from_bytes(vec![0x6a]);
        fixture.deposits.sign_key_spend(&mut other, None);
        fixture.rpc.append(vec![other.clone()]);
        fixture
            .poll()
            .await
            .expect("scan malformed deposit candidate");
        assert_eq!(
            fixture
                .observed()
                .expect("request")
                .spend
                .expect("consumed output")
                .spend,
            DepositSpend::Other {
                txid: other.compute_txid()
            }
        );
    }

    #[tokio::test]
    async fn observer_rejects_a_different_network_before_initializing_storage() {
        let mut fixture = ObserverFixture::new();
        fixture.params.network = Network::Signet;
        fixture
            .rpc
            .append(vec![fixture.deposits.request_tx().clone()]);
        assert!(matches!(
            fixture.poll().await,
            Err(BitcoinObserverError::NetworkMismatch)
        ));
        assert!(fixture
            .db
            .get_observer_state()
            .expect("observer state")
            .is_none());
    }

    #[test]
    fn initial_scan_covers_recovery_and_failure_visibility_windows() {
        let fixture = ObserverFixture::new();
        assert_eq!(
            initial_scan_height(100, &fixture.params, &fixture.config, 6).expect("lookback"),
            88
        );
        let mut params = fixture.params;
        params.recovery_delay = 20;
        assert_eq!(
            initial_scan_height(100, &params, &fixture.config, 6).expect("recovery window"),
            74
        );
        assert_eq!(
            initial_scan_height(5, &params, &fixture.config, 6).expect("genesis bound"),
            1
        );
        assert!(matches!(
            initial_scan_height(100, &params, &fixture.config, u64::MAX),
            Err(BitcoinObserverError::Overflow)
        ));
    }

    #[tokio::test]
    async fn observer_recovers_a_pre_start_request_under_its_original_covenant() {
        let mut fixture = ObserverFixture::new();
        let request_block = fixture
            .rpc
            .append(vec![fixture.deposits.request_tx().clone()]);
        for _ in 0..20 {
            fixture.rpc.append(Vec::new());
        }
        let deposit_block = fixture.rpc.append(vec![fixture.deposits.deposit_tx()]);
        let replacement = DepositParams::new(
            fixture.deposits.params().magic(),
            make_keypair(9).x_only_public_key().0,
            fixture.deposits.params().deposit_amount(),
            1,
        )
        .expect("replacement covenant");
        fixture.params = fixture.params.with_test_covenant_period(2, replacement);

        fixture.poll().await.expect("recover old request");
        let row = fixture.observed().expect("recovered request and deposit");
        assert_eq!(row.inclusion.block, request_block);
        assert_eq!(row.spend.expect("deposit").inclusion.block, deposit_block);
        assert!(!fixture.rpc.fetched_blocks().contains(&1));
        assert!(fixture.status.borrow().caught_up);

        // The recovered row belongs to the DT block's atomic write and undo record.
        fixture.rpc.fork_after(21);
        fixture.rpc.append(Vec::new());
        fixture.poll().await.expect("replace DT block");
        assert!(fixture.observed().is_none());
    }

    #[tokio::test]
    async fn observer_retries_unavailable_request_recovery_without_advancing_the_deposit_block() {
        for result in [
            Ok(None),
            Err(BitcoinRpcError::Client(ClientError::Server(
                -28,
                "warming up".into(),
            ))),
        ] {
            let fixture = ObserverFixture::new();
            fixture
                .rpc
                .append(vec![fixture.deposits.request_tx().clone()]);
            for _ in 0..20 {
                fixture.rpc.append(Vec::new());
            }
            let deposit_block = fixture.rpc.append(vec![fixture.deposits.deposit_tx()]);
            fixture.rpc.override_transaction(result);

            assert!(fixture
                .poll()
                .await
                .expect_err("request unavailable")
                .is_retryable());
            assert_eq!(fixture.tip().height, deposit_block.height - 1);
            assert!(fixture.observed().is_none());
            assert!(!fixture.status.borrow().caught_up);
            assert!(fixture.status.borrow().last_error.is_some());

            fixture.poll().await.expect("request lookup recovers");
            assert_eq!(fixture.tip(), deposit_block);
            assert!(fixture.observed().expect("request").spend.is_some());
            assert!(fixture.status.borrow().caught_up);
            assert!(fixture.status.borrow().last_error.is_none());
        }
    }

    #[tokio::test]
    async fn observer_retries_recovery_from_a_noncanonical_request_block() {
        let fixture = ObserverFixture::new();
        fixture
            .rpc
            .append(vec![fixture.deposits.request_tx().clone()]);
        for _ in 0..20 {
            fixture.rpc.append(Vec::new());
        }
        let deposit_block = fixture.rpc.append(vec![fixture.deposits.deposit_tx()]);
        let mut recovered = fixture
            .rpc
            .transaction(fixture.deposits.request_tx().compute_txid())
            .await
            .expect("lookup")
            .expect("mined request");
        recovered.inclusion.block = L1BlockCommitment::new(
            1,
            genesis_block(Network::Regtest)
                .block_hash()
                .to_l1_block_id(),
        );
        fixture.rpc.override_transaction(Ok(Some(recovered)));
        assert!(matches!(
            fixture.poll().await,
            Err(BitcoinObserverError::ChainChanged)
        ));
        assert_eq!(fixture.tip().height, deposit_block.height - 1);
        assert!(fixture.observed().is_none());
        fixture.poll().await.expect("canonical request retry");
        assert_eq!(fixture.tip(), deposit_block);
    }

    #[tokio::test]
    async fn observer_skips_invalid_recovered_requests_and_invalid_deposits() {
        for invalid_request in [true, false] {
            let fixture = ObserverFixture::new();
            let mut request = fixture.deposits.request_tx().clone();
            let mut deposit = fixture.deposits.deposit_tx();
            if invalid_request {
                request.output[0].script_pubkey = ScriptBuf::from_bytes(vec![0x6a]);
                deposit.input[0].previous_output.txid = request.compute_txid();
            } else {
                deposit.output[1].script_pubkey = ScriptBuf::new();
            }
            fixture.deposits.sign_key_spend(&mut deposit, None);
            let outpoint = deposit.input[0].previous_output;
            fixture.rpc.append(vec![request]);
            for _ in 0..20 {
                fixture.rpc.append(Vec::new());
            }
            let deposit_block = fixture.rpc.append(vec![deposit]);

            fixture
                .poll()
                .await
                .expect("deterministically invalid candidate is skipped");
            assert_eq!(fixture.tip(), deposit_block);
            assert!(fixture.db.get_deposit(outpoint).expect("lookup").is_none());
        }
    }

    #[tokio::test]
    async fn observer_publishes_catchup_and_failed_poll_progress() {
        let fixture = ObserverFixture::new().with_blocks_per_batch(1);
        let first = fixture
            .rpc
            .append(vec![fixture.deposits.request_tx().clone()]);
        let second = fixture.rpc.append(vec![fixture.deposits.deposit_tx()]);
        fixture.poll().await.expect("first batch");
        assert_eq!(
            *fixture.status.borrow(),
            BitcoinObserverStatus {
                tip: Some(first),
                revision: 1,
                caught_up: false,
                last_error: None
            }
        );

        // The node tip stays unchanged while the next batch catches up.
        fixture.poll().await.expect("second batch");
        assert_eq!(
            *fixture.status.borrow(),
            BitcoinObserverStatus {
                tip: Some(second),
                revision: 2,
                caught_up: true,
                last_error: None
            }
        );
        let third = fixture.rpc.append(Vec::new());
        fixture.rpc.fail_at(Some(u64::from(third.height)));
        fixture.poll().await.expect_err("block fetch failed");
        let status = fixture.status.borrow().clone();
        assert_eq!(status.tip, Some(second));
        assert_eq!(status.revision, 2);
        assert!(!status.caught_up);
        assert!(status.last_error.is_some());
    }
}
