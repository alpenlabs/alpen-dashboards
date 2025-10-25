use jsonrpsee::http_client::{HttpClient, HttpClientBuilder};
use std::time::Duration;
use std::{fmt, future::Future};
use tokio::time::sleep;
use tracing::warn;

use super::retry_policy::ExponentialBackoff;

/// Creates a JSON-RPC HTTP client with connection pooling and timeout configuration
///
/// This creates a reusable HTTP client that maintains a connection pool to avoid
/// connection exhaustion. The client is configured with:
///
/// - 30-second request timeout to prevent hanging requests
/// - 10MB max request size limit
/// - Internal connection pooling (managed by hyper)
///
/// # Arguments
///
/// * `rpc_url` - Base URL of the JSON-RPC server
///
/// # Returns
///
/// A configured [`HttpClient`] ready to make JSON-RPC requests
///
/// # Panics
///
/// Panics if the client cannot be built (e.g., invalid URL format)
///
/// # Example
///
/// ```ignore
/// let client = create_rpc_client("http://localhost:8332");
/// let result = client.request("getblockcount", ()).await?;
/// ```
pub fn create_rpc_client(rpc_url: &str) -> HttpClient {
    HttpClientBuilder::default()
        .request_timeout(Duration::from_secs(30))
        .max_request_size(10 * 1024 * 1024) // 10MB
        .build(rpc_url)
        .expect("Failed to create JSON-RPC client")
}

/// Execute an async operation with exponential backoff retry logic
pub async fn execute_with_retries<F, Fut, T, E>(operation: F, operation_name: &str) -> Result<T, E>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: fmt::Display,
{
    let retry_policy = ExponentialBackoff::new(3, 10, 1.5);
    let mut last_error = None;

    for attempt in 0..=retry_policy.max_retries() {
        match operation().await {
            Ok(result) => return Ok(result),
            Err(e) => {
                if attempt < retry_policy.max_retries() {
                    let delay = retry_policy.get_delay(attempt + 1);
                    warn!(
                        operation = operation_name,
                        attempt = attempt + 1,
                        max_retries = retry_policy.max_retries(),
                        delay_secs = delay,
                        error = %e,
                        "Operation failed, retrying..."
                    );
                    sleep(Duration::from_secs(delay)).await;
                }
                last_error = Some(e);
            }
        }
    }

    Err(last_error.expect("last_error should be set after all retries"))
}
