use axum::Json;
use bitcoin::{OutPoint, Txid};
use jsonrpsee::core::client::ClientT;
use jsonrpsee::core::ClientError;
use jsonrpsee::http_client::HttpClient;
use std::{sync::Arc, str::FromStr};
use serde_json::Value;
use strata_bridge_primitives::types::PublickeyTable;
use tokio::{
    sync::RwLock,
    time::{interval, Duration},
};
use tracing::{error, info, warn};

use strata_bridge_rpc::types::{
    RpcClaimInfo, RpcDepositInfo, RpcOperatorStatus,
    RpcWithdrawalInfo,
};

use crate::{config::BridgeMonitoringConfig, utils::rpc_client::create_rpc_client,
    bridge::types::{
        BridgeStatus, DepositInfo, DepositToWithdrawal, OperatorStatus, ReimbursementInfo,
        WithdrawalInfo,
    }};

/// Shared bridge state
pub type SharedBridgeState = Arc<RwLock<BridgeStatus>>;

/// Periodically fetch bridge status and update shared bridge state
pub async fn bridge_monitoring_task(state: SharedBridgeState, config: &BridgeMonitoringConfig) {
    let mut interval = interval(Duration::from_secs(config.status_refetch_interval()));
    let strata_rpc = create_rpc_client(config.strata_rpc_url());
    let bridge_rpc = create_rpc_client(config.bridge_rpc_url());

    loop {
        interval.tick().await;
        let mut locked_state = state.write().await;

        // Bridge operator status
        let operators = get_bridge_operators(&bridge_rpc).await.unwrap();
        let mut operator_statuses = Vec::new();
        for (index, public_key) in operators.0.iter() {
            let operator_id = format!("Alpen Labs #{}", index);
            let status = get_operator_status(&bridge_rpc, *index).await.unwrap();

            operator_statuses.push(OperatorStatus::new(
                operator_id,
                *public_key,
                status,
            ));
        }

        locked_state.operators = operator_statuses;

        // Current deposits
        let current_deposits = get_current_deposits(&strata_rpc).await.unwrap();
        // Deposits with withdrawal requests
        let mut deposits_to_withdrawals: Vec<DepositToWithdrawal> = Vec::new();

        for deposit_id in current_deposits {
            let (deposit_info, deposit_to_wd) =
                get_deposit_info(&strata_rpc, &bridge_rpc, deposit_id)
                    .await
                    .unwrap();
            if let Some(deposit) = deposit_info {
                locked_state.deposits.push(deposit);
                deposits_to_withdrawals.push(deposit_to_wd.unwrap());
            } else {
                warn!(%deposit_id, "Missing deposit entry for id");
            }
        }

        // Withdrawal fulfillment
        let mut withdrawal_infos: Vec<WithdrawalInfo> =
            match get_withdrawals(&bridge_rpc, deposits_to_withdrawals).await {
                Ok(data) => data,
                Err(e) => {
                    error!(error = %e, "Bridge get withdrawal failed");
                    Vec::new()
                }
            };
        locked_state.withdrawals.append(&mut withdrawal_infos);

        // Reimbursements
        let reimbursements: Vec<ReimbursementInfo> = match get_reimbursements(&bridge_rpc).await {
            Ok(data) => data,
            Err(e) => {
                error!(error = %e, "Bridge get reimbursement failed");
                Vec::new()
            }
        };
        locked_state.reimbursements = reimbursements;
    }
}

/// Fetch operator idx and public keys
async fn get_bridge_operators(rpc_client: &HttpClient) -> Result<PublickeyTable, ClientError> {
    let operator_table: PublickeyTable = match rpc_client
        .request("stratabridge_bridgeOperators", ((),))
        .await
    {
        Ok(data) => data,
        Err(e) => {
            error!(error = %e, "Bridge operators query failed");
            return Err(e);
        }
    };

    Ok(operator_table)
}

/// Fetch operator status
async fn get_operator_status(
    bridge_client: &HttpClient,
    operator_idx: u32,
) -> Result<RpcOperatorStatus, ClientError> {
    let status: RpcOperatorStatus = match bridge_client
        .request("stratabridge_operatorStatus", (operator_idx,))
        .await
    {
        Ok(data) => data,
        Err(e) => {
            error!(error = %e, "Bridge operator status query");
            return Err(e);
        }
    };

    Ok(status)
}

/// Fetch current deposits
async fn get_current_deposits(strata_client: &HttpClient) -> Result<Vec<u32>, ClientError> {
    let deposit_ids: Vec<u32> = match strata_client
        .request("strata_getCurrentDeposits", ((),))
        .await
    {
        Ok(data) => data,
        Err(e) => {
            error!(error = %e, "Current deposits query failed");
            return Err(e);
        }
    };

    Ok(deposit_ids)
}

/// Fetch deposit info
///
/// First get deposit entry, which may have withdrawal request txid.
/// Return DepositInfo and DepositToWithdrawal (needed to fetch withdrawals)
async fn get_deposit_info(
    strata_rpc: &HttpClient,
    bridge_rpc: &HttpClient,
    deposit_id: u32,
) -> Result<(Option<DepositInfo>, Option<DepositToWithdrawal>), ClientError> {
    let response: Value = match strata_rpc
        .request("strata_getCurrentDepositById", (deposit_id,))
        .await
    {
        Ok(resp) => {
            info!(?resp, "deposit entry");
            resp
        }
        Err(e) => {
            warn!(%deposit_id, %e, "Skipping deposit id due to RPC error");
            return Ok((None, None));
        }
    };

    // Extract output (deposit_outpoint)
    let deposit_outpoint: Option<OutPoint> = response
        .get("output")
        .and_then(|v| v.as_str())
        .and_then(|s| OutPoint::from_str(s).ok());
    // Extract withdrawal_request_txid
    let withdrawal_request_txid: Option<Txid> = response
        .get("withdrawal_request_txid")
        .and_then(|v| v.as_str())
        .and_then(|s| Txid::from_str(s).ok());

    // Let caller decide what to do
    if deposit_outpoint.is_none() {
        return Ok((None, None));
    }

    let deposit_to_withdrawal = DepositToWithdrawal::new (
        deposit_outpoint.unwrap(),
        withdrawal_request_txid,
    );

    let deposit_info: RpcDepositInfo = match bridge_rpc
        .request("stratabridge_depositInfo", (deposit_outpoint,))
        .await
    {
        Ok(data) => data,
        Err(e) => {
            error!(error = %e, "Get deposit info failed");
            return Err(e);
        }
    };

    Ok((
        Some(DepositInfo::from(deposit_info)),
        Some(deposit_to_withdrawal),
    ))
}

/// Fetch withdrawal infos
async fn get_withdrawals(
    bridge_rpc: &HttpClient,
    deposit_to_withdrawals: Vec<DepositToWithdrawal>,
) -> Result<Vec<WithdrawalInfo>, ClientError> {
    let mut withdrawal_infos = Vec::new();
    for deposit_to_wd in deposit_to_withdrawals.iter() {
        if deposit_to_wd.withdrawal_request_txid().is_none() {
            continue;
        }

        let wd_info: RpcWithdrawalInfo = match bridge_rpc
            .request(
                "stratabridge_withdrawalInfo",
                (deposit_to_wd.deposit_outpoint(),),
            )
            .await
        {
            Ok(data) => data,
            Err(e) => {
                error!(error = %e, "Get withdrawal info failed");
                return Err(e);
            }
        };

        withdrawal_infos.push(WithdrawalInfo::from_rpc(
            &wd_info,
            deposit_to_wd.withdrawal_request_txid().unwrap(),
        ));
    }

    Ok(withdrawal_infos)
}

/// Fetch claim/reimbursement infos
async fn get_reimbursements(
    bridge_rpc: &HttpClient,
) -> Result<Vec<ReimbursementInfo>, ClientError> {
    let claim_txids: Vec<String> = match bridge_rpc.request("stratabridge_claims", ((),)).await {
        Ok(data) => data,
        Err(e) => {
            error!(error = %e, "Get claims failed");
            return Err(e);
        }
    };

    let mut reimbursement_infos = Vec::new();
    for txid in claim_txids.iter() {
        let reimb_info: RpcClaimInfo = match bridge_rpc
            .request("stratabridge_claimInfo", (txid.clone(),))
            .await
        {
            Ok(data) => data,
            Err(e) => {
                error!(error = %e, "Get claim info failed");
                return Err(e);
            }
        };

        reimbursement_infos.push(ReimbursementInfo::from(&reimb_info));
    }

    Ok(reimbursement_infos)
}

/// Return latest bridge status
pub async fn get_bridge_status(state: SharedBridgeState) -> Json<BridgeStatus> {
    let data = state.read().await.clone();
    Json(data)
}
