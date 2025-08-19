use std::env;

/// Extension trait for parsing environment variables
pub trait EnvVarParser {
    fn parse_env_var(key: &str) -> Option<Self>
    where
        Self: std::str::FromStr;
}

/// Parse a generic numeric environment variable, returning None if not set or invalid
pub fn parse_env_var_number<T>(key: &str) -> Option<T>
where
    T: std::str::FromStr,
{
    env::var(key).ok().and_then(|s| s.parse::<T>().ok())
}

// Implement for String
impl EnvVarParser for String {
    fn parse_env_var(key: &str) -> Option<Self> {
        env::var(key).ok()
    }
}

// Implement for common numeric types
impl EnvVarParser for u64 {
    fn parse_env_var(key: &str) -> Option<Self> {
        parse_env_var_number(key)
    }
}
