use chrono::{DateTime, Datelike, Duration, Utc};
use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use crate::config::ActivityMonitoringConfig;

/// Enum for activity statistics
#[derive(Debug, Eq, PartialEq, Hash, Deserialize, Serialize, Clone, Copy)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ActivityStatName {
    /// Number of user oprations
    #[serde(rename = "ACTIVITY_STATS__USER_OPS")]
    UserOps,
    /// Total gas used
    #[serde(rename = "ACTIVITY_STATS__GAS_USED")]
    GasUsed,
    /// Number of unique active accounts
    #[serde(rename = "ACTIVITY_STATS__UNIQUE_ACTIVE_ACCOUNTS")]
    UniqueActiveAccounts,
}

/// Enum for time windows
#[derive(Debug, Eq, PartialEq, Hash, Deserialize, Serialize, Clone, Copy)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TimeWindow {
    /// Last 24 hours
    #[serde(rename = "TIME_WINDOW__LAST_24_HOURS")]
    Last24Hours,
    /// Last 30 days
    #[serde(rename = "TIME_WINDOW__LAST_30_DAYS")]
    Last30Days,
    /// Year to date
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
    /// Recent accounts
    #[serde(rename = "ACCOUNTS__RECENT")]
    Recent,
    /// Top gas consumers in the last 24 hours
    #[serde(rename = "ACCOUNTS__TOP_GAS_CONSUMERS_24H")]
    TopGasConsumers24h,
}

/// Struct for mappings between JSON keys and their names in the dashboard UI
#[derive(Debug, PartialEq, Deserialize)]
pub(crate) struct ActivityStatsKeys {
    /// Mapping of [`ActivityStatName`] to the name used in the dashboard UI
    pub activity_stat_names: HashMap<ActivityStatName, String>,
    /// Mapping of [`TimeWindow`] to the name used in the dashboard UI
    pub time_windows: HashMap<TimeWindow, String>,
    /// Mapping of [`SelectAccountsBy`] to the name used in the dashboard UI
    pub select_accounts_by: HashMap<SelectAccountsBy, String>,
}

/// Ethereum-style address hash, extracted from a nested `{ "hash": ... }` structure.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct AddressHash(pub String);

impl<'de> Deserialize<'de> for AddressHash {
    // Custom deserializer to extract "hash" from the "address" field
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value: Value = Deserialize::deserialize(deserializer)?;
        if let Some(hash) = value.get("hash").and_then(|h| h.as_str()) {
            Ok(AddressHash(hash.to_owned()))
        } else {
            Err(de::Error::missing_field("address.hash"))
        }
    }
}

/// ERC4337 account
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Account {
    /// Account address hash
    pub address: AddressHash,

    /// Creation timestamp
    #[serde(deserialize_with = "from_null_or_datetime")]
    pub creation_timestamp: Option<DateTime<Utc>>,

    /// Total gas used by the account
    #[serde(default)]
    pub gas_used: u64,
}

/// ERC4337 user operation
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct UserOp {
    /// Sender address hash
    #[serde(rename = "address")]
    pub sender: AddressHash,

    /// Gas used
    #[serde(rename = "fee")]
    #[serde(deserialize_with = "convert_to_u64")]
    pub gas_used: u64,

    /// Timestamp
    #[serde(deserialize_with = "deserialize_iso8601")]
    pub timestamp: DateTime<Utc>,
}

/// Stats shown in the dashboard
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ActivityStats {
    /// Counts of activity by time window
    pub stats: HashMap<ActivityStatName, HashMap<TimeWindow, u64>>,
    /// Selected accounts based on criteria, e.g. "Recent accounts" or "Top gas consumers (24h)"
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

/// Response from Blockscout user operations indexer when querying "operations"
#[derive(Serialize, Deserialize, Debug)]
pub struct UserOpsResponse {
    pub user_ops: Vec<UserOp>,
    pub next_page_token: Option<String>,
}

/// Response from Blockscout user operations indexer when querying "accounts"
#[derive(Serialize, Deserialize, Debug)]
pub struct AccountsResponse {
    pub accounts: Vec<Account>,
    pub next_page_token: Option<String>,
}

/// Type alias for unique accounts per time window
pub type UniqueAccounts = HashMap<TimeWindow, HashSet<AddressHash>>;
/// Type alias for gas usage per account
pub type AccountsGasUsage = HashMap<AddressHash, u64>;

/// Deserializes a u64 value
pub fn convert_to_u64<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let s: String = Deserialize::deserialize(deserializer)?;
    s.parse::<u64>().map_err(de::Error::custom)
}

/// Deserializes a datetime that can be null or a string
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

/// Deserializes an ISO 8601 datetime
pub fn deserialize_iso8601<'de, D>(deserializer: D) -> Result<DateTime<Utc>, D::Error>
where
    D: Deserializer<'de>,
{
    let s: String = Deserialize::deserialize(deserializer)?;
    DateTime::parse_from_rfc3339(&s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(serde::de::Error::custom)
}
