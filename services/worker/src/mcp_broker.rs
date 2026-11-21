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
use tokio::sync::{mpsc, Mutex};
use tokio_util::sync::CancellationToken;

const PROTOCOL_VERSION: &str = "2024-11-05";

/// One in-flight JSON-RPC request, keyed by the string form of its id.
struct ActiveRequest {
    id: Value,
    cancel: Arc<CancellationToken>,
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

pub async fn run() -> anyhow::Result<()> {
    let broker_url = std::env::var("ORBIT_BROKER_URL")
        .map_err(|_| anyhow::anyhow!("ORBIT_BROKER_URL is required"))?;
    let token = std::env::var("ORBIT_BROKER_TOKEN")
        .map_err(|_| anyhow::anyhow!("ORBIT_BROKER_TOKEN is required"))?;
    let client = orbit_security::client_with_custom_ca(Duration::from_secs(60 * 30));

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
        let cancel = Arc::new(CancellationToken::new());
        let delivered = Arc::new(AtomicBool::new(false));
        {
            let mut active = self.active.lock().await;
            if let Some(previous) = active.insert(
                key.clone(),
                ActiveRequest {
                    id: id.clone(),
                    cancel: cancel.clone(),
                    delivered: delivered.clone(),
                },
            ) {
                // A harness retry with the same id must not produce two writes.
                previous.cancel.cancel();
                previous.delivered.store(true, Ordering::SeqCst);
            }
        }

        let task = self.clone();
        let task_cancel = cancel.clone();
        let task_delivered = delivered.clone();
        let task_key = key.clone();
        let method = method.to_string();
        tokio::spawn(async move {
            let work = handle_method(&task, id, &method, params);
            let response = tokio::select! {
                _ = task_cancel.cancelled() => None,
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
                .is_some_and(|entry| Arc::ptr_eq(&entry.cancel, &task_cancel))
            {
                active.remove(&task_key);
            }
        });
    }

    async fn cancel(&self, id: &Value) {
        let key = id.to_string();
        if let Some(active) = self.active.lock().await.remove(&key) {
            active.cancel.cancel();
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
            entry.cancel.cancel();
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
        "tools/call" => match broker_post(
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
        },
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

#[cfg(test)]
mod tests {
    use axum::extract::State;
    use axum::routing::{get, post};
    use axum::{Json, Router};

    use super::*;

    #[derive(Clone)]
    struct FakeBroker {
        call_delay: Duration,
    }

    async fn tools() -> Json<Value> {
        Json(json!({ "tools": [] }))
    }

    async fn tools_call(State(broker): State<FakeBroker>) -> Json<Value> {
        tokio::time::sleep(broker.call_delay).await;
        Json(json!({ "content": [], "isError": false }))
    }

    async fn serve(call_delay: Duration) -> (Bridge, mpsc::UnboundedReceiver<Value>) {
        let app = Router::new()
            .route("/tools", get(tools))
            .route("/tools/call", post(tools_call))
            .with_state(FakeBroker { call_delay });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let (responses, receiver) = mpsc::unbounded_channel();
        let bridge = Bridge {
            client: orbit_security::client_with_custom_ca(Duration::from_secs(5)),
            broker_url: Arc::from(format!("http://{addr}").as_str()),
            token: Arc::from("test-token"),
            responses,
            active: Arc::new(Mutex::new(HashMap::new())),
        };
        (bridge, receiver)
    }

    async fn next_response(receiver: &mut mpsc::UnboundedReceiver<Value>) -> Value {
        tokio::time::timeout(Duration::from_secs(3), receiver.recv())
            .await
            .expect("response timeout")
            .expect("response channel closed")
    }

    #[tokio::test]
    async fn slow_tool_call_does_not_block_ping_or_tools_list() {
        let (bridge, mut responses) = serve(Duration::from_millis(500)).await;
        bridge
            .handle_message(json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "write" }
            }))
            .await;
        bridge
            .handle_message(json!({ "jsonrpc": "2.0", "id": 2, "method": "ping" }))
            .await;
        bridge
            .handle_message(json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list" }))
            .await;

        let mut ids = Vec::new();
        for _ in 0..3 {
            ids.push(next_response(&mut responses).await["id"].as_u64().unwrap());
        }
        assert!(ids.contains(&2), "ping answered while a call was pending");
        assert!(
            ids.contains(&3),
            "tools/list answered while a call was pending"
        );
        assert!(ids.contains(&1), "the slow call eventually answered");
        assert!(bridge.active.lock().await.is_empty());
    }

    #[tokio::test]
    async fn cancellation_notification_answers_and_drops_the_request() {
        let (bridge, mut responses) = serve(Duration::from_secs(30)).await;
        bridge
            .handle_message(json!({
                "jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": { "name": "write" }
            }))
            .await;
        // Give the request task time to register and reach the broker call.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(bridge.active.lock().await.len(), 1);

        bridge
            .handle_message(json!({
                "jsonrpc": "2.0",
                "method": "notifications/cancelled",
                "params": { "requestId": 7, "reason": "harness timeout" }
            }))
            .await;

        let cancelled = next_response(&mut responses).await;
        assert_eq!(cancelled["id"], 7);
        assert_eq!(cancelled["error"]["code"], -32800);
        assert!(bridge.active.lock().await.is_empty());

        // The bridge remains usable after cancellation.
        bridge
            .handle_message(json!({ "jsonrpc": "2.0", "id": 8, "method": "ping" }))
            .await;
        assert_eq!(next_response(&mut responses).await["id"], 8);
    }

    #[tokio::test]
    async fn duplicate_request_id_is_not_executed_twice() {
        let (bridge, mut responses) = serve(Duration::from_secs(30)).await;
        let request = json!({
            "jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": { "name": "write" }
        });
        bridge.handle_message(request.clone()).await;
        bridge.handle_message(request).await;
        assert_eq!(bridge.active.lock().await.len(), 1);

        // The retry cancels the original call; the replacement still completes.
        bridge
            .handle_message(json!({
                "jsonrpc": "2.0",
                "method": "notifications/cancelled",
                "params": { "requestId": 9 }
            }))
            .await;
        let response = next_response(&mut responses).await;
        assert_eq!(response["id"], 9);
        assert_eq!(response["error"]["code"], -32800);
        assert!(bridge.active.lock().await.is_empty());
    }
}
