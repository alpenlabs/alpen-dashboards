use std::collections::{BTreeMap, BTreeSet};

use bitcoin::ScriptBuf;
use strata_bridge_primitives::types::DepositIdx;
use tokio::sync::RwLock;
use tracing::warn;

use super::{
    cache::BridgeStatusCache,
    db::{
        error::DbResult,
        traits::BridgeStatusDb,
        types::{DbBridgeStatusSnapshot, DbWithdrawalRequestRow},
    },
    types::{
        BridgeStatus, DepositInfo, DepositStatus, OperatorStatus, ReimbursementInfo,
        ReimbursementStatus, ReimbursementStatusCursor, WithdrawalInfo, WithdrawalPairingCursor,
        WithdrawalSeq, WithdrawalStatus, WithdrawalStatusCursor,
    },
};

/// A bridge fulfillment resolved for one deposit index.
///
/// The fulfillment is identified by the outputs it paid, so it can be correlated
/// to the indexed withdrawal request that targets the same destination for the
/// same amount. `deposit_idx` is the bridge-side index the fulfillment was
/// queried under. `paid_outputs` holds every output of the fulfillment tx (the
/// destination payment plus any SPS-50 OP_RETURN / change), so the matcher can
/// pick whichever output corresponds to a withdrawal request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WithdrawalFulfillment {
    pub(crate) deposit_idx: DepositIdx,
    pub(crate) paid_outputs: Vec<WithdrawalMatchKey>,
}

/// Correlation key joining a fulfillment to a withdrawal request: the paid
/// destination scriptPubKey plus the amount in satoshis.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct WithdrawalMatchKey {
    pub(crate) script_pubkey: ScriptBuf,
    pub(crate) amount_sats: u64,
}

/// Deposit status update collected during one monitoring tick.
///
/// The update carries the cache key separately because [`DepositInfo`] is the
/// dashboard row value and does not include its bridge deposit index.
#[derive(Debug)]
pub(crate) struct DepositInfoUpdate {
    pub(crate) deposit_idx: DepositIdx,
    pub(crate) info: DepositInfo,
    pub(crate) confirmations: Option<u64>,
}

/// Paired withdrawal candidate whose bridge status can be queried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WithdrawalStatusCandidate {
    pub(crate) deposit_idx: DepositIdx,
    pub(crate) withdrawal_seq: WithdrawalSeq,
}

/// Withdrawal status update collected during one monitoring tick.
#[derive(Debug)]
pub(crate) struct WithdrawalInfoUpdate {
    pub(crate) deposit_idx: DepositIdx,
    pub(crate) info: WithdrawalInfo,
    pub(crate) confirmations: Option<u64>,
}

/// Reimbursement status update collected during one monitoring tick.
#[derive(Debug)]
pub(crate) struct ReimbursementInfoUpdate {
    pub(crate) deposit_idx: DepositIdx,
    pub(crate) info: ReimbursementInfo,
    pub(crate) confirmations: Option<u64>,
}

/// Mutable bridge monitoring state shared by the polling task and HTTP handler.
#[derive(Debug, Default)]
pub(crate) struct BridgeMonitoringState {
    cache: RwLock<BridgeStatusCache>,
}

impl BridgeMonitoringState {
    pub(crate) fn from_snapshot(snapshot: DbBridgeStatusSnapshot) -> Self {
        Self {
            cache: RwLock::new(cache_from_status_snapshot(snapshot)),
        }
    }

    pub(crate) async fn select_deposit_info_candidates(
        &self,
        deposit_indices: &[DepositIdx],
    ) -> Vec<DepositIdx> {
        let cache = self.cache.read().await;
        let deposit_info_cursor = cache.deposit_info_cursor();
        deposit_indices
            .iter()
            .copied()
            .filter(|deposit_idx| *deposit_idx >= deposit_info_cursor)
            .collect()
    }

    pub(crate) async fn apply_deposit_info_updates(
        &self,
        status_db: &impl BridgeStatusDb,
        updates: Vec<DepositInfoUpdate>,
        max_confirmations: u64,
    ) -> DbResult<()> {
        let mut cache_updates = Vec::new();
        let mut terminal_deposit_indices_to_purge = Vec::new();

        for update in updates {
            match update.info.status {
                DepositStatus::InProgress => {
                    cache_updates.push((update.deposit_idx, update.info, None));
                }
                DepositStatus::Failed | DepositStatus::Complete => {
                    let Some(confirmations) = update.confirmations else {
                        continue;
                    };

                    if confirmations >= max_confirmations {
                        terminal_deposit_indices_to_purge.push(update.deposit_idx);
                    } else {
                        cache_updates.push((update.deposit_idx, update.info, Some(confirmations)));
                    }
                }
            }
        }

        let current_cursor = {
            let cache = self.cache.read().await;
            cache.deposit_info_cursor()
        };
        let next_cursor =
            next_deposit_info_cursor(current_cursor, &terminal_deposit_indices_to_purge);
        status_db.put_deposit_info_cursor(next_cursor)?;

        let mut cache = self.cache.write().await;
        cache.apply_deposit_updates(cache_updates);
        cache.purge_deposits(terminal_deposit_indices_to_purge);
        cache.set_deposit_info_cursor(next_cursor);
        Ok(())
    }

    pub(crate) async fn withdrawal_pairing_cursor(&self) -> WithdrawalPairingCursor {
        let cache = self.cache.read().await;
        cache.withdrawal_pairing_cursor()
    }

    pub(crate) async fn apply_withdrawal_pairings(
        &self,
        status_db: &impl BridgeStatusDb,
        fulfillments: &[WithdrawalFulfillment],
        withdrawal_requests: &[DbWithdrawalRequestRow],
    ) -> DbResult<Vec<(DepositIdx, WithdrawalSeq)>> {
        let current_cursor = {
            let cache = self.cache.read().await;
            cache.withdrawal_pairing_cursor()
        };
        let (pairings, next_cursor) =
            plan_withdrawal_pairings(current_cursor, fulfillments, withdrawal_requests);
        if pairings.is_empty() {
            return Ok(pairings);
        }

        status_db.put_withdrawal_pairings(&pairings)?;
        status_db.put_withdrawal_pairing_cursor(next_cursor)?;

        let mut cache = self.cache.write().await;
        cache.update_withdrawal_pairings(&pairings, next_cursor);
        Ok(pairings)
    }

    pub(crate) async fn select_withdrawal_status_candidates(
        &self,
    ) -> Vec<WithdrawalStatusCandidate> {
        let cache = self.cache.read().await;
        let cursor = cache.withdrawal_status_cursor().next_deposit_idx;
        cache
            .withdrawal_pairings_from(cursor)
            .into_iter()
            .map(|(deposit_idx, withdrawal_seq)| WithdrawalStatusCandidate {
                deposit_idx,
                withdrawal_seq,
            })
            .collect()
    }

    pub(crate) async fn update_operators(&self, operators: Vec<OperatorStatus>) {
        let mut cache = self.cache.write().await;
        cache.update_operators(operators);
    }

    pub(crate) async fn apply_withdrawal_updates(
        &self,
        status_db: &impl BridgeStatusDb,
        updates: Vec<WithdrawalInfoUpdate>,
        max_confirmations: u64,
    ) -> DbResult<()> {
        let mut cache_updates = Vec::new();
        let mut withdrawal_infos_to_persist = Vec::new();
        let mut terminal_deposit_indices_to_purge = Vec::new();

        for update in updates {
            match update.info.status {
                WithdrawalStatus::InProgress => {
                    cache_updates.push((update.deposit_idx, update.info, None));
                }
                WithdrawalStatus::Complete => {
                    let Some(confirmations) = update.confirmations else {
                        continue;
                    };

                    if confirmations >= max_confirmations {
                        terminal_deposit_indices_to_purge.push(update.deposit_idx);
                    }
                    withdrawal_infos_to_persist.push((update.deposit_idx, update.info));
                    cache_updates.push((update.deposit_idx, update.info, Some(confirmations)));
                }
            }
        }

        let current_cursor = {
            let cache = self.cache.read().await;
            cache.withdrawal_status_cursor()
        };
        let next_cursor =
            next_withdrawal_status_cursor(current_cursor, &terminal_deposit_indices_to_purge);
        for (deposit_idx, info) in &withdrawal_infos_to_persist {
            status_db.put_withdrawal_info(*deposit_idx, info)?;
        }
        status_db.put_withdrawal_status_cursor(next_cursor)?;

        let pairing_purge_frontier = next_cursor.next_deposit_idx;
        let pairings_purged =
            match status_db.del_withdrawal_pairings_range(0, pairing_purge_frontier) {
                Ok(()) => true,
                Err(e) => {
                    warn!(error = %e, "failed to purge old withdrawal pairings");
                    false
                }
            };

        let mut cache = self.cache.write().await;
        cache.apply_withdrawal_updates(cache_updates);
        if pairings_purged {
            cache.purge_withdrawal_pairings_range(0, pairing_purge_frontier);
        }
        cache.set_withdrawal_status_cursor(next_cursor);
        Ok(())
    }

    pub(crate) async fn select_reimbursement_status_candidates(&self) -> Vec<DepositIdx> {
        let cache = self.cache.read().await;
        let reimbursement_cursor = cache.reimbursement_status_cursor().next_deposit_idx;
        let withdrawal_cursor = cache.withdrawal_status_cursor().next_deposit_idx;
        let mut candidates = (reimbursement_cursor..withdrawal_cursor).collect::<BTreeSet<_>>();

        candidates.extend(
            cache
                .filter_withdrawals(|deposit_idx, info, _| {
                    deposit_idx >= reimbursement_cursor
                        && matches!(info.status, WithdrawalStatus::Complete)
                })
                .into_iter()
                .map(|(deposit_idx, _)| deposit_idx),
        );

        candidates.into_iter().collect()
    }

    pub(crate) async fn apply_reimbursement_updates(
        &self,
        status_db: &impl BridgeStatusDb,
        updates: Vec<ReimbursementInfoUpdate>,
        max_confirmations: u64,
    ) -> DbResult<()> {
        let mut cache_updates = Vec::new();
        let mut terminal_deposit_indices_to_purge = Vec::new();

        for update in updates {
            match update.info.status {
                ReimbursementStatus::NotStarted => continue,
                ReimbursementStatus::InProgress => {
                    cache_updates.push((update.deposit_idx, update.info, None));
                }
                ReimbursementStatus::Slashed
                | ReimbursementStatus::Aborted
                | ReimbursementStatus::Complete => {
                    let Some(confirmations) = update.confirmations else {
                        continue;
                    };

                    if confirmations >= max_confirmations {
                        terminal_deposit_indices_to_purge.push(update.deposit_idx);
                    } else {
                        cache_updates.push((update.deposit_idx, update.info, Some(confirmations)));
                    }
                }
            }
        }

        let (next_cursor, withdrawal_deposit_indices_to_purge) = {
            let cache = self.cache.read().await;
            let current_cursor = cache.reimbursement_status_cursor();
            let next_cursor = next_reimbursement_status_cursor(
                current_cursor,
                &terminal_deposit_indices_to_purge,
            );
            (
                next_cursor,
                cache
                    .filter_withdrawals(|deposit_idx, _, _| {
                        deposit_idx < next_cursor.next_deposit_idx
                    })
                    .into_iter()
                    .map(|(deposit_idx, _)| deposit_idx)
                    .collect::<Vec<_>>(),
            )
        };

        status_db.put_reimbursement_status_cursor(next_cursor)?;

        let mut purged_withdrawal_deposit_indices = Vec::new();
        for deposit_idx in withdrawal_deposit_indices_to_purge {
            match status_db.del_withdrawal_info(deposit_idx) {
                Ok(_) => purged_withdrawal_deposit_indices.push(deposit_idx),
                Err(e) => {
                    warn!(deposit_idx, error = %e, "failed to purge old withdrawal row");
                }
            }
        }

        let mut cache = self.cache.write().await;
        cache.apply_reimbursement_updates(cache_updates);
        cache.purge_reimbursements(terminal_deposit_indices_to_purge);
        cache.purge_withdrawals(purged_withdrawal_deposit_indices);
        cache.set_reimbursement_status_cursor(next_cursor);
        Ok(())
    }

    pub(crate) async fn bridge_status(&self, max_confirmations: u64) -> BridgeStatus {
        let cache = self.cache.read().await;

        BridgeStatus {
            operators: cache.get_operators(),
            deposits: cache
                // Omit terminal rows whose confirmations reached `max_confirmations`.
                .filter_deposits(|_, info, confirmations| {
                    !matches!(info.status, DepositStatus::Complete | DepositStatus::Failed)
                        || confirmations
                            .is_none_or(|confirmations| confirmations < max_confirmations)
                })
                .into_iter()
                .map(|(_, info)| info)
                .collect(),
            withdrawals: cache
                // Complete withdrawals may be retained as reimbursement
                // handoff state; omit them from this response at `max_confirmations`.
                .filter_withdrawals(|_, info, confirmations| {
                    !matches!(info.status, WithdrawalStatus::Complete)
                        || confirmations
                            .is_none_or(|confirmations| confirmations < max_confirmations)
                })
                .into_iter()
                .map(|(_, info)| info)
                .collect(),
            reimbursements: cache
                // Omit terminal rows whose confirmations reached `max_confirmations`.
                .filter_reimbursements(|_, info, confirmations| {
                    !matches!(
                        info.status,
                        ReimbursementStatus::Complete
                            | ReimbursementStatus::Slashed
                            | ReimbursementStatus::Aborted
                    ) || confirmations.is_none_or(|confirmations| confirmations < max_confirmations)
                })
                .into_iter()
                .map(|(_, info)| info)
                .collect(),
        }
    }
}

fn cache_from_status_snapshot(snapshot: DbBridgeStatusSnapshot) -> BridgeStatusCache {
    let mut cache = BridgeStatusCache::default();

    cache.apply_withdrawal_updates(
        snapshot
            .withdrawals
            .into_iter()
            .map(|(deposit_idx, info)| (deposit_idx, info, None))
            .collect(),
    );
    cache.update_withdrawal_pairings(
        &snapshot.withdrawal_pairings,
        snapshot.cursors.withdrawal_pairing,
    );
    cache.set_deposit_info_cursor(snapshot.cursors.deposit_info);
    cache.set_withdrawal_status_cursor(snapshot.cursors.withdrawal_status);
    cache.set_reimbursement_status_cursor(snapshot.cursors.reimbursement_status);

    cache
}

fn next_deposit_info_cursor(
    current_cursor: DepositIdx,
    terminal_deposit_indices_to_purge: &[DepositIdx],
) -> DepositIdx {
    let terminal_deposit_indices_to_purge = terminal_deposit_indices_to_purge
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let mut next_cursor = current_cursor;

    while terminal_deposit_indices_to_purge.contains(&next_cursor) {
        let Some(next) = next_cursor.checked_add(1) else {
            break;
        };
        next_cursor = next;
    }

    next_cursor
}

fn next_withdrawal_status_cursor(
    current_cursor: WithdrawalStatusCursor,
    terminal_deposit_indices_to_purge: &[DepositIdx],
) -> WithdrawalStatusCursor {
    let terminal_deposit_indices_to_purge = terminal_deposit_indices_to_purge
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let mut next_cursor = current_cursor.next_deposit_idx;

    while terminal_deposit_indices_to_purge.contains(&next_cursor) {
        let Some(next) = next_cursor.checked_add(1) else {
            break;
        };
        next_cursor = next;
    }

    WithdrawalStatusCursor {
        next_deposit_idx: next_cursor,
    }
}

fn next_reimbursement_status_cursor(
    current_cursor: ReimbursementStatusCursor,
    terminal_deposit_indices_to_purge: &[DepositIdx],
) -> ReimbursementStatusCursor {
    let terminal_deposit_indices_to_purge = terminal_deposit_indices_to_purge
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let mut next_cursor = current_cursor.next_deposit_idx;

    while terminal_deposit_indices_to_purge.contains(&next_cursor) {
        let Some(next) = next_cursor.checked_add(1) else {
            break;
        };
        next_cursor = next;
    }

    ReimbursementStatusCursor {
        next_deposit_idx: next_cursor,
    }
}

/// Build the correlation key (paid scriptPubKey + amount) for an indexed
/// withdrawal request from its BOSD `destination` descriptor.
///
/// Returns `None` if the stored descriptor bytes don't decode to a BOSD
/// descriptor; such a row can't be correlated and is skipped.
fn request_match_key(request: &DbWithdrawalRequestRow) -> Option<WithdrawalMatchKey> {
    let descriptor = bitcoin_bosd::Descriptor::from_bytes(&request.request.destination).ok()?;
    Some(WithdrawalMatchKey {
        script_pubkey: descriptor.to_script(),
        amount_sats: request.request.amount_sats,
    })
}

/// Plan withdrawal-to-deposit pairings by correlating bridge fulfillments to
/// indexed withdrawal requests on the **destination scriptPubKey + amount**, not
/// on FIFO array position.
///
/// A `WithdrawalIntentEvent` carries no deposit index, and there is no EVM↔BTC
/// txid reference, so the only correlatable key is the destination the bridge
/// paid out to (which equals the request's BOSD `destination.to_script()`) for
/// the matching amount.
///
/// The pairing cursor still advances over the contiguous prefix of resolved
/// deposit indices so the persisted cursor/GC model keeps working:
///
/// - `fulfillments` are the deposit indices at and above `cursor.next_deposit_idx`
///   whose bridge status was resolvable, in ascending `deposit_idx` order. The
///   caller stops at the first unresolved deposit, so a `None`/missing
///   fulfillment for a deposit leaves it (and everything after it) pending.
/// - Each fulfillment is matched to the lowest-`seq` not-yet-consumed request
///   whose `(scriptPubKey, amount)` equals the fulfillment's. Multiple
///   single-denom rows from one multi-`sub_idx` event share a key, so amount +
///   stable seq ordering disambiguates them; each request is consumed once, so
///   one fulfillment maps to exactly one request (dedup).
/// - `next_deposit_idx` advances past every contiguous deposit that produced a
///   match. `next_withdrawal_seq` advances to the lowest seq that was not
///   consumed, so unmatched lower seqs stay candidates on the next tick.
fn plan_withdrawal_pairings(
    cursor: WithdrawalPairingCursor,
    fulfillments: &[WithdrawalFulfillment],
    withdrawal_requests: &[DbWithdrawalRequestRow],
) -> (Vec<(DepositIdx, WithdrawalSeq)>, WithdrawalPairingCursor) {
    // Group candidate request seqs by their correlation key, ordered by seq so
    // matching is FIFO *within* a destination+amount group (not across the whole
    // stream). Only consider requests at or above the seq low-water mark.
    let mut requests_by_key: BTreeMap<WithdrawalMatchKey, Vec<WithdrawalSeq>> = BTreeMap::new();
    for request in withdrawal_requests {
        if request.seq < cursor.next_withdrawal_seq {
            continue;
        }
        let Some(key) = request_match_key(request) else {
            warn!(
                seq = request.seq,
                "indexed withdrawal request has an undecodable BOSD destination"
            );
            continue;
        };
        requests_by_key.entry(key).or_default().push(request.seq);
    }
    for seqs in requests_by_key.values_mut() {
        seqs.sort_unstable();
    }

    // Index resolvable fulfillments by deposit index for the contiguous walk.
    let fulfillments_by_idx = fulfillments
        .iter()
        .map(|fulfillment| (fulfillment.deposit_idx, fulfillment))
        .collect::<BTreeMap<_, _>>();

    let mut next_deposit_idx = cursor.next_deposit_idx;
    let mut consumed_seqs = BTreeSet::new();
    let mut pairings = Vec::new();

    // Walk the contiguous prefix of resolved deposit indices. Stop at the first
    // deposit that is unresolved or has no matching request: it (and everything
    // after it) stays pending until a later tick.
    while let Some(fulfillment) = fulfillments_by_idx.get(&next_deposit_idx) {
        // Try each paid output: the destination payment matches a request key;
        // SPS-50 OP_RETURN / change outputs simply won't match any request.
        let Some(seq) = fulfillment.paid_outputs.iter().find_map(|key| {
            requests_by_key.get(key).and_then(|seqs| {
                seqs.iter()
                    .copied()
                    .find(|seq| !consumed_seqs.contains(seq))
            })
        }) else {
            break;
        };

        consumed_seqs.insert(seq);
        pairings.push((next_deposit_idx, seq));

        let Some(next) = next_deposit_idx.checked_add(1) else {
            break;
        };
        next_deposit_idx = next;
    }

    // Advance the seq low-water mark to the lowest seq not consumed this round,
    // so an unmatched lower seq remains a candidate next tick while consumed
    // seqs aren't re-fetched.
    let next_withdrawal_seq = withdrawal_requests
        .iter()
        .map(|request| request.seq)
        .filter(|seq| *seq >= cursor.next_withdrawal_seq && !consumed_seqs.contains(seq))
        .min()
        .unwrap_or_else(|| {
            // Every candidate was consumed (or there were none): move past the
            // highest consumed seq, else hold the cursor.
            consumed_seqs
                .iter()
                .copied()
                .max()
                .and_then(|seq| seq.checked_add(1))
                .unwrap_or(cursor.next_withdrawal_seq)
        });

    let next_cursor = WithdrawalPairingCursor {
        next_deposit_idx,
        next_withdrawal_seq,
    };

    (pairings, next_cursor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::{hashes::Hash, Txid};
    use strata_primitives::buf::Buf32;

    use crate::db::{traits::BridgeStatusDb, types::DbWithdrawalRequest, BridgeStatusDbSled};

    /// Build a withdrawal request row whose `destination` is a valid BOSD P2WPKH
    /// descriptor derived from `seed`, paired with `amount_sats`. Returns the row
    /// alongside the `WithdrawalMatchKey` (scriptPubKey + amount) a fulfillment
    /// paying this destination would produce, so tests can correlate the two
    /// without depending on FIFO position.
    fn request_with_destination(
        seq: WithdrawalSeq,
        seed: u8,
        amount_sats: u64,
    ) -> (DbWithdrawalRequestRow, WithdrawalMatchKey) {
        let descriptor = bitcoin_bosd::Descriptor::new_p2wpkh(&[seed; 20]);
        let destination = descriptor.to_bytes();
        let key = WithdrawalMatchKey {
            script_pubkey: descriptor.to_script(),
            amount_sats,
        };
        let row = DbWithdrawalRequestRow {
            seq,
            request: DbWithdrawalRequest {
                tx_hash: Buf32([seed; 32]),
                log_index: u64::from(seed),
                sub_idx: 0,
                amount_sats,
                destination,
                selected_operator: 0,
                block_number: 1_000 + u64::from(seed),
            },
        };
        (row, key)
    }

    /// Build a fulfillment whose single paid output matches `key`, queried under
    /// `deposit_idx`.
    fn fulfillment(deposit_idx: DepositIdx, key: &WithdrawalMatchKey) -> WithdrawalFulfillment {
        WithdrawalFulfillment {
            deposit_idx,
            paid_outputs: vec![key.clone()],
        }
    }

    #[test]
    fn deposit_info_cursor_advances_over_contiguous_purged_deposits() {
        assert_eq!(next_deposit_info_cursor(0, &[0, 1, 3]), 2);
        assert_eq!(next_deposit_info_cursor(2, &[3, 4]), 2);
        assert_eq!(next_deposit_info_cursor(2, &[2, 3, 4]), 5);
    }

    #[test]
    fn withdrawal_status_cursor_advances_over_contiguous_purged_deposits() {
        assert_eq!(
            next_withdrawal_status_cursor(
                WithdrawalStatusCursor {
                    next_deposit_idx: 0
                },
                &[2]
            ),
            WithdrawalStatusCursor {
                next_deposit_idx: 0
            }
        );
        assert_eq!(
            next_withdrawal_status_cursor(
                WithdrawalStatusCursor {
                    next_deposit_idx: 0
                },
                &[0, 1]
            ),
            WithdrawalStatusCursor {
                next_deposit_idx: 2
            }
        );
    }

    #[test]
    fn reimbursement_status_cursor_advances_over_contiguous_purged_deposits() {
        assert_eq!(
            next_reimbursement_status_cursor(
                ReimbursementStatusCursor {
                    next_deposit_idx: 0
                },
                &[2]
            ),
            ReimbursementStatusCursor {
                next_deposit_idx: 0
            }
        );
        assert_eq!(
            next_reimbursement_status_cursor(
                ReimbursementStatusCursor {
                    next_deposit_idx: 0
                },
                &[0, 1]
            ),
            ReimbursementStatusCursor {
                next_deposit_idx: 2
            }
        );
    }

    #[test]
    fn withdrawal_pairing_planner_matches_on_destination_not_position() {
        // Two requests whose seq order is the REVERSE of the deposit-index order
        // they get fulfilled in. Old FIFO logic would emit (0,0) and (1,1),
        // mispairing each request with the wrong deposit. Correct matching keys
        // on destination+amount, so deposit 0 (which paid req B's destination)
        // must pair with seq 1, and deposit 1 with seq 0.
        let (req_a, key_a) = request_with_destination(0, 0xA1, 100_000);
        let (req_b, key_b) = request_with_destination(1, 0xB2, 200_000);

        let fulfillments = vec![fulfillment(0, &key_b), fulfillment(1, &key_a)];
        let (pairings, cursor) = plan_withdrawal_pairings(
            WithdrawalPairingCursor::default(),
            &fulfillments,
            &[req_a, req_b],
        );

        assert_eq!(pairings, vec![(0, 1), (1, 0)]);
        assert_eq!(
            cursor,
            WithdrawalPairingCursor {
                next_deposit_idx: 2,
                next_withdrawal_seq: 2,
            }
        );
    }

    #[test]
    fn withdrawal_pairing_planner_disambiguates_multi_sub_idx_by_seq() {
        // One multi-sub_idx event: three single-denom rows sharing destination
        // AND amount, with consecutive seqs. Each of the three fulfillments must
        // map to a DISTINCT request (dedup), assigned by stable seq order.
        let descriptor = bitcoin_bosd::Descriptor::new_p2wpkh(&[0xCD; 20]);
        let destination = descriptor.to_bytes();
        let key = WithdrawalMatchKey {
            script_pubkey: descriptor.to_script(),
            amount_sats: 100_000,
        };
        let rows = (0..3)
            .map(|sub_idx| DbWithdrawalRequestRow {
                seq: u64::from(sub_idx),
                request: DbWithdrawalRequest {
                    tx_hash: Buf32([7; 32]),
                    log_index: 5,
                    sub_idx,
                    amount_sats: 100_000,
                    destination: destination.clone(),
                    selected_operator: 0,
                    block_number: 1_000,
                },
            })
            .collect::<Vec<_>>();

        let fulfillments = vec![
            fulfillment(0, &key),
            fulfillment(1, &key),
            fulfillment(2, &key),
        ];
        let (pairings, cursor) =
            plan_withdrawal_pairings(WithdrawalPairingCursor::default(), &fulfillments, &rows);

        // Distinct seqs, no duplicates, FIFO within the shared-key group.
        assert_eq!(pairings, vec![(0, 0), (1, 1), (2, 2)]);
        assert_eq!(
            cursor,
            WithdrawalPairingCursor {
                next_deposit_idx: 3,
                next_withdrawal_seq: 3,
            }
        );
    }

    #[test]
    fn withdrawal_pairing_planner_leaves_unmatched_deposit_pending() {
        // Deposit 0 has a fulfillment but no request matches its destination, so
        // it (and the contiguous walk) stalls: no pairing, cursor unchanged.
        let (_req_a, key_a) = request_with_destination(0, 0xA1, 100_000);
        let (req_b, _key_b) = request_with_destination(0, 0xB2, 200_000);

        let fulfillments = vec![fulfillment(0, &key_a)];
        let (pairings, cursor) =
            plan_withdrawal_pairings(WithdrawalPairingCursor::default(), &fulfillments, &[req_b]);

        assert!(pairings.is_empty());
        assert_eq!(cursor, WithdrawalPairingCursor::default());
    }

    #[test]
    fn withdrawal_pairing_planner_stops_without_fulfillment() {
        let (req_a, _key_a) = request_with_destination(0, 0xA1, 100_000);
        let (pairings, cursor) =
            plan_withdrawal_pairings(WithdrawalPairingCursor::default(), &[], &[req_a]);

        assert!(pairings.is_empty());
        assert_eq!(cursor, WithdrawalPairingCursor::default());
    }

    #[test]
    fn withdrawal_pairing_planner_does_not_repair_old_indices() {
        let cursor = WithdrawalPairingCursor {
            next_deposit_idx: 2,
            next_withdrawal_seq: 2,
        };
        // Fulfillments/requests below the cursor must not be re-paired.
        let (req_a, key_a) = request_with_destination(0, 0xA1, 100_000);
        let (req_b, key_b) = request_with_destination(1, 0xB2, 200_000);
        let fulfillments = vec![fulfillment(0, &key_a), fulfillment(1, &key_b)];
        let (pairings, next_cursor) =
            plan_withdrawal_pairings(cursor, &fulfillments, &[req_a, req_b]);

        assert!(pairings.is_empty());
        assert_eq!(next_cursor, cursor);
    }

    #[tokio::test]
    async fn hydrates_persisted_status_snapshot() {
        let status_db = BridgeStatusDbSled::open_temporary().expect("open status db");
        let withdrawal_info = WithdrawalInfo {
            withdrawal_request_txid: Buf32([2; 32]),
            fulfillment_txid: Some(Txid::from_byte_array([4; 32])),
            status: WithdrawalStatus::Complete,
        };

        status_db
            .put_withdrawal_info(2, &withdrawal_info)
            .expect("put withdrawal info");
        status_db
            .put_withdrawal_pairings(&[(2, 7)])
            .expect("put withdrawal pairing");
        status_db
            .put_deposit_info_cursor(2)
            .expect("put deposit cursor");
        status_db
            .put_withdrawal_pairing_cursor(WithdrawalPairingCursor {
                next_deposit_idx: 3,
                next_withdrawal_seq: 8,
            })
            .expect("put pairing cursor");
        status_db
            .put_withdrawal_status_cursor(WithdrawalStatusCursor {
                next_deposit_idx: 3,
            })
            .expect("put withdrawal cursor");
        status_db
            .put_reimbursement_status_cursor(ReimbursementStatusCursor {
                next_deposit_idx: 2,
            })
            .expect("put reimbursement cursor");

        let snapshot = status_db
            .get_status_snapshot()
            .expect("load status snapshot");
        let state = BridgeMonitoringState::from_snapshot(snapshot);

        assert_eq!(
            state.withdrawal_pairing_cursor().await,
            WithdrawalPairingCursor {
                next_deposit_idx: 3,
                next_withdrawal_seq: 8,
            }
        );
        assert_eq!(
            state.select_deposit_info_candidates(&[0, 1, 2]).await,
            vec![2]
        );
        assert_eq!(
            state.select_withdrawal_status_candidates().await,
            Vec::<WithdrawalStatusCandidate>::new()
        );
        assert_eq!(
            state.select_reimbursement_status_candidates().await,
            vec![2]
        );

        let status = state.bridge_status(6).await;
        assert!(status.deposits.is_empty());
        assert_eq!(status.withdrawals.len(), 1);
        assert!(status.reimbursements.is_empty());
    }

    #[tokio::test]
    async fn deposit_updates_persist_cursor_only() {
        let status_db = BridgeStatusDbSled::open_temporary().expect("open status db");
        let state = BridgeMonitoringState::default();
        let deposit_request_txid = Txid::from_byte_array([6; 32]);
        let deposit_txid = Txid::from_byte_array([7; 32]);

        state
            .apply_deposit_info_updates(
                &status_db,
                vec![DepositInfoUpdate {
                    deposit_idx: 0,
                    info: DepositInfo {
                        deposit_request_txid,
                        deposit_txid: None,
                        status: DepositStatus::InProgress,
                    },
                    confirmations: None,
                }],
                6,
            )
            .await
            .expect("persist deposit cursor");

        let snapshot = status_db
            .get_status_snapshot()
            .expect("load status snapshot");
        assert_eq!(snapshot.cursors.deposit_info, 0);
        assert!(snapshot.withdrawals.is_empty());

        state
            .apply_deposit_info_updates(
                &status_db,
                vec![DepositInfoUpdate {
                    deposit_idx: 0,
                    info: DepositInfo {
                        deposit_request_txid,
                        deposit_txid: Some(deposit_txid),
                        status: DepositStatus::Complete,
                    },
                    confirmations: Some(6),
                }],
                6,
            )
            .await
            .expect("persist deposit cursor");

        let snapshot = status_db
            .get_status_snapshot()
            .expect("load status snapshot");
        assert_eq!(snapshot.cursors.deposit_info, 1);
        assert!(snapshot.withdrawals.is_empty());
        assert_eq!(state.select_deposit_info_candidates(&[0, 1]).await, vec![1]);
    }

    #[tokio::test]
    async fn withdrawal_pairings_persist_rows_and_cursor() {
        let status_db = BridgeStatusDbSled::open_temporary().expect("open status db");
        let state = BridgeMonitoringState::default();
        let (req_0, key_0) = request_with_destination(0, 0xA1, 100_000);
        let (req_1, key_1) = request_with_destination(1, 0xB2, 200_000);
        let requests = vec![req_0, req_1];
        let fulfillments = vec![fulfillment(0, &key_0), fulfillment(1, &key_1)];

        let pairings = state
            .apply_withdrawal_pairings(&status_db, &fulfillments, &requests)
            .await
            .expect("persist withdrawal pairings");

        assert_eq!(pairings, vec![(0, 0), (1, 1)]);
        assert_eq!(
            state.withdrawal_pairing_cursor().await,
            WithdrawalPairingCursor {
                next_deposit_idx: 2,
                next_withdrawal_seq: 2
            }
        );
        let snapshot = status_db
            .get_status_snapshot()
            .expect("load status snapshot");
        assert_eq!(snapshot.withdrawal_pairings, vec![(0, 0), (1, 1)]);
        assert_eq!(
            snapshot.cursors.withdrawal_pairing,
            WithdrawalPairingCursor {
                next_deposit_idx: 2,
                next_withdrawal_seq: 2
            }
        );
    }

    #[tokio::test]
    async fn withdrawal_status_candidates_follow_cursor() {
        let status_db = BridgeStatusDbSled::open_temporary().expect("open status db");
        let state = BridgeMonitoringState::default();
        let (req_0, key_0) = request_with_destination(0, 0xA1, 100_000);
        let (req_1, key_1) = request_with_destination(1, 0xB2, 200_000);
        let requests = vec![req_0, req_1];
        let fulfillments = vec![fulfillment(0, &key_0), fulfillment(1, &key_1)];

        state
            .apply_withdrawal_pairings(&status_db, &fulfillments, &requests)
            .await
            .expect("persist withdrawal pairings");
        assert_eq!(
            state.select_withdrawal_status_candidates().await,
            vec![
                WithdrawalStatusCandidate {
                    deposit_idx: 0,
                    withdrawal_seq: 0
                },
                WithdrawalStatusCandidate {
                    deposit_idx: 1,
                    withdrawal_seq: 1
                }
            ]
        );

        state
            .apply_withdrawal_updates(
                &status_db,
                vec![WithdrawalInfoUpdate {
                    deposit_idx: 0,
                    info: WithdrawalInfo {
                        withdrawal_request_txid: requests[0].request.tx_hash,
                        fulfillment_txid: Some(Txid::from_byte_array([3; 32])),
                        status: WithdrawalStatus::Complete,
                    },
                    confirmations: Some(6),
                }],
                6,
            )
            .await
            .expect("persist withdrawal status");

        assert_eq!(
            state.select_withdrawal_status_candidates().await,
            vec![WithdrawalStatusCandidate {
                deposit_idx: 1,
                withdrawal_seq: 1
            }]
        );
        let snapshot = status_db
            .get_status_snapshot()
            .expect("load status snapshot");
        assert_eq!(
            snapshot.withdrawal_pairings,
            vec![(1, 1)],
            "confirmed withdrawal pairings below the cursor are purged"
        );
        assert_eq!(
            snapshot.cursors.withdrawal_status,
            WithdrawalStatusCursor {
                next_deposit_idx: 1
            }
        );
        assert_eq!(snapshot.withdrawals.len(), 1);
        assert_eq!(snapshot.withdrawals[0].0, 0);
    }

    #[tokio::test]
    async fn pairing_gc_retries_below_cursor() {
        let status_db = BridgeStatusDbSled::open_temporary().expect("open status db");
        status_db
            .put_withdrawal_pairings(&[(0, 0), (1, 1)])
            .expect("put withdrawal pairings");
        status_db
            .put_withdrawal_status_cursor(WithdrawalStatusCursor {
                next_deposit_idx: 1,
            })
            .expect("put withdrawal status cursor");

        let snapshot = status_db
            .get_status_snapshot()
            .expect("load status snapshot");
        let state = BridgeMonitoringState::from_snapshot(snapshot);

        state
            .apply_withdrawal_updates(&status_db, Vec::new(), 6)
            .await
            .expect("persist withdrawal status");

        let snapshot = status_db
            .get_status_snapshot()
            .expect("load status snapshot");
        assert_eq!(snapshot.withdrawal_pairings, vec![(1, 1)]);
        assert_eq!(
            state.select_withdrawal_status_candidates().await,
            vec![WithdrawalStatusCandidate {
                deposit_idx: 1,
                withdrawal_seq: 1
            }]
        );
    }

    #[tokio::test]
    async fn withdrawal_cache_allows_shared_request_tx_hash() {
        let status_db = BridgeStatusDbSled::open_temporary().expect("open status db");
        let state = BridgeMonitoringState::default();
        let withdrawal_request_txid = Buf32([9; 32]);

        state
            .apply_withdrawal_updates(
                &status_db,
                vec![
                    WithdrawalInfoUpdate {
                        deposit_idx: 0,
                        info: WithdrawalInfo {
                            withdrawal_request_txid,
                            fulfillment_txid: None,
                            status: WithdrawalStatus::InProgress,
                        },
                        confirmations: None,
                    },
                    WithdrawalInfoUpdate {
                        deposit_idx: 1,
                        info: WithdrawalInfo {
                            withdrawal_request_txid,
                            fulfillment_txid: None,
                            status: WithdrawalStatus::InProgress,
                        },
                        confirmations: None,
                    },
                ],
                6,
            )
            .await
            .expect("persist withdrawal status");

        let status = state.bridge_status(6).await;

        assert_eq!(status.withdrawals.len(), 2);
        assert!(status
            .withdrawals
            .iter()
            .all(|info| info.withdrawal_request_txid == withdrawal_request_txid));
        assert!(
            status_db
                .get_status_snapshot()
                .expect("load status snapshot")
                .withdrawals
                .is_empty(),
            "in-progress withdrawal rows are cache-only"
        );
    }

    #[tokio::test]
    async fn reimbursement_candidates_use_complete_withdrawals() {
        let status_db = BridgeStatusDbSled::open_temporary().expect("open status db");
        let state = BridgeMonitoringState::default();
        let (req_0, key_0) = request_with_destination(0, 0xA1, 100_000);
        let requests = vec![req_0];
        let fulfillments = vec![fulfillment(0, &key_0)];

        assert_eq!(
            state.select_reimbursement_status_candidates().await,
            Vec::<DepositIdx>::new()
        );

        state
            .apply_withdrawal_pairings(&status_db, &fulfillments, &requests)
            .await
            .expect("persist withdrawal pairings");
        assert_eq!(
            state.select_reimbursement_status_candidates().await,
            Vec::<DepositIdx>::new()
        );

        state
            .apply_withdrawal_updates(
                &status_db,
                vec![WithdrawalInfoUpdate {
                    deposit_idx: 0,
                    info: WithdrawalInfo {
                        withdrawal_request_txid: requests[0].request.tx_hash,
                        fulfillment_txid: Some(Txid::from_byte_array([3; 32])),
                        status: WithdrawalStatus::Complete,
                    },
                    confirmations: Some(1),
                }],
                6,
            )
            .await
            .expect("persist withdrawal status");
        assert_eq!(
            status_db
                .get_status_snapshot()
                .expect("load status snapshot")
                .cursors
                .withdrawal_status,
            WithdrawalStatusCursor {
                next_deposit_idx: 0
            }
        );
        assert_eq!(
            state.select_reimbursement_status_candidates().await,
            vec![0]
        );
        assert_eq!(state.bridge_status(6).await.withdrawals.len(), 1);

        state
            .apply_withdrawal_updates(
                &status_db,
                vec![WithdrawalInfoUpdate {
                    deposit_idx: 0,
                    info: WithdrawalInfo {
                        withdrawal_request_txid: requests[0].request.tx_hash,
                        fulfillment_txid: Some(Txid::from_byte_array([3; 32])),
                        status: WithdrawalStatus::Complete,
                    },
                    confirmations: Some(6),
                }],
                6,
            )
            .await
            .expect("persist confirmed withdrawal status");
        assert_eq!(
            state.select_reimbursement_status_candidates().await,
            vec![0]
        );
        assert!(state.bridge_status(6).await.withdrawals.is_empty());

        state
            .apply_reimbursement_updates(
                &status_db,
                vec![ReimbursementInfoUpdate {
                    deposit_idx: 0,
                    info: ReimbursementInfo {
                        claim_txid: Txid::from_byte_array([4; 32]),
                        challenge_step: crate::types::ChallengeStep::Claimed,
                        payout_txid: None,
                        status: ReimbursementStatus::InProgress,
                    },
                    confirmations: None,
                }],
                6,
            )
            .await
            .expect("persist active reimbursement status");
        let snapshot = status_db
            .get_status_snapshot()
            .expect("load status snapshot");
        assert_eq!(
            snapshot.cursors.reimbursement_status,
            ReimbursementStatusCursor::default()
        );
        assert_eq!(snapshot.withdrawals.len(), 1);

        state
            .apply_reimbursement_updates(
                &status_db,
                vec![ReimbursementInfoUpdate {
                    deposit_idx: 0,
                    info: ReimbursementInfo {
                        claim_txid: Txid::from_byte_array([4; 32]),
                        challenge_step: crate::types::ChallengeStep::NotApplicable,
                        payout_txid: Some(Txid::from_byte_array([5; 32])),
                        status: ReimbursementStatus::Complete,
                    },
                    confirmations: Some(6),
                }],
                6,
            )
            .await
            .expect("persist reimbursement status");

        assert_eq!(
            state.select_reimbursement_status_candidates().await,
            Vec::<DepositIdx>::new()
        );
        let snapshot = status_db
            .get_status_snapshot()
            .expect("load status snapshot");
        assert_eq!(
            snapshot.cursors.reimbursement_status,
            ReimbursementStatusCursor {
                next_deposit_idx: 1
            }
        );
        assert!(snapshot.withdrawals.is_empty());
        assert!(state.bridge_status(6).await.withdrawals.is_empty());
    }

    #[tokio::test]
    async fn reimbursement_candidates_skip_incomplete_withdrawals() {
        let status_db = BridgeStatusDbSled::open_temporary().expect("open status db");
        let state = BridgeMonitoringState::default();

        state
            .apply_withdrawal_updates(
                &status_db,
                vec![
                    WithdrawalInfoUpdate {
                        deposit_idx: 0,
                        info: WithdrawalInfo {
                            withdrawal_request_txid: Buf32([10; 32]),
                            fulfillment_txid: None,
                            status: WithdrawalStatus::InProgress,
                        },
                        confirmations: None,
                    },
                    WithdrawalInfoUpdate {
                        deposit_idx: 1,
                        info: WithdrawalInfo {
                            withdrawal_request_txid: Buf32([11; 32]),
                            fulfillment_txid: Some(Txid::from_byte_array([12; 32])),
                            status: WithdrawalStatus::Complete,
                        },
                        confirmations: Some(1),
                    },
                ],
                6,
            )
            .await
            .expect("persist withdrawal status");

        assert_eq!(
            state.select_reimbursement_status_candidates().await,
            vec![1]
        );
    }

    #[tokio::test]
    async fn reimbursement_candidates_include_withdrawal_cursor_lag() {
        let status_db = BridgeStatusDbSled::open_temporary().expect("open status db");
        status_db
            .put_withdrawal_status_cursor(WithdrawalStatusCursor {
                next_deposit_idx: 3,
            })
            .expect("put withdrawal status cursor");
        status_db
            .put_reimbursement_status_cursor(ReimbursementStatusCursor {
                next_deposit_idx: 1,
            })
            .expect("put reimbursement status cursor");

        let snapshot = status_db
            .get_status_snapshot()
            .expect("load status snapshot");
        let state = BridgeMonitoringState::from_snapshot(snapshot);

        assert_eq!(
            state.select_reimbursement_status_candidates().await,
            vec![1, 2]
        );
    }
}
