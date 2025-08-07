use thiserror::Error;

/// Error types for wallet operations
#[derive(Debug, Error)]
pub(crate) enum WalletBalanceError {
    #[error("HTTP request failed: {0}")]
    HttpRequest(#[from] reqwest::Error),

    #[error("JSON parsing failed: {0}")]
    JsonParse(#[from] serde_json::Error),

    #[error("Invalid balance format: {0}")]
    BalanceFormat(String),

    #[error("Configuration error: {0}")]
    Config(String),
}
