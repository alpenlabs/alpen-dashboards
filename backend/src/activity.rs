use anyhow::{Context, Result};
use axum::Json;
use chrono::{DateTime, Datelike, Duration, TimeZone, Utc};
use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use tokio::{sync::RwLock, time::interval};
use tracing::{error, info};

use crate::config::ActivityMonitoringConfig;

/// Enum for activity statistics
#[derive(Debug, Eq, PartialEq, Hash, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum ActivityStatName {
    #[serde(rename = "ACTIVITY_STATS__USER_OPS")]
    UserOps,
    #[serde(rename = "ACTIVITY_STATS__GAS_USED")]
    GasUsed,
    #[serde(rename = "ACTIVITY_STATS__UNIQUE_ACTIVE_ACCOUNTS")]
    UniqueActiveAccounts,
}

/// Enum for time windows
#[derive(Debug, Eq, PartialEq, Hash, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum TimeWindow {
    #[serde(rename = "TIME_WINDOW__LAST_24_HOURS")]
    Last24Hours,
    #[serde(rename = "TIME_WINDOW__LAST_30_DAYS")]
    Last30Days,
    #[serde(rename = "TIME_WINDOW__YEAR_TO_DATE")]
    YearToDate,
}

impl TimeWindow {
    fn to_duration(&self, now: DateTime<Utc>) -> Duration {
        match self {
            TimeWindow::Last24Hours => Duration::days(1),
            TimeWindow::Last30Days => Duration::days(30),
            TimeWindow::YearToDate => {
                Duration::days(now.ordinal() as i64) // Days since Jan 1st
            }
        }
    }
}

/// Enum for account selection criteria
#[derive(Debug, Eq, PartialEq, Hash, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum SelectAccountsBy {
    #[serde(rename = "ACCOUNTS__RECENT")]
    Recent,
    #[serde(rename = "ACCOUNTS__TOP_GAS_CONSUMERS_24H")]
    TopGasConsumers24h,
}

/// Struct for holding parsed JSON
#[derive(Debug, PartialEq, Deserialize)]
pub(crate) struct ActivityStatsKeys {
    activity_stat_names: HashMap<ActivityStatName, String>,
    time_windows: HashMap<TimeWindow, String>,
    select_accounts_by: HashMap<SelectAccountsBy, String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Account {
    #[serde(deserialize_with = "get_address_hash")]
    address: String,

    #[serde(deserialize_with = "from_null_or_string")]
    creation_timestamp: String, // ISO 8601 formatted timestamp

    #[serde(default)]
    gas_used: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct UserOp {
    #[serde(rename = "address", deserialize_with = "get_address_hash")]
    sender: String,

    #[serde(rename = "fee")]
    #[serde(deserialize_with = "convert_to_u64")]
    gas_used: u64,

    timestamp: String,
}

struct UserOpsResponse {
    user_ops: Vec<UserOp>,
    next_page_token: Option<String>,
}

struct AccountsResponse {
    accounts: Vec<Account>,
    next_page_token: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ActivityStats {
    /// Activity stats:
    /// First level key is the name of stat. See USAGE_STATS in `activity_keys.json`.
    /// Second level key is time period. See TIME_WINDOWS in `activity_keys.json`.
    stats: HashMap<String, HashMap<String, u64>>,

    /// Selected accounts: e.g. recently deployed, top gas consumers
    /// First level key is the name of stat. See SELECTED_ACCOUNTS in `activity_keys.json`.
    selected_accounts: HashMap<String, Vec<Account>>,
}

impl ActivityStats {
    pub fn default(config: &ActivityMonitoringConfig) -> ActivityStats {
        let stats: HashMap<String, HashMap<String, u64>> = config
            .activity_stats_keys()
            .activity_stat_names
            .values()
            .map(|stat_name| {
                let inner: HashMap<String, u64> = config
                    .activity_stats_keys()
                    .time_windows
                    .values()
                    .map(|window| (window.clone(), 0u64))
                    .collect();
                (stat_name.clone(), inner)
            })
            .collect();

        let selected_accounts: HashMap<_, Vec<Account>> = config
            .activity_stats_keys()
            .select_accounts_by
            .values()
            .map(|key| (key.to_owned(), Vec::new()))
            .collect();

        ActivityStats {
            stats,
            selected_accounts,
        }
    }
}

/// Shared activity stats
pub type SharedActivityStats = Arc<RwLock<ActivityStats>>;

type UniqueAccounts = HashMap<String, HashSet<String>>;
type AccountsGasUsage = HashMap<String, u64>;

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
        // Aggregate gas used per sender (in the last 24 hours)
        let mut gas_usage: AccountsGasUsage = HashMap::new();

        let time_windows: Vec<(String, Duration)> = config
            .activity_stats_keys()
            .time_windows
            .iter()
            .map(|(tw, tw_value)| (tw_value.clone(), tw.to_duration(now)))
            .collect();

        // Initialize or reset stats
        for (period, _) in &time_windows {
            for stat_name in config.activity_stats_keys().activity_stat_names.values() {
                locked_stats
                    .stats
                    .entry(stat_name.clone())
                    .or_default()
                    .insert(period.to_string(), 0);
            }
        }

        // Create sets to track unique active accounts per period
        let mut unique_accounts: UniqueAccounts = HashMap::new();
        for (period, _) in &time_windows {
            unique_accounts.insert(period.clone(), HashSet::new());
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
                    // compute stats for each TIME_WINDOW
                    for entry in response.user_ops {
                        if let Ok(op_time) = DateTime::parse_from_rfc3339(&entry.timestamp)
                            .map(|dt| dt.with_timezone(&Utc))
                        {
                            for (period, duration) in &time_windows {
                                if now - *duration <= op_time {
                                    for (stat_key, stat_name) in
                                        &config.activity_stats_keys().activity_stat_names
                                    {
                                        if matches!(
                                            stat_key,
                                            ActivityStatName::UserOps | ActivityStatName::GasUsed
                                        ) {
                                            *locked_stats
                                                .stats
                                                .entry(stat_name.clone()) // Get or insert HashMap entry
                                                .or_default() // Insert default if missing
                                                .entry(period.to_string()) // Get nested period entry
                                                .or_insert(0) += 1; // Increment counter
                                        }
                                    }
                                    // Track unique senders
                                    unique_accounts
                                        .get_mut(period)
                                        .unwrap()
                                        .insert(entry.sender.clone());
                                }
                            }
                            // Update gas used by sender
                            if now - Duration::days(1) <= op_time {
                                *gas_usage.entry(entry.sender.clone()).or_insert(0) +=
                                    entry.gas_used;
                            }
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

        // Store the count of unique active accounts
        for (period, accounts_set) in unique_accounts {
            locked_stats
                .stats
                .entry(
                    config.activity_stats_keys().activity_stat_names
                        [&ActivityStatName::UniqueActiveAccounts]
                        .clone(),
                ) // Use enum variant
                .or_default()
                .insert(period.to_string(), accounts_set.len() as u64);
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
                    // Sort accounts by creation_timestamp (most recent first)
                    let mut sorted_accounts: Vec<Account> = response
                        .accounts
                        .iter()
                        .filter(|acc| !acc.creation_timestamp.is_empty())
                        .cloned()
                        .collect();

                    sorted_accounts.sort_by(|a, b| {
                        let a_time = DateTime::parse_from_rfc3339(a.creation_timestamp.as_str())
                            .map(|dt| dt.with_timezone(&Utc))
                            .unwrap_or(Utc::now()); // Default to now if parsing fails
                        let b_time = DateTime::parse_from_rfc3339(b.creation_timestamp.as_str())
                            .map(|dt| dt.with_timezone(&Utc))
                            .unwrap_or(Utc::now());

                        b_time.cmp(&a_time) // Sort descending
                    });

                    // Take the top 5 most recent accounts
                    let recent_accounts = sorted_accounts.into_iter().take(5).collect::<Vec<_>>();
                    // Store in shared stats
                    locked_stats.selected_accounts.insert(
                        config.activity_stats_keys().select_accounts_by[&SelectAccountsBy::Recent]
                            .clone(),
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

        // Top gas consumers: get from gas_usage and sort by gas used (descending)
        let gas_usage_clone = gas_usage.clone();
        let mut top_gas_consumers: Vec<Account> = gas_usage_clone
            .into_iter()
            .map(|(address, gas_used)| Account {
                address,
                creation_timestamp: "".to_string(),
                gas_used,
            })
            .collect();

        top_gas_consumers.sort_by_key(|acc| gas_usage.get(&acc.address).cloned().unwrap_or(0));
        top_gas_consumers.reverse();
        top_gas_consumers.truncate(5); // Take top 5

        // Store in shared stats
        locked_stats.selected_accounts.insert(
            config.activity_stats_keys().select_accounts_by[&SelectAccountsBy::TopGasConsumers24h]
                .clone(),
            top_gas_consumers,
        );
    }
}

// Custom deserializer to extract "hash" from the "address" field
fn get_address_hash<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value: Value = Deserialize::deserialize(deserializer)?;

    // Extract "hash" field from "address"
    if let Some(hash) = value.get("hash").and_then(|h| h.as_str()) {
        return Ok(hash.to_string());
    }

    Err(de::Error::missing_field("address.hash"))
}

// Custom deserializer to convert a string to u64
fn convert_to_u64<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let s: String = Deserialize::deserialize(deserializer)?;
    s.parse::<u64>().map_err(de::Error::custom) // Convert string to u64 safely
}

// Custom deserializer to handle `null` timestamps
fn from_null_or_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let opt: Option<Value> = Option::deserialize(deserializer)?;
    match opt {
        Some(Value::String(s)) => Ok(s), // If valid string, return it
        Some(Value::Null) | None => Ok("".to_string()), // If null or missing, return None
        _ => Err(de::Error::custom("Expected a string or null")),
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
    use crate::activity::{
        convert_to_u64, fetch_accounts, fetch_user_ops, get_address_hash, ActivityMonitoringConfig,
        ActivityStats, TimeWindow,
    };
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

        // Year to date should return the number of days since Jan 1st
        let expected_days = now.ordinal() as i64;
        assert_eq!(
            TimeWindow::YearToDate.to_duration(now),
            chrono::Duration::days(expected_days)
        );
    }

    #[test]
    fn test_activity_stats_default() {
        let config = ActivityMonitoringConfig::new();
        let stats = ActivityStats::default(&config);

        for stat_name in config.activity_stats_keys().activity_stat_names.values() {
            let inner = stats.stats.get(stat_name).expect("Missing stat name key");
            for time_window in config.activity_stats_keys().time_windows.values() {
                assert_eq!(
                    inner.get(time_window),
                    Some(&0),
                    "Expected stat value 0 for {stat_name} in {time_window}"
                );
            }
        }

        for select_by in config.activity_stats_keys().select_accounts_by.values() {
            let accounts = stats
                .selected_accounts
                .get(select_by)
                .expect("Missing selected_accounts key");
            assert!(
                accounts.is_empty(),
                "Expected empty accounts list for {select_by}"
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

        let json_data = json!({ "address": {} }); // Missing "hash"
        let result: Result<TestAddress, _> = serde_json::from_value(json_data);
        assert!(result.is_err());

        let json_data = json!({}); // Missing "address" field
        let result: Result<TestAddress, _> = serde_json::from_value(json_data);
        assert!(result.is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_fetch_user_ops() {
        // Use the async version of mockito server
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

        // Use a persistent reqwest client
        let client = reqwest::Client::new();
        let start_time = Utc::now() - chrono::Duration::days(1);
        let end_time = Utc::now();

        // Await the async call properly
        let result = fetch_user_ops(&client, &url, start_time, end_time, Some(5), None)
            .await
            .unwrap();
        // Ensures the request actually hit the mock server
        mock_endpoint.assert();
        let ops = result.user_ops;
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].sender, "0x123456789abcdef");
        assert_eq!(ops[0].gas_used, 100);
        let page_token = result.next_page_token;
        assert!(page_token.is_none())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_fetch_accounts() {
        let mut server = Server::new_async().await;

        // Ensure the mock server recognizes `/accounts`
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
        // Ensures the request actually hit the mock server
        mock_endpoint.assert();
        let accounts = result.accounts;
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].address, "0xabcdef123456");
        let page_token = result.next_page_token;
        assert!(page_token.is_none())
    }
}
