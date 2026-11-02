//! Shared helpers for bridge tests.

use axum::Router;
use status_config::BridgeMonitoringConfig;
use strata_retry::RetryConfig;
use tokio::{net::TcpListener, task::JoinHandle};

pub(crate) fn make_retry_config() -> RetryConfig {
    let config: BridgeMonitoringConfig = toml::from_str(
        r#"
        esplora_url = "http://localhost:3000"
        max_tx_confirmations = 6
        status_refetch_interval_s = 1
        operators = []

        [retry]
        base_delay_ms = 1
        max_delay_ms = 1
        "#,
    )
    .expect("parse test retry config");
    config.retry_policy()
}

/// Local HTTP server with a task that is aborted on drop.
pub(crate) struct TestHttpServer {
    url: String,
    task: JoinHandle<()>,
}

impl TestHttpServer {
    pub(crate) async fn new(router: Router) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test HTTP listener");
        let url = format!(
            "http://{}",
            listener.local_addr().expect("test listener address")
        );
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("serve test HTTP requests");
        });
        Self { url, task }
    }

    pub(crate) fn url(&self) -> &str {
        &self.url
    }
}

impl Drop for TestHttpServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
