//! In-memory observer storage; snapshot copies keep test rollback setup explicit.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::RwLock,
};

use bitcoin::{consensus::serialize, OutPoint};
use strata_identifiers::L1BlockCommitment;

use crate::{
    db::{
        error::{BitcoinObserverConsistencyError, DbResult},
        traits::BitcoinObserverDb,
        types::{BitcoinObserverDbMetadata, BitcoinObserverState},
    },
    l1::observations::ObservedDeposit,
};

type Rows = BTreeMap<OutPoint, ObservedDeposit>;

#[derive(Default)]
struct Store {
    observer_state: Option<BitcoinObserverState>,
    rows: Rows,
    undo: Vec<MockBlockUndo>,
}

struct MockBlockUndo {
    parent: Option<L1BlockCommitment>,
    rows_before: Rows,
    updated_outpoints: BTreeSet<OutPoint>,
}

#[derive(Default)]
pub(crate) struct MockBitcoinObserverDb {
    store: RwLock<Store>,
}

impl BitcoinObserverDb for MockBitcoinObserverDb {
    fn get_observer_state(&self) -> DbResult<Option<BitcoinObserverState>> {
        Ok(self
            .store
            .read()
            .expect("mock store")
            .observer_state
            .clone())
    }

    fn initialize(
        &self,
        metadata: &BitcoinObserverDbMetadata,
        anchor: Option<L1BlockCommitment>,
    ) -> DbResult<()> {
        let mut store = self.store.write().expect("mock store");
        if let Some(stored_state) = &store.observer_state {
            if stored_state.metadata.genesis_hash != metadata.genesis_hash {
                return Err(BitcoinObserverConsistencyError::NetworkMismatch.into());
            }
            if stored_state.metadata.params_hash != metadata.params_hash {
                return Err(BitcoinObserverConsistencyError::ParamsMismatch.into());
            }
        } else {
            store.observer_state = Some(BitcoinObserverState {
                metadata: metadata.clone(),
                last_processed_block: anchor,
                revision: 0,
            });
        }
        Ok(())
    }

    fn get_deposit(&self, outpoint: OutPoint) -> DbResult<Option<ObservedDeposit>> {
        Ok(self
            .store
            .read()
            .expect("mock store")
            .rows
            .get(&outpoint)
            .cloned())
    }

    fn get_deposits_from(
        &self,
        after: Option<OutPoint>,
        limit: usize,
    ) -> DbResult<Vec<ObservedDeposit>> {
        let store = self.store.read().expect("mock store");
        let after = after.map(|outpoint| serialize(&outpoint));
        let mut rows = store
            .rows
            .values()
            .filter(|row| {
                after
                    .as_ref()
                    .is_none_or(|after| serialize(&row.outpoint()) > *after)
            })
            .cloned()
            .collect::<Vec<_>>();
        rows.sort_by_cached_key(|row| serialize(&row.outpoint()));
        rows.truncate(limit);
        Ok(rows)
    }

    fn rollback_history(&self, limit: u32) -> DbResult<Vec<Option<L1BlockCommitment>>> {
        let store = self.store.read().expect("mock store");
        let mut history = vec![
            store
                .observer_state
                .as_ref()
                .ok_or(BitcoinObserverConsistencyError::ObserverStateMismatch)?
                .last_processed_block,
        ];
        history.extend(
            store
                .undo
                .iter()
                .rev()
                .take(limit as usize)
                .map(|undo| undo.parent),
        );
        Ok(history)
    }

    fn apply_block(
        &self,
        parent: Option<L1BlockCommitment>,
        block: L1BlockCommitment,
        rows: &[ObservedDeposit],
        window: u32,
    ) -> DbResult<()> {
        let mut store = self.store.write().expect("mock store");
        let stored_state = store
            .observer_state
            .as_ref()
            .ok_or(BitcoinObserverConsistencyError::ObserverStateMismatch)?;
        if stored_state.last_processed_block == Some(block) {
            let record = store
                .undo
                .last()
                .ok_or(BitcoinObserverConsistencyError::MissingUndo)?;
            let replay_outpoints = rows
                .iter()
                .map(ObservedDeposit::outpoint)
                .collect::<BTreeSet<_>>();
            if record.parent != parent
                || rows.len() != replay_outpoints.len()
                || record.updated_outpoints != replay_outpoints
                || rows
                    .iter()
                    .any(|row| store.rows.get(&row.outpoint()) != Some(row))
            {
                return Err(BitcoinObserverConsistencyError::ConflictingBlockReplay(block).into());
            }
            return Ok(());
        }
        if stored_state.last_processed_block != parent
            || parent.map_or(Some(0), |p| p.height.checked_add(1)) != Some(block.height)
            || window == 0
        {
            return Err(BitcoinObserverConsistencyError::ObserverStateMismatch.into());
        }
        let keys = rows
            .iter()
            .map(ObservedDeposit::outpoint)
            .collect::<BTreeSet<_>>();
        if keys.len() != rows.len() {
            return Err(BitcoinObserverConsistencyError::ObserverStateMismatch.into());
        }
        let revision = stored_state
            .revision
            .checked_add(1)
            .ok_or(BitcoinObserverConsistencyError::RevisionOverflow)?;
        let previous = store.rows.clone();
        store.undo.push(MockBlockUndo {
            parent,
            rows_before: previous,
            updated_outpoints: keys,
        });
        if store.undo.len() > window as usize {
            store.undo.remove(0);
        }
        for row in rows {
            store.rows.insert(row.outpoint(), row.clone());
        }
        store
            .observer_state
            .as_mut()
            .expect("initialized observer_state")
            .last_processed_block = Some(block);
        store
            .observer_state
            .as_mut()
            .expect("initialized observer state")
            .revision = revision;
        Ok(())
    }

    fn rollback_tip(&self, expected: L1BlockCommitment) -> DbResult<()> {
        let mut store = self.store.write().expect("mock store");
        if store
            .observer_state
            .as_ref()
            .ok_or(BitcoinObserverConsistencyError::ObserverStateMismatch)?
            .last_processed_block
            != Some(expected)
        {
            return Err(BitcoinObserverConsistencyError::ObserverStateMismatch.into());
        }
        let revision = store
            .observer_state
            .as_ref()
            .expect("initialized observer state")
            .revision
            .checked_add(1)
            .ok_or(BitcoinObserverConsistencyError::RevisionOverflow)?;
        let undo = store
            .undo
            .pop()
            .ok_or(BitcoinObserverConsistencyError::MissingUndo)?;
        store.rows = undo.rows_before;
        store
            .observer_state
            .as_mut()
            .expect("initialized observer_state")
            .last_processed_block = undo.parent;
        store
            .observer_state
            .as_mut()
            .expect("initialized observer state")
            .revision = revision;
        Ok(())
    }
}
