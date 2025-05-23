use anyhow::{Context, Result};
use axum::Json;
use chrono::{DateTime, Datelike, Duration, TimeZone, Utc};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use tokio::{sync::RwLock, time::interval};
use tracing::{error, info};

use crate::{config::ActivityMonitoringConfig, types::activity::*};

/// Shared activity stats
pub type SharedActivityStats = Arc<RwLock<ActivityStats>>;

/// Periodically fetch user operations and accounts and compute activity stats
pub async fn activity_monitoring_task(
    shared_stats: SharedActivityStats,
    config: &ActivityMonitoringConfig,
) {
    let mut interval = interval(tokio::time::Duration::from_secs(
        config.stats_refetch_interval(),
    ));

    loop {
        interval.tick().await;
        let http_client = reqwest::Client::new();

        info!("Refresing activity stats...");
        let now = Utc::now();

        // Determine the start_time for stats
        let time_30d_earlier = now - Duration::days(30);
        let mut start_time = Utc.with_ymd_and_hms(now.year(), 1, 1, 0, 0, 0).unwrap();
        if time_30d_earlier < start_time {
            start_time = time_30d_earlier;
        }

        let mut locked_stats = shared_stats.write().await;
        let mut gas_usage: AccountsGasUsage = HashMap::new();

        let time_windows: Vec<(TimeWindow, Duration)> = config
            .activity_stats_keys()
            .time_windows
            .keys()
            .map(|&tw| (tw, tw.to_duration(now)))
            .collect();

        for (period, _) in &time_windows {
            for &stat_name in config.activity_stats_keys().activity_stat_names.keys() {
                locked_stats
                    .stats
                    .entry(stat_name)
                    .or_default()
                    .insert(*period, 0);
            }
        }

        let mut unique_accounts: UniqueAccounts = HashMap::new();
        for (period, _) in &time_windows {
            unique_accounts.insert(*period, HashSet::new());
        }

        let mut more_items = true;
        let mut page_token = None;
        while more_items {
            let result = fetch_user_ops(
                &http_client,
                config.user_ops_query_url(),
                start_time,
                now,
                Some(config.query_page_size()),
                page_token,
            )
            .await;

            match result {
                Ok(response) => {
                    for entry in response.user_ops {
                        let op_time = entry.timestamp;
                        for (period, duration) in &time_windows {
                            if now - *duration <= op_time {
                                if let Some(stat_map) = locked_stats.stats.get_mut(&ActivityStatName::UserOps) {
                                    *stat_map.entry(*period).or_insert(0) += 1;
                                }
                                if let Some(stat_map) = locked_stats.stats.get_mut(&ActivityStatName::GasUsed) {
                                    *stat_map.entry(*period).or_insert(0) += entry.gas_used;
                                }
                                unique_accounts.get_mut(period).unwrap().insert(entry.sender.clone());
                            }
                        }

                        if now - Duration::days(1) <= op_time {
                            *gas_usage.entry(entry.sender.clone()).or_insert(0) += entry.gas_used;
                        }
                    }

                    page_token = response.next_page_token;
                    more_items = page_token.is_some();
                }
                Err(e) => {
                    error!(error = %e, "Fetch user ops failed");
                    break;
                }
            }
        }

        for (period, accounts_set) in unique_accounts {
            locked_stats
                .stats
                .entry(ActivityStatName::UniqueActiveAccounts)
                .or_default()
                .insert(period, accounts_set.len() as u64);
        }

        let mut more_items = true;
        let mut page_token = None;
        while more_items {
            let result = fetch_accounts(
                &http_client,
                config.accounts_query_url(),
                start_time,
                now,
                Some(config.query_page_size()),
                page_token,
            )
            .await;
            match result {
                Ok(response) => {
                    let mut sorted_accounts: Vec<Account> = response
                        .accounts
                        .iter()
                        .filter(|acc| acc.creation_timestamp.is_some())
                        .cloned()
                        .collect();

                    sorted_accounts.sort_by(|a, b| b.creation_timestamp.cmp(&a.creation_timestamp));

                    let recent_accounts = sorted_accounts.into_iter().take(5).collect::<Vec<_>>();
                    locked_stats.selected_accounts.insert(
                        SelectAccountsBy::Recent,
                        recent_accounts,
                    );

                    page_token = response.next_page_token;
                    more_items = page_token.is_some();
                }
                Err(e) => {
                    error!(error = %e, "Fetch accounts failed");
                    break;
                }
            }
        }

        let mut top_gas_consumers: Vec<Account> = gas_usage
            .iter()
            .map(|(address, &gas_used)| Account {
                address: address.clone(),
                creation_timestamp: None,
                gas_used,
            })
            .collect();

        top_gas_consumers.sort_by_key(|acc| std::cmp::Reverse(acc.gas_used));
        top_gas_consumers.truncate(5);

        locked_stats.selected_accounts.insert(
            SelectAccountsBy::TopGasConsumers24h,
            top_gas_consumers,
        );
    }
}

async fn fetch_activity_common(
    http_client: &reqwest::Client,
    query_url: &str,
    start_time: DateTime<Utc>,
    end_time: DateTime<Utc>,
    page_size: Option<u64>,
    page_token: Option<String>,
) -> Result<serde_json::Value, anyhow::Error> {
    // Format to YYYY-MM-DD HH:MM:SS
    let format_time =
        |time: DateTime<Utc>| -> String { time.format("%Y-%m-%d %H:%M:%S").to_string() };

    // Construct query parameters, only adding Some(_) values
    let mut query_params: HashMap<&str, String> = HashMap::new();
    query_params.insert("start_time", format_time(start_time));
    query_params.insert("end_time", format_time(end_time));
    if let Some(size) = page_size {
        query_params.insert("page_size", size.to_string());
    }

    if let Some(token) = page_token {
        info!("page token {}", token);
        query_params.insert("page_token", token);
    }

    // Send request with query parameters (browser-like format)
    let response = http_client
        .get(query_url)
        .query(&query_params) // Use query parameters instead of JSON body
        .send()
        .await?
        .error_for_status()? // Converts HTTP errors into Rust errors
        .json::<serde_json::Value>()
        .await?;

    Ok(response)
}

async fn fetch_user_ops(
    http_client: &reqwest::Client,
    query_url: &str,
    start_time: DateTime<Utc>,
    end_time: DateTime<Utc>,
    page_size: Option<u64>,
    page_token: Option<String>,
) -> Result<UserOpsResponse, anyhow::Error> {
    info!("Fetching user operations");

    let data = fetch_activity_common(
        http_client,
        query_url,
        start_time,
        end_time,
        page_size,
        page_token,
    )
    .await
    .context("Failed to fetch user operations")?;

    // Extract "items" and deserialize into Vec<UserOp>
    let items = data.get("items").context("Missing 'items' in response")?;
    let user_ops: Vec<UserOp> =
        serde_json::from_value(items.clone()).context("Failed to deserialize user ops")?;

    // Extract next_page_token safely
    let next_page_token = data
        .get("next_page_params")
        .and_then(|params| params.get("page_token"))
        .and_then(|token| token.as_str()) // ✅ Get string reference directly
        .map(|s| s.trim_matches('"').to_string()); // ✅ Remove extra quotes if present

    Ok(UserOpsResponse {
        user_ops,
        next_page_token,
    })
}

async fn fetch_accounts(
    http_client: &reqwest::Client,
    query_url: &str,
    start_time: DateTime<Utc>,
    end_time: DateTime<Utc>,
    page_size: Option<u64>,
    page_token: Option<String>,
) -> Result<AccountsResponse, anyhow::Error> {
    info!("Fetching accounts");

    let data = fetch_activity_common(
        http_client,
        query_url,
        start_time,
        end_time,
        page_size,
        page_token,
    )
    .await
    .context("Failed to fetch accounts")?;

    // Extract "items" field safely
    let items = data
        .get("items")
        .context("Missing 'items' field in response")?;
    let accounts: Vec<Account> =
        serde_json::from_value(items.clone()).context("Failed to deserialize accounts")?;

    // Extract next_page_token safely
    let next_page_token = data
        .get("next_page_params")
        .and_then(|params| params.get("page_token"))
        .and_then(|token| token.as_str()) // ✅ Get string reference directly
        .map(|s| s.trim_matches('"').to_string()); // ✅ Remove extra quotes if present

    Ok(AccountsResponse {
        accounts,
        next_page_token,
    })
}

pub async fn get_activity_stats(state: SharedActivityStats) -> Json<ActivityStats> {
    let data = state.read().await.clone();
    Json(data)
}

#[cfg(test)]
mod tests {
    use crate::types::activity::{
        convert_to_u64, get_address_hash, ActivityStats, TimeWindow
    };
    use crate::{activity::{fetch_accounts, fetch_user_ops}, config::ActivityMonitoringConfig};
    use chrono::{Datelike, TimeZone, Utc};
    use mockito::{Matcher, Server};
    use serde::Deserialize;
    use serde_json::json;

    #[test]
    fn test_time_window_to_duration() {
        let now = Utc.with_ymd_and_hms(2025, 2, 17, 0, 0, 0).unwrap();

        assert_eq!(
            TimeWindow::Last24Hours.to_duration(now),
            chrono::Duration::days(1)
        );
        assert_eq!(
            TimeWindow::Last30Days.to_duration(now),
            chrono::Duration::days(30)
        );

        let expected_days = now.ordinal() as i64;
        assert_eq!(
            TimeWindow::YearToDate.to_duration(now),
            chrono::Duration::days(expected_days)
        );
    }

    #[test]
    fn test_activity_stats_default() {
        let config = ActivityMonitoringConfig::new();
        let stats = ActivityStats::with_config(&config);

        for &stat_name in config.activity_stats_keys().activity_stat_names.keys() {
            let inner = stats.stats.get(&stat_name).expect("Missing stat name key");
            for &time_window in config.activity_stats_keys().time_windows.keys() {
                assert_eq!(
                    inner.get(&time_window),
                    Some(&0),
                    "Expected stat value 0 for {:?} in {:?}",
                    stat_name,
                    time_window
                );
            }
        }

        for &select_by in config.activity_stats_keys().select_accounts_by.keys() {
            let accounts = stats
                .selected_accounts
                .get(&select_by)
                .expect("Missing selected_accounts key");
            assert!(
                accounts.is_empty(),
                "Expected empty accounts list for {:?}",
                select_by
            );
        }
    }

    #[test]
    fn test_convert_to_u64() {
        #[derive(Deserialize)]
        struct TestFee {
            #[serde(deserialize_with = "convert_to_u64")]
            fee: u64,
        }

        let json_data = json!({ "fee": "12345" });
        let obj: TestFee = serde_json::from_value(json_data).unwrap();
        assert_eq!(obj.fee, 12345);

        let json_data = json!({ "fee": "invalid" });
        let result: Result<TestFee, _> = serde_json::from_value(json_data);
        assert!(result.is_err());
    }

    #[derive(Deserialize)]
    struct TestAddress {
        #[serde(deserialize_with = "get_address_hash")]
        address: String,
    }

    #[test]
    fn test_get_address_hash() {
        let json_data = json!({ "address": { "hash": "0x123456" } });
        let obj: TestAddress = serde_json::from_value(json_data).unwrap();
        assert_eq!(obj.address, "0x123456");

        let json_data = json!({ "address": {} });
        let result: Result<TestAddress, _> = serde_json::from_value(json_data);
        assert!(result.is_err());

        let json_data = json!({});
        let result: Result<TestAddress, _> = serde_json::from_value(json_data);
        assert!(result.is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_fetch_user_ops() {
        let mut server = Server::new_async().await;

        let mock_endpoint = server
            .mock("GET", Matcher::Regex(r"^/user_ops(\?.*)?$".to_string()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "items": [
                        {
                            "address": { "hash": "0x123456789abcdef" },
                            "fee": "100",
                            "timestamp": "2024-03-10T12:00:00Z"
                        }
                    ]
                })
                .to_string(),
            )
            .create();

        let url = format!("{}/user_ops", server.url());

        let client = reqwest::Client::new();
        let start_time = Utc::now() - chrono::Duration::days(1);
        let end_time = Utc::now();

        let result = fetch_user_ops(&client, &url, start_time, end_time, Some(5), None)
            .await
            .unwrap();
        mock_endpoint.assert();
        let ops = result.user_ops;
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].sender, "0x123456789abcdef");
        assert_eq!(ops[0].gas_used, 100);
        assert!(result.next_page_token.is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_fetch_accounts() {
        let mut server = Server::new_async().await;

        let mock_endpoint = server
            .mock("GET", Matcher::Regex(r"^/accounts(\?.*)?$".to_string()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "items": [
                        {
                            "address": { "hash": "0xabcdef123456" },
                            "creation_timestamp": "2024-03-10T12:00:00Z",
                        }
                    ]
                })
                .to_string(),
            )
            .create();

        let url = format!("{}/accounts", server.url());

        let client = reqwest::Client::new();
        let start_time = Utc::now() - chrono::Duration::days(1);
        let end_time = Utc::now();

        let result = fetch_accounts(&client, &url, start_time, end_time, Some(5), None)
            .await
            .unwrap();
        mock_endpoint.assert();
        let accounts = result.accounts;
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].address, "0xabcdef123456");
        assert!(result.next_page_token.is_none());
    }
}
