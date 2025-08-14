use jsonrpsee::http_client::{HttpClient, HttpClientBuilder};

/// Creates a JSON-RPC client with a dynamic URL
pub fn create_rpc_client(rpc_url: &str) -> HttpClient {
    HttpClientBuilder::default()
        .build(rpc_url)
        .expect("Failed to create JSON-RPC client")
}
