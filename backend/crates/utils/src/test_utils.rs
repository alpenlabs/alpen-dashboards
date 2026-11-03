//! HTTP and JSON-RPC servers shared by backend tests.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use axum::{extract::State, routing::post, Json, Router};
use jsonrpsee::http_client::HttpClient;
use serde_json::{json, Value};
use tokio::{net::TcpListener, task::JoinHandle};

use crate::create_rpc_client;

/// Runs a local router until dropped, aborting its server task on cleanup.
pub struct TestHttpServer {
    url: String,
    task: JoinHandle<()>,
}

impl TestHttpServer {
    pub async fn new(router: Router) -> Self {
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

    pub fn url(&self) -> &str {
        &self.url
    }
}

impl Drop for TestHttpServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct RpcResponses {
    replies: VecDeque<Value>,
    methods: Vec<String>,
}

/// Serves queued JSON-RPC replies, repeating the final reply and recording methods.
pub struct TestRpcServer {
    server: TestHttpServer,
    responses: Arc<Mutex<RpcResponses>>,
}

impl TestRpcServer {
    pub async fn new(replies: Vec<Value>) -> Self {
        Self::with_routes(replies, Router::new()).await
    }

    /// Adds application-specific routes alongside the JSON-RPC endpoint at `/`.
    pub async fn with_routes(replies: Vec<Value>, router: Router) -> Self {
        assert!(!replies.is_empty(), "RPC fixture needs a response");
        let responses = Arc::new(Mutex::new(RpcResponses {
            replies: replies.into(),
            methods: Vec::new(),
        }));
        let router = router.merge(
            Router::new()
                .route("/", post(rpc_response))
                .with_state(responses.clone()),
        );
        Self {
            server: TestHttpServer::new(router).await,
            responses,
        }
    }

    pub fn url(&self) -> &str {
        self.server.url()
    }

    pub fn client(&self) -> HttpClient {
        create_rpc_client(self.url())
    }

    pub fn attempts(&self) -> usize {
        self.responses
            .lock()
            .expect("RPC response lock")
            .methods
            .len()
    }

    pub fn methods(&self) -> Vec<String> {
        self.responses
            .lock()
            .expect("RPC response lock")
            .methods
            .clone()
    }
}

async fn rpc_response(
    State(responses): State<Arc<Mutex<RpcResponses>>>,
    Json(request): Json<Value>,
) -> Json<Value> {
    let mut responses = responses.lock().expect("RPC response lock");
    responses
        .methods
        .push(request["method"].as_str().expect("RPC method").to_owned());
    let mut reply = responses.replies.front().expect("RPC reply").clone();
    if responses.replies.len() > 1 {
        responses.replies.pop_front();
    }
    reply["jsonrpc"] = json!("2.0");
    reply["id"] = request["id"].clone();
    Json(reply)
}

pub fn make_rpc_error_reply() -> Value {
    json!({"error": {"code": -32000, "message": "temporarily unavailable"}})
}
