//! Deposit display status derived from a caught-up Bitcoin observer snapshot.

use std::{collections::BTreeMap, sync::Arc};

use bitcoin::{blockdata::constants::genesis_block, OutPoint};
use status_config::BitcoinObserverConfig;
use strata_identifiers::L1BlockCommitment;
use tokio::sync::watch;

use crate::{
    db::{error::DbError, traits::BitcoinObserverDb, types::BitcoinObserverDbMetadata},
    l1::{
        deposits::DepositSpend,
        observations::{ObservedDeposit, TransactionInclusion},
        observer::BitcoinObserverStatus,
        params::BitcoinParams,
    },
    types::{DepositInfo, DepositStatus, ObservedDepositStatusRow},
};

const DEPOSIT_PAGE_SIZE: usize = 256;

#[derive(Debug, thiserror::Error)]
pub(crate) enum DepositStatusError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error("Bitcoin observer has not caught up to the canonical node tip")]
    NotReady,
    #[error("Bitcoin observer changed while reading deposit observations")]
    SnapshotChanged,
    #[error("Bitcoin observer metadata does not match the configured scan")]
    ConfigurationMismatch,
    #[error("Bitcoin deposit inclusion is inconsistent with the observer tip")]
    InvalidInclusion,
}

/// Reads persisted deposit evidence using the observer's published progress.
pub struct DepositStatusReader {
    db: Arc<dyn BitcoinObserverDb>,
    metadata: BitcoinObserverDbMetadata,
    status: watch::Receiver<BitcoinObserverStatus>,
    reorg_safe_depth: u64,
    recovery_delay: u64,
    max_tx_confirmations: u64,
}

impl DepositStatusReader {
    pub fn new(
        observer: &BitcoinObserverConfig,
        params: &BitcoinParams,
        max_tx_confirmations: u64,
        db: Arc<impl BitcoinObserverDb>,
        status: watch::Receiver<BitcoinObserverStatus>,
    ) -> Self {
        Self {
            db,
            metadata: BitcoinObserverDbMetadata::new(
                genesis_block(params.network).block_hash(),
                params.params_hash,
            ),
            status,
            reorg_safe_depth: u64::from(observer.reorg_safe_depth()),
            recovery_delay: u64::from(params.recovery_delay),
            max_tx_confirmations,
        }
    }

    pub(crate) fn status(&self) -> watch::Receiver<BitcoinObserverStatus> {
        self.status.clone()
    }

    pub(crate) async fn read(
        &self,
        bridge_answers: &BTreeMap<OutPoint, DepositStatus>,
    ) -> Result<Vec<ObservedDepositStatusRow>, DepositStatusError> {
        self.read_pages(bridge_answers, DEPOSIT_PAGE_SIZE).await
    }

    async fn read_pages(
        &self,
        bridge_answers: &BTreeMap<OutPoint, DepositStatus>,
        page_size: usize,
    ) -> Result<Vec<ObservedDepositStatusRow>, DepositStatusError> {
        let progress = self.status.borrow().clone();
        if !progress.caught_up {
            return Err(DepositStatusError::NotReady);
        }
        let tip = progress.tip.ok_or(DepositStatusError::NotReady)?;
        self.check_snapshot(&progress)?;
        let mut rows = Vec::new();
        let mut after = None;
        loop {
            let page = self.db.get_deposits_from(after, page_size)?;
            let last = page.last().map(ObservedDeposit::outpoint);
            let count = page.len();
            for observed in page {
                let answer = bridge_answers.get(&observed.outpoint()).copied();
                if let Some(row) = deposit_status_row(
                    observed,
                    answer,
                    tip,
                    self.reorg_safe_depth,
                    self.recovery_delay,
                    self.max_tx_confirmations,
                )? {
                    rows.push(row);
                }
            }
            self.check_snapshot(&progress)?;
            if count < page_size {
                break;
            }
            after = last;
            tokio::task::yield_now().await;
        }
        Ok(rows)
    }

    fn check_snapshot(&self, progress: &BitcoinObserverStatus) -> Result<(), DepositStatusError> {
        let state = self
            .db
            .get_observer_state()?
            .ok_or(DepositStatusError::NotReady)?;
        if state.metadata != self.metadata {
            return Err(DepositStatusError::ConfigurationMismatch);
        }
        let current = self.status.borrow();
        if state.last_processed_block != progress.tip
            || state.revision != progress.revision
            || !current.caught_up
            || current.tip != progress.tip
            || current.revision != progress.revision
        {
            return Err(DepositStatusError::SnapshotChanged);
        }
        Ok(())
    }
}

fn deposit_status_row(
    observed: ObservedDeposit,
    bridge_answer: Option<DepositStatus>,
    tip: L1BlockCommitment,
    reorg_safe_depth: u64,
    recovery_delay: u64,
    max_tx_confirmations: u64,
) -> Result<Option<ObservedDepositStatusRow>, DepositStatusError> {
    let request_confirmations = confirmations_from_inclusion(observed.inclusion, tip)?;
    let mut confirmations = request_confirmations;
    let mut terminal_depth = None;
    let mut info = DepositInfo {
        deposit_request_txid: observed.request.compute_txid(),
        deposit_txid: None,
        status: DepositStatus::InProgress,
    };
    if let Some(spend) = &observed.spend {
        confirmations = confirmations_from_inclusion(spend.inclusion, tip)?;
        match spend.spend {
            DepositSpend::Deposit { txid, .. } => {
                info.deposit_txid = Some(txid);
                if confirmations >= reorg_safe_depth {
                    info.status = DepositStatus::Complete;
                    terminal_depth = Some(reorg_safe_depth);
                }
            }
            DepositSpend::Reclaim { .. } | DepositSpend::Other { .. } => return Ok(None),
        }
    } else if matches!(bridge_answer, Some(DepositStatus::Failed)) {
        info.status = DepositStatus::Failed;
        terminal_depth = Some(reorg_safe_depth);
    } else if !matches!(bridge_answer, Some(DepositStatus::Complete))
        && request_confirmations >= recovery_delay
    {
        info.status = DepositStatus::Failed;
        terminal_depth = Some(recovery_delay);
    }
    // Subtracting the terminal anchor avoids overflow when retention is configured very large.
    if terminal_depth
        .and_then(|depth| confirmations.checked_sub(depth))
        .is_some_and(|age| age >= max_tx_confirmations)
    {
        return Ok(None);
    }
    Ok(Some(ObservedDepositStatusRow {
        request_outpoint: observed.outpoint(),
        info,
    }))
}

fn confirmations_from_inclusion(
    inclusion: TransactionInclusion,
    tip: L1BlockCommitment,
) -> Result<u64, DepositStatusError> {
    let depth = tip
        .height
        .checked_sub(inclusion.block.height)
        .ok_or(DepositStatusError::InvalidInclusion)?;
    if depth == 0 && inclusion.block.blkid != tip.blkid {
        return Err(DepositStatusError::InvalidInclusion);
    }
    Ok(u64::from(depth) + 1)
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use std::{collections::BTreeMap, fs};

    use bitcoin::{blockdata::constants::genesis_block, Network, OutPoint};
    use status_config::{
        test_utils::{make_bridge_config, LOCAL_ESPLORA_URL},
        BitcoinObserverConfig,
    };
    use strata_btc_types::BlockHashExt;
    use strata_identifiers::L1BlockCommitment;
    use strata_tasks::TaskManager;
    use tokio::{
        runtime::Handle,
        sync::{oneshot, watch},
        time::{timeout, Duration},
    };
    use toml::{Table, Value as TomlValue};

    use super::{DepositStatusError, DepositStatusReader};
    use crate::{
        context::BridgeMonitoringContext,
        db::{
            bitcoin_observer::mock::MockBitcoinObserverDb,
            error::DbResult,
            status::{db::BridgeStatusDbSled, mock::MockBridgeStatusDb},
            traits::BitcoinObserverDb,
            types::{BitcoinObserverDbMetadata, BitcoinObserverState},
            withdrawal_index::mock::MockWithdrawalIndexerDb,
        },
        l1::{
            deposits::{deposit_request_outpoint, DepositSpend},
            observations::{ObservedDeposit, ObservedDepositSpend, TransactionInclusion},
            observer::BitcoinObserverStatus,
            params::BitcoinParams,
            test_utils::{
                make_deposit_params, DepositFixture, MockBitcoinSource, TEST_DEPOSIT_INDEX,
            },
        },
        state::{BridgeMonitoringState, DepositInfoUpdate},
        status::{deposit_status_monitoring_task, get_bridge_status},
        test_utils::make_unique_db_path,
        types::{DepositInfo, DepositStatus, ObservedDepositStatusRow},
    };

    const DISPLAY_CONFIRMATION_CUTOFF: u64 = 100;
    const TEST_RECOVERY_DELAY: u16 = 144;

    struct DepositStatusFixture {
        deposit: DepositFixture,
        db: Arc<MockBitcoinObserverDb>,
        source: Arc<MockBitcoinSource>,
        params: BitcoinParams,
        observer: BitcoinObserverConfig,
        progress: watch::Sender<BitcoinObserverStatus>,
        indexed: Vec<DepositInfo>,
    }

    impl DepositStatusFixture {
        fn new() -> Self {
            Self::with_recovery_delay(TEST_RECOVERY_DELAY)
        }

        fn with_recovery_delay(recovery_delay: u16) -> Self {
            let deposit = DepositFixture::with_params(make_deposit_params(recovery_delay));
            let params = BitcoinParams::for_test(Network::Regtest, 1, deposit.params().clone());
            let metadata = BitcoinObserverDbMetadata::new(
                genesis_block(Network::Regtest).block_hash(),
                params.params_hash,
            );
            let db = Arc::new(MockBitcoinObserverDb::default());
            let source = Arc::new(MockBitcoinSource::new());
            let genesis = L1BlockCommitment::new(0, metadata.genesis_hash.to_l1_block_id());
            db.initialize(&metadata, Some(genesis))
                .expect("initialize scan anchor");
            Self {
                deposit,
                db,
                source,
                params,
                observer: toml::from_str("").expect("observer config"),
                progress: watch::channel(BitcoinObserverStatus::default()).0,
                indexed: Vec::new(),
            }
        }

        fn tip(&self) -> L1BlockCommitment {
            self.db
                .get_observer_state()
                .expect("state")
                .expect("initialized")
                .last_processed_block
                .expect("scan anchor")
        }

        fn mine_request(&self) {
            let parent = self.tip();
            let block = self.source.append(vec![self.deposit.request_tx().clone()]);
            let row = ObservedDeposit::new(
                self.deposit.request_tx().clone(),
                TransactionInclusion::new(block, 0),
                None,
            );
            self.db
                .apply_block(
                    Some(parent),
                    block,
                    &[row],
                    self.observer.reorg_safe_depth(),
                )
                .expect("request block");
            self.publish_progress(true);
        }

        fn mine_deposit(&self) {
            let parent = self.tip();
            let tx = self.deposit.deposit_tx();
            let block = self.source.append(vec![tx.clone()]);
            let mut row = self
                .db
                .get_deposit(self.deposit.request().outpoint())
                .expect("request")
                .expect("known request");
            row.spend = Some(ObservedDepositSpend::new(
                TransactionInclusion::new(block, 0),
                DepositSpend::Deposit {
                    txid: tx.compute_txid(),
                    deposit_idx: TEST_DEPOSIT_INDEX,
                },
            ));
            self.db
                .apply_block(
                    Some(parent),
                    block,
                    &[row],
                    self.observer.reorg_safe_depth(),
                )
                .expect("deposit block");
            self.publish_progress(true);
        }

        fn mine_request_spend(&self, reclaim: bool) {
            let parent = self.tip();
            let mut tx = self.deposit.spend_tx();
            if reclaim {
                self.deposit.sign_reclaim(&mut tx, None);
            } else {
                self.deposit.sign_key_spend(&mut tx, None);
            }
            let spend = self
                .deposit
                .parser()
                .parse_deposit_request_spend(&self.deposit.request(), &tx)
                .expect("valid spend")
                .expect("spends request");
            let block = self.source.append(vec![tx]);
            let mut row = self
                .db
                .get_deposit(self.deposit.request().outpoint())
                .expect("request")
                .expect("known request");
            row.spend = Some(ObservedDepositSpend::new(
                TransactionInclusion::new(block, 0),
                spend,
            ));
            self.db
                .apply_block(
                    Some(parent),
                    block,
                    &[row],
                    self.observer.reorg_safe_depth(),
                )
                .expect("spend block");
            self.publish_progress(true);
        }

        fn mine_empty_blocks(&self, count: u32) {
            for _ in 0..count {
                let parent = self.tip();
                let block = self.source.append(Vec::new());
                self.db
                    .apply_block(Some(parent), block, &[], self.observer.reorg_safe_depth())
                    .expect("empty block");
                self.publish_progress(true);
            }
        }

        fn rollback_to(&self, height: u32) {
            self.source.fork_after(height as usize);
            while self.tip().height > height {
                self.db.rollback_tip(self.tip()).expect("rollback");
                self.publish_progress(true);
            }
        }

        fn publish_progress(&self, caught_up: bool) {
            let state = self
                .db
                .get_observer_state()
                .expect("state")
                .expect("initialized");
            self.progress.send_replace(BitcoinObserverStatus {
                tip: state.last_processed_block,
                revision: state.revision,
                caught_up,
                last_error: None,
            });
        }

        fn reader(&self) -> DepositStatusReader {
            self.reader_with_retention(DISPLAY_CONFIRMATION_CUTOFF)
        }

        fn reader_with_retention(&self, max_tx_confirmations: u64) -> DepositStatusReader {
            DepositStatusReader::new(
                &self.observer,
                &self.params,
                max_tx_confirmations,
                Arc::clone(&self.db),
                self.progress.subscribe(),
            )
        }

        async fn read(&self) -> Result<Vec<ObservedDepositStatusRow>, DepositStatusError> {
            let answers = self
                .indexed
                .iter()
                .map(|info| {
                    (
                        deposit_request_outpoint(info.deposit_request_txid),
                        info.status,
                    )
                })
                .collect();
            self.reader().read_pages(&answers, 1).await
        }

        async fn row(&self) -> ObservedDepositStatusRow {
            let mut rows = self.read().await.expect("canonical snapshot");
            assert_eq!(rows.len(), 1);
            rows.pop().expect("deposit row")
        }
    }

    struct ReplayingBitcoinObserverDb {
        inner: Arc<MockBitcoinObserverDb>,
        replayed: AtomicBool,
        reorg_safe_depth: u32,
    }

    impl BitcoinObserverDb for ReplayingBitcoinObserverDb {
        fn get_observer_state(&self) -> DbResult<Option<BitcoinObserverState>> {
            self.inner.get_observer_state()
        }

        fn initialize(
            &self,
            metadata: &BitcoinObserverDbMetadata,
            anchor: Option<L1BlockCommitment>,
        ) -> DbResult<()> {
            self.inner.initialize(metadata, anchor)
        }

        fn get_deposit(&self, outpoint: OutPoint) -> DbResult<Option<ObservedDeposit>> {
            self.inner.get_deposit(outpoint)
        }

        fn rollback_history(&self, limit: u32) -> DbResult<Vec<Option<L1BlockCommitment>>> {
            self.inner.rollback_history(limit)
        }

        fn apply_block(
            &self,
            parent: Option<L1BlockCommitment>,
            block: L1BlockCommitment,
            rows: &[ObservedDeposit],
            window: u32,
        ) -> DbResult<()> {
            self.inner.apply_block(parent, block, rows, window)
        }

        fn rollback_tip(&self, expected: L1BlockCommitment) -> DbResult<()> {
            self.inner.rollback_tip(expected)
        }

        fn get_deposits_from(
            &self,
            after: Option<OutPoint>,
            limit: usize,
        ) -> DbResult<Vec<ObservedDeposit>> {
            let page = self.inner.get_deposits_from(after, limit)?;
            if !self.replayed.swap(true, Ordering::SeqCst) {
                let state = self.inner.get_observer_state()?.expect("initialized");
                let tip = state.last_processed_block.expect("tip");
                let rows = page.clone();
                let history = self.inner.rollback_history(1)?;
                self.inner.rollback_tip(tip)?;
                self.inner
                    .apply_block(history[1], tip, &rows, self.reorg_safe_depth)?;
            }
            Ok(page)
        }
    }

    fn start_status_task(
        reader: DepositStatusReader,
        refresh_seconds: u64,
    ) -> (
        Arc<BridgeMonitoringContext>,
        TaskManager,
        oneshot::Receiver<bool>,
    ) {
        let mut settings = Table::try_from(make_bridge_config(LOCAL_ESPLORA_URL, &[], Some(0)))
            .expect("bridge settings");
        settings.insert(
            "max_tx_confirmations".into(),
            TomlValue::try_from(DISPLAY_CONFIRMATION_CUTOFF).expect("display cutoff"),
        );
        settings.insert(
            "status_refetch_interval_s".into(),
            TomlValue::try_from(refresh_seconds).expect("refresh interval"),
        );
        let context = Arc::new(
            BridgeMonitoringContext::new(
                settings.try_into().expect("bridge config"),
                Arc::new(MockWithdrawalIndexerDb::default()),
                Arc::new(MockBridgeStatusDb::default()),
            )
            .expect("monitoring context"),
        );
        let manager = TaskManager::new(Handle::current());
        let (completed, done) = oneshot::channel();
        let polling_context = Arc::clone(&context);
        manager
            .create_executor()
            .spawn_critical_async_with_shutdown("test-deposit-status", move |guard| async move {
                let result = deposit_status_monitoring_task(polling_context, reader, guard).await;
                completed
                    .send(result.is_ok())
                    .expect("test completion receiver");
                result
            });
        (context, manager, done)
    }

    #[tokio::test]
    async fn deposit_status_excludes_a_request_without_a_canonical_inclusion() {
        // Seed bridge metadata without a mined request; it is not canonical Bitcoin evidence.
        let mut fixture = DepositStatusFixture::new();
        fixture.mine_empty_blocks(1);
        fixture.indexed.push(DepositInfo {
            deposit_request_txid: fixture.deposit.request_tx().compute_txid(),
            deposit_txid: None,
            status: DepositStatus::InProgress,
        });
        assert!(fixture.read().await.expect("no mined request").is_empty());
    }

    #[tokio::test]
    async fn deposit_status_includes_a_request_at_one_confirmation_before_bridge_indexing() {
        let fixture = DepositStatusFixture::new();
        fixture.mine_request();
        // The observer supplies the row before the bridge has assigned a deposit index.
        let row = fixture.row().await;
        assert_eq!(row.request_outpoint, fixture.deposit.request().outpoint());
        assert!(matches!(row.info.status, DepositStatus::InProgress));
        assert!(row.info.deposit_txid.is_none());
    }

    #[tokio::test]
    async fn deposit_status_excludes_an_unconfirmed_deposit_transaction_even_when_the_bridge_says_complete(
    ) {
        let mut fixture = DepositStatusFixture::new();
        fixture.mine_request();
        fixture.indexed.push(DepositInfo {
            deposit_request_txid: fixture.deposit.request_tx().compute_txid(),
            deposit_txid: Some(fixture.deposit.deposit_tx().compute_txid()),
            status: DepositStatus::Complete,
        });
        let row = fixture.row().await;
        assert!(row.info.deposit_txid.is_none());
        assert!(matches!(row.info.status, DepositStatus::InProgress));
    }

    #[tokio::test]
    async fn deposit_status_includes_the_deposit_transaction_at_one_confirmation_and_completes_at_six(
    ) {
        let mut fixture = DepositStatusFixture::new();
        fixture.mine_request();
        fixture.mine_deposit();
        // Protocol completion is deliberately absent; the display follows Bitcoin evidence.
        fixture.indexed.push(DepositInfo {
            deposit_request_txid: fixture.deposit.request_tx().compute_txid(),
            deposit_txid: None,
            status: DepositStatus::InProgress,
        });
        // One confirmation exposes the transaction, but does not complete the deposit.
        let first = fixture.row().await;
        assert_eq!(
            first.info.deposit_txid,
            Some(fixture.deposit.deposit_tx().compute_txid())
        );
        assert!(matches!(first.info.status, DepositStatus::InProgress));
        // Five confirmations remain In progress; the sixth completes the deposit.
        fixture.mine_empty_blocks(4);
        let fifth = fixture.row().await;
        assert!(matches!(fifth.info.status, DepositStatus::InProgress));
        fixture.mine_empty_blocks(1);
        let sixth = fixture.row().await;
        assert!(matches!(sixth.info.status, DepositStatus::Complete));
        assert_eq!(fixture.read().await.expect("merged view").len(), 1);
    }

    #[tokio::test]
    async fn deposit_status_uses_the_configured_completion_depth() {
        let mut fixture = DepositStatusFixture::new();
        fixture.observer = toml::from_str("reorg_safe_depth = 3").expect("observer config");
        fixture.mine_request();
        fixture.mine_deposit();
        fixture.mine_empty_blocks(1);
        assert!(matches!(
            fixture.row().await.info.status,
            DepositStatus::InProgress
        ));
        fixture.mine_empty_blocks(1);
        assert!(matches!(
            fixture.row().await.info.status,
            DepositStatus::Complete
        ));
    }

    #[tokio::test]
    async fn deposit_status_reorg_reverses_completion_and_removes_an_orphaned_deposit_transaction()
    {
        let fixture = DepositStatusFixture::new();
        fixture.mine_request();
        fixture.mine_deposit();
        fixture.mine_empty_blocks(5);
        let state = BridgeMonitoringState::default();
        state
            .replace_observed_deposits(fixture.read().await.expect("complete snapshot"))
            .await;
        assert!(matches!(
            state
                .bridge_status(DISPLAY_CONFIRMATION_CUTOFF)
                .await
                .deposits[0]
                .status,
            DepositStatus::Complete
        ));

        // The DT remains canonical at a lower depth, so the row goes back to In progress.
        fixture.rollback_to(3);
        state
            .replace_observed_deposits(fixture.read().await.expect("lower depth"))
            .await;
        let status = state.bridge_status(DISPLAY_CONFIRMATION_CUTOFF).await;
        assert!(matches!(
            status.deposits[0].status,
            DepositStatus::InProgress
        ));
        assert!(status.deposits[0].deposit_txid.is_some());
        // Removing the spend removes its TXID as well as its completed status.
        fixture.rollback_to(1);
        state
            .replace_observed_deposits(fixture.read().await.expect("orphaned DT"))
            .await;
        assert!(state
            .bridge_status(DISPLAY_CONFIRMATION_CUTOFF)
            .await
            .deposits[0]
            .deposit_txid
            .is_none());
        // Use a fresh chain so removing the request stays within the retained undo history.
        let fixture = DepositStatusFixture::new();
        fixture.mine_request();
        fixture.rollback_to(0);
        state
            .replace_observed_deposits(fixture.read().await.expect("orphaned request"))
            .await;
        assert!(state
            .bridge_status(DISPLAY_CONFIRMATION_CUTOFF)
            .await
            .deposits
            .is_empty());
    }

    #[tokio::test]
    async fn deposit_status_waits_until_observer_progress_matches_the_node_tip() {
        let fixture = DepositStatusFixture::new();
        fixture.mine_request();
        fixture.source.append(Vec::new());
        fixture.publish_progress(false);
        assert!(matches!(
            fixture.read().await,
            Err(DepositStatusError::NotReady)
        ));
    }

    #[tokio::test]
    async fn deposit_status_rejects_a_snapshot_crossing_rollback_and_replay_of_the_same_tip() {
        let fixture = DepositStatusFixture::new();
        fixture.mine_request();
        let db = ReplayingBitcoinObserverDb {
            inner: Arc::clone(&fixture.db),
            replayed: AtomicBool::new(false),
            reorg_safe_depth: fixture.observer.reorg_safe_depth(),
        };
        // Mutate storage between page and revision reads, returning to the same block hash.
        let original = fixture
            .db
            .get_observer_state()
            .expect("original state")
            .expect("initialized");
        let reader = DepositStatusReader::new(
            &fixture.observer,
            &fixture.params,
            DISPLAY_CONFIRMATION_CUTOFF,
            Arc::new(db),
            fixture.progress.subscribe(),
        );
        let result = reader.read_pages(&BTreeMap::new(), 1).await;
        assert!(matches!(result, Err(DepositStatusError::SnapshotChanged)));
        assert_eq!(
            fixture
                .db
                .get_observer_state()
                .expect("state")
                .expect("initialized")
                .last_processed_block,
            original.last_processed_block
        );
        assert_eq!(
            fixture
                .db
                .get_observer_state()
                .expect("state")
                .expect("initialized")
                .revision,
            original.revision + 2
        );
    }

    #[tokio::test]
    async fn deposit_status_restores_the_view_after_restart_and_http_uses_only_the_cache() {
        let fixture = DepositStatusFixture::new();
        fixture.mine_request();
        fixture.mine_deposit();
        fixture.mine_empty_blocks(5);
        let reader = fixture.reader();
        let (context, manager, done) = start_status_task(reader, 60);
        let shutdown = manager.get_shutdown_signal();
        // A fresh monitoring context publishes from persisted observer evidence.
        let first = timeout(
            Duration::from_secs(2),
            get_bridge_status(Arc::clone(&context)),
        )
        .await
        .expect("initial status publication")
        .expect("initial HTTP status");
        assert!(matches!(
            first.0.deposits[0].status,
            DepositStatus::Complete
        ));
        shutdown.send();
        assert!(timeout(Duration::from_secs(2), done)
            .await
            .expect("status task shutdown")
            .expect("completed status task"));
        // Once polling stops, the HTTP handler still serves the cache without Bitcoin RPC.
        let response = get_bridge_status(context)
            .await
            .expect("cached HTTP status");
        assert_eq!(response.0.deposits.len(), 1);
        assert!(matches!(
            response.0.deposits[0].status,
            DepositStatus::Complete
        ));
    }

    #[tokio::test]
    async fn deposit_status_hides_completed_deposits_at_the_retention_boundary() {
        let fixture = DepositStatusFixture::new();
        fixture.mine_request();
        fixture.mine_deposit();
        // Complete at six confirmations; retain for twelve additional confirmations.
        fixture.mine_empty_blocks(16);
        let reader = fixture.reader_with_retention(12);
        let rows = reader.read(&BTreeMap::new()).await.expect("before cutoff");
        assert_eq!(rows.len(), 1);
        assert!(matches!(rows[0].info.status, DepositStatus::Complete));
        fixture.mine_empty_blocks(1);
        assert!(reader
            .read(&BTreeMap::new())
            .await
            .expect("at cutoff")
            .is_empty());
    }

    #[tokio::test]
    async fn deposit_status_overrides_unfinished_requests_at_the_recovery_delay() {
        for answer in [None, Some(DepositStatus::InProgress)] {
            let fixture = DepositStatusFixture::with_recovery_delay(6);
            fixture.mine_request();
            let answers = answer
                .map(|status| (fixture.deposit.request().outpoint(), status))
                .into_iter()
                .collect();
            let reader = fixture.reader_with_retention(12);
            fixture.mine_empty_blocks(4);
            let rows = reader.read(&answers).await.expect("before recovery");
            assert!(matches!(rows[0].info.status, DepositStatus::InProgress));
            fixture.mine_empty_blocks(1);
            let rows = reader.read(&answers).await.expect("at recovery");
            assert!(matches!(rows[0].info.status, DepositStatus::Failed));
            fixture.mine_empty_blocks(11);
            assert_eq!(reader.read(&answers).await.expect("before cutoff").len(), 1);
            fixture.mine_empty_blocks(1);
            assert!(reader.read(&answers).await.expect("at cutoff").is_empty());
            // Recovery eligibility does not expire the deposit path.
            fixture.mine_deposit();
            let rows = reader.read(&answers).await.expect("late deposit");
            assert!(matches!(rows[0].info.status, DepositStatus::InProgress));
            fixture.mine_empty_blocks(5);
            let rows = reader.read(&answers).await.expect("buried deposit");
            assert!(matches!(rows[0].info.status, DepositStatus::Complete));
        }
    }

    #[tokio::test]
    async fn deposit_status_does_not_override_a_bridge_complete_answer() {
        let fixture = DepositStatusFixture::with_recovery_delay(6);
        fixture.mine_request();
        fixture.mine_empty_blocks(30);
        let answers = BTreeMap::from([(
            fixture.deposit.request().outpoint(),
            DepositStatus::Complete,
        )]);
        let rows = fixture
            .reader_with_retention(12)
            .read(&answers)
            .await
            .expect("bridge complete");
        assert_eq!(rows.len(), 1);
        assert!(matches!(rows[0].info.status, DepositStatus::InProgress));
        assert!(rows[0].info.deposit_txid.is_none());
    }

    #[tokio::test]
    async fn deposit_status_retains_bridge_failure_after_polling_cache_purge() {
        let fixture = DepositStatusFixture::new();
        fixture.mine_request();
        fixture.mine_empty_blocks(5);
        let state = BridgeMonitoringState::default();
        let info = DepositInfo {
            deposit_request_txid: fixture.deposit.request_tx().compute_txid(),
            deposit_txid: None,
            status: DepositStatus::Failed,
        };
        let status_db = MockBridgeStatusDb::default();
        state
            .record_bridge_deposit_answers(&status_db, &[info])
            .await
            .expect("record bridge answer");
        // The legacy polling cursor advances at six confirmations, before the display expires.
        state
            .apply_deposit_info_updates(
                &status_db,
                vec![DepositInfoUpdate {
                    deposit_idx: TEST_DEPOSIT_INDEX,
                    info,
                    confirmations: Some(6),
                }],
                6,
            )
            .await
            .expect("purge polling row");
        let reader = fixture.reader_with_retention(12);
        let rows = reader
            .read(&state.bridge_deposit_answers().await)
            .await
            .expect("retained answer");
        assert!(matches!(rows[0].info.status, DepositStatus::Failed));
        // Failed RPCs contribute no successful answers and cannot erase the last answer.
        state
            .record_bridge_deposit_answers(&status_db, &[])
            .await
            .expect("retain bridge answer");
        let rows = reader
            .read(&state.bridge_deposit_answers().await)
            .await
            .expect("failed poll");
        assert!(matches!(rows[0].info.status, DepositStatus::Failed));
        fixture.mine_empty_blocks(11);
        assert_eq!(
            reader
                .read(&state.bridge_deposit_answers().await)
                .await
                .expect("before cutoff")
                .len(),
            1
        );
        fixture.mine_empty_blocks(1);
        state
            .record_bridge_deposit_answers(&status_db, &[])
            .await
            .expect("retain bridge answer");
        assert!(reader
            .read(&state.bridge_deposit_answers().await)
            .await
            .expect("expired row")
            .is_empty());
        // Canonical deposit evidence takes priority over the retained bridge failure.
        fixture.mine_deposit();
        let rows = reader
            .read(&state.bridge_deposit_answers().await)
            .await
            .expect("late deposit");
        assert!(matches!(rows[0].info.status, DepositStatus::InProgress));
    }

    #[tokio::test]
    async fn deposit_status_retains_bridge_failure_across_restart_after_cursor_advance() {
        let fixture = DepositStatusFixture::new();
        fixture.mine_request();
        fixture.mine_empty_blocks(5);
        let path = make_unique_db_path("deposit_answer_restart");
        let info = DepositInfo {
            deposit_request_txid: fixture.deposit.request_tx().compute_txid(),
            deposit_txid: None,
            status: DepositStatus::Failed,
        };
        {
            let db = BridgeStatusDbSled::open(&path).expect("open status db");
            let state = BridgeMonitoringState::default();
            // Save the successful answer before the legacy cursor retires this deposit at c=6.
            state
                .record_bridge_deposit_answers(&db, &[info])
                .await
                .expect("record failure");
            state
                .apply_deposit_info_updates(
                    &db,
                    vec![DepositInfoUpdate {
                        deposit_idx: 0,
                        info,
                        confirmations: Some(6),
                    }],
                    6,
                )
                .await
                .expect("advance cursor");
        }
        // Reopen at c=6 and again at c=12, with no further bridge responses.
        for (extra_blocks, visible) in [(0, true), (6, false)] {
            fixture.mine_empty_blocks(extra_blocks);
            let db = Arc::new(BridgeStatusDbSled::open(&path).expect("reopen status db"));
            let context = BridgeMonitoringContext::new(
                make_bridge_config(LOCAL_ESPLORA_URL, &[], None),
                Arc::new(MockWithdrawalIndexerDb::default()),
                db,
            )
            .expect("restore monitoring context");
            assert!(context
                .state()
                .select_deposit_info_candidates(&[0])
                .await
                .is_empty());
            let answers = context.state().bridge_deposit_answers().await;
            assert!(matches!(
                answers.get(&fixture.deposit.request().outpoint()),
                Some(DepositStatus::Failed)
            ));
            let reader = fixture.reader_with_retention(6);
            let rows = reader.read(&answers).await.expect("restored status");
            assert_eq!(rows.len(), usize::from(visible));
            if visible {
                assert!(matches!(rows[0].info.status, DepositStatus::Failed));
            }
            context.state().replace_observed_deposits(rows).await;
            assert_eq!(
                context.bridge_status().await.deposits.len(),
                usize::from(visible)
            );
        }
        // Twelve confirmations are still far short of this request's recovery delay (144).
        fs::remove_dir_all(path).expect("remove status db");
    }

    #[tokio::test]
    async fn deposit_status_hides_reclaimed_and_other_spent_requests_immediately() {
        for reclaim in [true, false] {
            let fixture = DepositStatusFixture::with_recovery_delay(6);
            fixture.mine_request();
            fixture.mine_empty_blocks(5);
            assert_eq!(fixture.read().await.expect("unspent request").len(), 1);
            fixture.mine_request_spend(reclaim);
            assert!(
                fixture.read().await.expect("spent request").is_empty(),
                "reclaim={reclaim}"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn deposit_status_refreshes_bridge_answers_without_a_new_block() {
        let fixture = DepositStatusFixture::new();
        fixture.mine_request();
        fixture.mine_empty_blocks(5);
        let (context, manager, done) = start_status_task(fixture.reader(), 1);
        let initial = get_bridge_status(Arc::clone(&context))
            .await
            .expect("initial publication");
        assert!(matches!(
            initial.0.deposits[0].status,
            DepositStatus::InProgress
        ));
        let mut info = initial.0.deposits[0];
        info.status = DepositStatus::Failed;
        context
            .state()
            .record_bridge_deposit_answers(context.status_db(), &[info])
            .await
            .expect("record bridge answer");
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        assert!(matches!(
            context.bridge_status().await.deposits[0].status,
            DepositStatus::Failed
        ));
        // While catch-up is incomplete, serve the last published snapshot.
        fixture.publish_progress(false);
        fixture.mine_deposit();
        fixture.publish_progress(false);
        tokio::task::yield_now().await;
        assert!(context.bridge_status().await.deposits[0]
            .deposit_txid
            .is_none());
        fixture.publish_progress(true);
        tokio::task::yield_now().await;
        assert!(context.bridge_status().await.deposits[0]
            .deposit_txid
            .is_some());
        manager.get_shutdown_signal().send();
        assert!(done.await.expect("task completed"));
    }

    #[tokio::test(start_paused = true)]
    async fn deposit_status_retries_a_failed_read_without_a_watch_update() {
        let fixture = DepositStatusFixture::new();
        fixture.publish_progress(true);
        // Publish progress before storage is available, then restore storage without a notification.
        let db = Arc::new(MockBitcoinObserverDb::default());
        let reader = DepositStatusReader::new(
            &fixture.observer,
            &fixture.params,
            DISPLAY_CONFIRMATION_CUTOFF,
            Arc::clone(&db),
            fixture.progress.subscribe(),
        );
        let (context, manager, done) = start_status_task(reader, 60);
        tokio::task::yield_now().await;
        let state = fixture
            .db
            .get_observer_state()
            .expect("state")
            .expect("initialized");
        db.initialize(&state.metadata, state.last_processed_block)
            .expect("restore storage");
        // This deadline is much shorter than the periodic refresh; only the retry can publish.
        let rows = timeout(Duration::from_secs(1), get_bridge_status(context))
            .await
            .expect("retry before periodic refresh")
            .expect("published snapshot");
        assert!(rows.0.deposits.is_empty());
        manager.get_shutdown_signal().send();
        assert!(done.await.expect("task completed"));
    }
}
