use jsonrpsee::http_client::{HttpClient, HttpClientBuilder};
use std::time::Duration;

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

/// Creates a JSON-RPC HTTP client with a custom timeout duration
///
/// Similar to [`create_rpc_client`] but allows specifying a custom request timeout.
/// This is useful for operations that may take longer than the default 30 seconds.
///
/// # Arguments
///
/// * `rpc_url` - Base URL of the JSON-RPC server
/// * `timeout_secs` - Request timeout in seconds
///
/// # Returns
///
/// A configured [`HttpClient`] with the specified timeout
///
/// # Panics
///
/// Panics if the client cannot be built (e.g., invalid URL format)
///
/// # Example
///
/// ```ignore
/// // Create a client with 60-second timeout for slow operations
/// let client = create_rpc_client_with_timeout("http://localhost:8332", 60);
/// ```
#[allow(dead_code)]
pub fn create_rpc_client_with_timeout(rpc_url: &str, timeout_secs: u64) -> HttpClient {
    HttpClientBuilder::default()
        .request_timeout(Duration::from_secs(timeout_secs))
        .max_request_size(10 * 1024 * 1024) // 10MB
        .build(rpc_url)
        .expect("Failed to create JSON-RPC client")
}
