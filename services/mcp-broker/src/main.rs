//! MCP stdio bridge.
//!
//! Agent harnesses (OpenCode) spawn this binary as a local MCP server. It forwards
//! `tools/list` and `tools/call` to the ORBIT tool broker over loopback, authenticated
//! with an execution-scoped token. The harness never receives provider credentials; all
//! policy, approval and connector logic lives in the broker.
//!
//! Requests are dispatched concurrently: a `tools/call` waiting for a human approval
//! must not block `ping`, `tools/list` or other calls. `notifications/cancelled` aborts
//! the matching in-flight request.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot, Mutex};

const PROTOCOL_VERSION: &str = "2024-11-05";

/// One in-flight JSON-RPC request, keyed by the string form of its id.
struct ActiveRequest {
    id: Value,
    cancel: oneshot::Sender<()>,
    /// Guarantees exactly one response even if completion races with cancellation.
    delivered: Arc<AtomicBool>,
}

#[derive(Clone)]
struct Bridge {
    client: reqwest::Client,
    broker_url: Arc<str>,
    token: Arc<str>,
    responses: mpsc::UnboundedSender<Value>,
    active: Arc<Mutex<HashMap<String, ActiveRequest>>>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let broker_url = std::env::var("ORBIT_BROKER_URL")
        .map_err(|_| anyhow::anyhow!("ORBIT_BROKER_URL is required"))?;
    let token = std::env::var("ORBIT_BROKER_TOKEN")
        .map_err(|_| anyhow::anyhow!("ORBIT_BROKER_TOKEN is required"))?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60 * 30))
        .build()?;

    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();
    let (response_tx, mut response_rx) = mpsc::unbounded_channel::<Value>();
    let bridge = Bridge {
        client,
        broker_url: Arc::from(broker_url.as_str()),
        token: Arc::from(token.as_str()),
        responses: response_tx,
        active: Arc::new(Mutex::new(HashMap::new())),
    };

    // A single writer keeps stdout frames from interleaving across concurrent requests.
    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(response) = response_rx.recv().await {
            let mut encoded = serde_json::to_vec(&response)?;
            encoded.push(b'\n');
            stdout.write_all(&encoded).await?;
            stdout.flush().await?;
        }
        Ok::<(), anyhow::Error>(())
    });

    while let Some(line) = lines.next_line().await? {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("orbit-mcp-broker: invalid JSON-RPC: {error}");
                continue;
            }
        };
        bridge.handle_message(request).await;
    }

    // Cancel anything still running: the harness has gone away, and the spawned tasks
    // hold response senders that would otherwise keep the writer alive until their
    // (possibly 30-minute) broker calls time out.
    bridge.shutdown().await;
    drop(bridge);
    writer.await??;
    Ok(())
}

impl Bridge {
    async fn handle_message(&self, request: Value) {
        let method = request.get("method").and_then(Value::as_str).unwrap_or("");

        // Notifications never receive a response, but cancellations target a request.
        let request_id = request
            .get("params")
            .and_then(|params| params.get("requestId").or_else(|| params.get("id")));
        if method == "notifications/cancelled" {
            if let Some(id) = request_id {
                self.cancel(id).await;
            }
            return;
        }

        let Some(id) = request.get("id").cloned() else {
            return;
        };
        let params = request.get("params").cloned().unwrap_or(Value::Null);
        self.dispatch(id, method, params).await;
    }

    async fn dispatch(&self, id: Value, method: &str, params: Value) {
        let key = id.to_string();
        let (cancel_tx, cancel_rx) = oneshot::channel();
        let delivered = Arc::new(AtomicBool::new(false));
        {
            let mut active = self.active.lock().await;
            if let Some(previous) = active.insert(
                key.clone(),
                ActiveRequest {
                    id: id.clone(),
                    cancel: cancel_tx,
                    delivered: delivered.clone(),
                },
            ) {
                // A harness retry with the same id must not produce two writes.
                let _ = previous.cancel.send(());
                previous.delivered.store(true, Ordering::SeqCst);
            }
        }

        let task = self.clone();
        let task_delivered = delivered.clone();
        let task_key = key.clone();
        let method = method.to_string();
        tokio::spawn(async move {
            let work = handle_method(&task, id, &method, params);
            let response = tokio::select! {
                _ = cancel_rx => None,
                response = work => Some(response),
            };
            if let Some(response) = response {
                if !task_delivered.swap(true, Ordering::SeqCst) {
                    let _ = task.responses.send(response);
                }
            }
            let mut active = task.active.lock().await;
            if active
                .get(&task_key)
                .is_some_and(|entry| Arc::ptr_eq(&entry.delivered, &task_delivered))
            {
                active.remove(&task_key);
            }
        });
    }

    async fn cancel(&self, id: &Value) {
        let key = id.to_string();
        if let Some(active) = self.active.lock().await.remove(&key) {
            let _ = active.cancel.send(());
            if !active.delivered.swap(true, Ordering::SeqCst) {
                let _ = self.responses.send(json!({
                    "jsonrpc": "2.0",
                    "id": active.id,
                    "error": { "code": -32800, "message": "Request cancelled" }
                }));
            }
        }
    }

    /// Stop all in-flight work when stdin closes (the harness has exited). No
    /// cancellation responses are sent — nobody is listening on stdout any more.
    async fn shutdown(&self) {
        let active: Vec<_> = self
            .active
            .lock()
            .await
            .drain()
            .map(|(_, entry)| entry)
            .collect();
        for entry in active {
            let _ = entry.cancel.send(());
            entry.delivered.store(true, Ordering::SeqCst);
        }
    }
}

async fn handle_method(bridge: &Bridge, id: Value, method: &str, params: Value) -> Value {
    match method {
        "initialize" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "orbit-broker", "version": env!("CARGO_PKG_VERSION") }
            }
        }),
        "ping" => json!({ "jsonrpc": "2.0", "id": id, "result": {} }),
        "tools/list" => {
            match broker_get(&bridge.client, &bridge.broker_url, &bridge.token, "/tools").await {
                Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                Err(error) => tool_bridge_error(id, &error),
            }
        }
        "tools/call" => {
            match broker_post(
                &bridge.client,
                &bridge.broker_url,
                &bridge.token,
                "/tools/call",
                &params,
            )
            .await
            {
                Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                Err(error) => tool_bridge_error(id, &error),
            }
        }
        other => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32601, "message": format!("method not found: {other}") }
        }),
    }
}

fn tool_bridge_error(id: Value, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{ "type": "text", "text": format!("ORBIT tool broker error: {message}") }],
            "isError": true
        }
    })
}

async fn broker_get(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    path: &str,
) -> Result<Value, String> {
    let response = client
        .get(format!("{}{}", base_url.trim_end_matches('/'), path))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| format!("broker request failed: {e}"))?;
    parse_response(response).await
}

async fn broker_post(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    path: &str,
    body: &Value,
) -> Result<Value, String> {
    let response = client
        .post(format!("{}{}", base_url.trim_end_matches('/'), path))
        .bearer_auth(token)
        .json(body)
        .send()
        .await
        .map_err(|e| format!("broker request failed: {e}"))?;
    parse_response(response).await
}

async fn parse_response(response: reqwest::Response) -> Result<Value, String> {
    let status = response.status();
    let body: Value = response
        .json()
        .await
        .map_err(|e| format!("broker returned invalid JSON: {e}"))?;
    if !status.is_success() {
        let message = body
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("broker request failed");
        return Err(format!("{status}: {message}"));
    }
    Ok(body)
}
