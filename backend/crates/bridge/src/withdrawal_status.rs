use std::collections::BTreeMap;

use strata_primitives::L1Height;
use tracing::warn;

use super::{
    bridge_rpc::{self, RpcClientManager},
    db::traits::WithdrawalIndexerDb,
    esplora::{self, EsploraClient},
    state::{WithdrawalInfoUpdate, WithdrawalStatusCandidate},
    types::{WithdrawalInfo, WithdrawalStatus},
    withdrawal_requests,
};

/// Fetch withdrawal cache updates for paired deposits.
///
/// This function owns the source joins needed to render withdrawal rows:
/// `deposit_idx -> withdrawal_seq -> DbWithdrawalRequest -> bridge status`.
/// It takes source clients and DB handles, but never reads or mutates
/// [`BridgeMonitoringState`](super::state::BridgeMonitoringState).
pub(crate) async fn get_withdrawal_updates(
    rpc_manager: &RpcClientManager,
    withdrawal_index: &impl WithdrawalIndexerDb,
    esplora_client: &EsploraClient,
    chain_tip_height: L1Height,
    candidates: &[WithdrawalStatusCandidate],
    batch_size: usize,
) -> Vec<WithdrawalInfoUpdate> {
    if candidates.is_empty() {
        return Vec::new();
    }

    // Pairings now correlate deposit→seq by destination+amount, so a candidate's
    // `withdrawal_seq` is not necessarily contiguous from the lowest one. Fetch
    // the full seq span the candidates reference (gaps included) so every
    // candidate's request row is present in the lookup map.
    let min_seq = candidates
        .iter()
        .map(|candidate| candidate.withdrawal_seq)
        .min()
        .expect("candidates is non-empty");
    let max_seq = candidates
        .iter()
        .map(|candidate| candidate.withdrawal_seq)
        .max()
        .expect("candidates is non-empty");
    let span = usize::try_from(max_seq - min_seq)
        .ok()
        .and_then(|span| span.checked_add(1))
        .unwrap_or(candidates.len());
    let withdrawal_requests =
        withdrawal_requests::fetch_withdrawal_requests(withdrawal_index, min_seq, span, batch_size);
    let withdrawal_requests = withdrawal_requests
        .into_iter()
        .map(|row| (row.seq, row.request))
        .collect::<BTreeMap<_, _>>();

    let mut updates = Vec::new();
    for candidate in candidates {
        let Some(withdrawal_request) = withdrawal_requests.get(&candidate.withdrawal_seq) else {
            warn!(
                deposit_idx = candidate.deposit_idx,
                withdrawal_seq = candidate.withdrawal_seq,
                "missing indexed withdrawal request for paired deposit"
            );
            continue;
        };

        let status =
            match bridge_rpc::get_withdrawal_status(rpc_manager, candidate.deposit_idx).await {
                Ok(Some(status)) => status,
                Ok(None) => continue,
                Err(e) => {
                    warn!(
                        deposit_idx = candidate.deposit_idx,
                        error = %e,
                        "failed to fetch withdrawal status"
                    );
                    continue;
                }
            };

        let info = WithdrawalInfo::from_status(withdrawal_request.tx_hash, &status);
        let confirmations = match info.status {
            WithdrawalStatus::InProgress => None,
            WithdrawalStatus::Complete => {
                let Some(fulfillment_txid) = info.fulfillment_txid else {
                    continue;
                };
                esplora::get_tx_confirmations(esplora_client, fulfillment_txid, chain_tip_height)
                    .await
            }
        };

        updates.push(WithdrawalInfoUpdate {
            deposit_idx: candidate.deposit_idx,
            info,
            confirmations,
        });
    }

    updates
}
