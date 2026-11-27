//! Optional read-only control-plane MCP server.
//!
//! This is intentionally not the worker tool broker. It exposes a small, bounded selection
//! of ORBIT API reads over MCP stdio so a desktop/IDE client can inspect agents, teams,
//! executions and team runs with the caller's own bearer token.

use std::time::Duration;

use anyhow::{anyhow, Context};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const PROTOCOL_VERSION: &str = "2024-11-05";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let api_url = std::env::var("ORBIT_API_URL")
        .unwrap_or_else(|_| "http://localhost:8080".to_string())
        .trim_end_matches('/')
        .to_string();
    let token = std::env::var("ORBIT_API_TOKEN")
        .context("ORBIT_API_TOKEN is required (use a scoped ORBIT user or API token)")?;
    let client = orbit_security::client_with_custom_ca(Duration::from_secs(30));
    let server = Server {
        api_url,
        token,
        client,
    };

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(request) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(id) = request.get("id").cloned() else {
            continue;
        };
        let method = request.get("method").and_then(Value::as_str).unwrap_or("");
        let params = request.get("params").cloned().unwrap_or(Value::Null);
        let response = match method {
            "initialize" => ok(
                id,
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "orbit-mcp", "version": env!("CARGO_PKG_VERSION") }
                }),
            ),
            "ping" => ok(id, json!({})),
            "tools/list" => ok(id, json!({ "tools": tools() })),
            "tools/call" => match server.call(&params).await {
                Ok(value) => ok(
                    id,
                    json!({
                        "content": [{ "type": "text", "text": serde_json::to_string_pretty(&value)? }],
                        "isError": false
                    }),
                ),
                Err(error) => ok(
                    id,
                    json!({
                        "content": [{ "type": "text", "text": format!("{error:#}") }],
                        "isError": true
                    }),
                ),
            },
            _ => error(id, -32601, "Method not found"),
        };
        stdout
            .write_all(serde_json::to_string(&response)?.as_bytes())
            .await?;
        stdout.write_all(b"\n").await?;
        stdout.flush().await?;
    }
    Ok(())
}

fn ok(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

struct Server {
    api_url: String,
    token: String,
    client: reqwest::Client,
}

impl Server {
    async fn call(&self, params: &Value) -> anyhow::Result<Value> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("tool name is required"))?;
        let args = params.get("arguments").cloned().unwrap_or(json!({}));
        let path = match name {
            "agents.list" => {
                build_path("/api/v1/agents", &args, &["q", "status", "limit", "cursor"])
            }
            "agents.get" => path_id("/api/v1/agents", &args, "agentId")?,
            "teams.list" => build_path("/api/v1/agent-teams", &args, &["limit", "cursor"]),
            "teams.get" => path_id("/api/v1/agent-teams", &args, "teamId")?,
            "executions.list" => build_path(
                "/api/v1/executions",
                &args,
                &["status", "agentId", "limit", "cursor"],
            ),
            "executions.get" => path_id("/api/v1/executions", &args, "executionId")?,
            "executions.events" => {
                let id = required(&args, "executionId")?;
                let mut path = format!("/api/v1/executions/{}/events", encode(id));
                if let Some(limit) = args.get("limit") {
                    path.push_str(&format!("?limit={}", limit));
                }
                path
            }
            "team_runs.list" => build_path(
                "/api/v1/team-runs",
                &args,
                &["teamId", "status", "limit", "cursor"],
            ),
            "team_runs.get" => path_id("/api/v1/team-runs", &args, "runId")?,
            other => return Err(anyhow!("unknown read-only tool '{other}'")),
        };
        let response = self
            .client
            .get(format!("{}{}", self.api_url, path))
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|error| anyhow!("ORBIT API request failed: {error}"))?;
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow!("ORBIT API returned HTTP {status}: {body}"));
        }
        Ok(serde_json::from_str(&body).unwrap_or(Value::String(body)))
    }
}

fn required<'a>(args: &'a Value, key: &str) -> anyhow::Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("{key} is required"))
}

fn path_id(base: &str, args: &Value, key: &str) -> anyhow::Result<String> {
    Ok(format!("{}/{}", base, encode(required(args, key)?)))
}

fn encode(value: &str) -> String {
    urlencoding(value)
}

fn urlencoding(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn build_path(base: &str, args: &Value, keys: &[&str]) -> String {
    let mut params = Vec::new();
    for key in keys {
        if let Some(value) = args.get(*key) {
            let text = match value {
                Value::String(text) => text.clone(),
                Value::Number(number) => number.to_string(),
                Value::Bool(flag) => flag.to_string(),
                _ => continue,
            };
            if !text.is_empty() {
                params.push(format!("{}={}", encode(key), encode(&text)));
            }
        }
    }
    if params.is_empty() {
        base.to_string()
    } else {
        format!("{base}?{}", params.join("&"))
    }
}

fn tools() -> Vec<Value> {
    vec![
        tool(
            "agents.list",
            "List agents visible to the caller.",
            json!({
                "type": "object",
                "properties": {
                    "q": { "type": "string" },
                    "status": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 100 },
                    "cursor": { "type": "string" }
                }
            }),
        ),
        tool(
            "agents.get",
            "Read one agent.",
            json!({
                "type": "object", "properties": { "agentId": { "type": "string" } }, "required": ["agentId"]
            }),
        ),
        tool(
            "teams.list",
            "List agent teams visible to the caller.",
            json!({
                "type": "object",
                "properties": {
                    "limit": { "type": "integer", "minimum": 1, "maximum": 100 },
                    "cursor": { "type": "string" }
                }
            }),
        ),
        tool(
            "teams.get",
            "Read one agent team.",
            json!({
                "type": "object", "properties": { "teamId": { "type": "string" } }, "required": ["teamId"]
            }),
        ),
        tool(
            "executions.list",
            "List executions visible to the caller.",
            json!({
                "type": "object",
                "properties": {
                    "status": { "type": "string" },
                    "agentId": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 100 },
                    "cursor": { "type": "string" }
                }
            }),
        ),
        tool(
            "executions.get",
            "Read one execution.",
            json!({
                "type": "object", "properties": { "executionId": { "type": "string" } }, "required": ["executionId"]
            }),
        ),
        tool(
            "executions.events",
            "Read bounded execution events.",
            json!({
                "type": "object",
                "properties": {
                    "executionId": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 500 }
                },
                "required": ["executionId"]
            }),
        ),
        tool(
            "team_runs.list",
            "List team runs visible to the caller.",
            json!({
                "type": "object",
                "properties": {
                    "teamId": { "type": "string" },
                    "status": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 100 },
                    "cursor": { "type": "string" }
                }
            }),
        ),
        tool(
            "team_runs.get",
            "Read one team run and its steps.",
            json!({
                "type": "object", "properties": { "runId": { "type": "string" } }, "required": ["runId"]
            }),
        ),
    ]
}

fn tool(name: &str, description: &str, input_schema: Value) -> Value {
    json!({ "name": name, "description": description, "inputSchema": input_schema })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_builder_encodes_and_skips_empty_values() {
        let path = build_path(
            "/api/v1/agents",
            &json!({ "q": "a b", "status": "", "limit": 2 }),
            &["q", "status", "limit"],
        );
        assert_eq!(path, "/api/v1/agents?q=a%20b&limit=2");
    }

    #[test]
    fn id_paths_require_and_encode_ids() {
        assert_eq!(
            path_id("/api/v1/agents", &json!({ "agentId": "a/b" }), "agentId").unwrap(),
            "/api/v1/agents/a%2Fb"
        );
        assert!(path_id("/api/v1/agents", &json!({}), "agentId").is_err());
    }

    #[test]
    fn tool_list_is_read_only() {
        let names: Vec<String> = tools()
            .into_iter()
            .filter_map(|tool| tool["name"].as_str().map(str::to_string))
            .collect();
        assert!(names.iter().any(|name| name == "agents.list"));
        assert!(!names
            .iter()
            .any(|name| name.contains("start") || name.contains("cancel")));
    }
}
