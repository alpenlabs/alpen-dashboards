use chrono::{DateTime, Datelike, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde::de::{self, Deserializer};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use crate::config::ActivityMonitoringConfig;

/// Enum for activity statistics
#[derive(Debug, Eq, PartialEq, Hash, Deserialize, Serialize, Clone, Copy)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ActivityStatName {
    #[serde(rename = "ACTIVITY_STATS__USER_OPS")]
    UserOps,
    #[serde(rename = "ACTIVITY_STATS__GAS_USED")]
    GasUsed,
    #[serde(rename = "ACTIVITY_STATS__UNIQUE_ACTIVE_ACCOUNTS")]
    UniqueActiveAccounts,
}

/// Enum for time windows
#[derive(Debug, Eq, PartialEq, Hash, Deserialize, Serialize, Clone, Copy)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TimeWindow {
    #[serde(rename = "TIME_WINDOW__LAST_24_HOURS")]
    Last24Hours,
    #[serde(rename = "TIME_WINDOW__LAST_30_DAYS")]
    Last30Days,
    #[serde(rename = "TIME_WINDOW__YEAR_TO_DATE")]
    YearToDate,
}

impl TimeWindow {
    pub fn to_duration(&self, now: DateTime<Utc>) -> Duration {
        match self {
            TimeWindow::Last24Hours => Duration::days(1),
            TimeWindow::Last30Days => Duration::days(30),
            TimeWindow::YearToDate => Duration::days(now.ordinal() as i64),
        }
    }
}

/// Enum for account selection criteria
#[derive(Debug, Eq, PartialEq, Hash, Deserialize, Serialize, Clone, Copy)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SelectAccountsBy {
    #[serde(rename = "ACCOUNTS__RECENT")]
    Recent,
    #[serde(rename = "ACCOUNTS__TOP_GAS_CONSUMERS_24H")]
    TopGasConsumers24h,
}

/// Struct for holding parsed JSON
#[derive(Debug, PartialEq, Deserialize)]
pub(crate) struct ActivityStatsKeys {
    pub activity_stat_names: HashMap<ActivityStatName, String>,
    pub time_windows: HashMap<TimeWindow, String>,
    pub select_accounts_by: HashMap<SelectAccountsBy, String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Account {
    #[serde(deserialize_with = "get_address_hash")]
    pub address: String,

    #[serde(deserialize_with = "from_null_or_datetime")]
    pub creation_timestamp: Option<DateTime<Utc>>,

    #[serde(default)]
    pub gas_used: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct UserOp {
    #[serde(rename = "address", deserialize_with = "get_address_hash")]
    pub sender: String,

    #[serde(rename = "fee")]
    #[serde(deserialize_with = "convert_to_u64")]
    pub gas_used: u64,

    #[serde(deserialize_with = "deserialize_iso8601")]
    pub timestamp: DateTime<Utc>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ActivityStats {
    pub stats: HashMap<ActivityStatName, HashMap<TimeWindow, u64>>,
    pub selected_accounts: HashMap<SelectAccountsBy, Vec<Account>>,
}

impl ActivityStats {
    pub fn with_config(config: &ActivityMonitoringConfig) -> ActivityStats {
        let stats: HashMap<ActivityStatName, HashMap<TimeWindow, u64>> = config
            .activity_stats_keys()
            .activity_stat_names
            .keys()
            .map(|&stat_name| {
                let inner = config
                    .activity_stats_keys()
                    .time_windows
                    .keys()
                    .map(|&window| (window, 0u64))
                    .collect();
                (stat_name, inner)
            })
            .collect();

        let selected_accounts: HashMap<_, Vec<Account>> = config
            .activity_stats_keys()
            .select_accounts_by
            .keys()
            .map(|&key| (key, Vec::new()))
            .collect();

        ActivityStats {
            stats,
            selected_accounts,
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct UserOpsResponse {
    pub user_ops: Vec<UserOp>,
    pub next_page_token: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct AccountsResponse {
    pub accounts: Vec<Account>,
    pub next_page_token: Option<String>,
}

pub type UniqueAccounts = HashMap<TimeWindow, HashSet<String>>;
pub type AccountsGasUsage = HashMap<String, u64>;

// Custom deserializer to extract "hash" from the "address" field
pub fn get_address_hash<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value: Value = Deserialize::deserialize(deserializer)?;
    if let Some(hash) = value.get("hash").and_then(|h| h.as_str()) {
        Ok(hash.to_string())
    } else {
        Err(de::Error::missing_field("address.hash"))
    }
}

pub fn convert_to_u64<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let s: String = Deserialize::deserialize(deserializer)?;
    s.parse::<u64>().map_err(de::Error::custom)
}

pub fn from_null_or_datetime<'de, D>(deserializer: D) -> Result<Option<DateTime<Utc>>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt: Option<Value> = Option::deserialize(deserializer)?;
    match opt {
        Some(Value::String(s)) => Ok(Some(DateTime::from_str(&s).map_err(de::Error::custom)?)),
        Some(Value::Null) | None => Ok(None),
        _ => Err(de::Error::custom("Expected a string or null")),
    }
}

pub fn deserialize_iso8601<'de, D>(deserializer: D) -> Result<DateTime<Utc>, D::Error>
where
    D: Deserializer<'de>,
{
    let s: String = Deserialize::deserialize(deserializer)?;
    DateTime::parse_from_rfc3339(&s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(serde::de::Error::custom)
}
