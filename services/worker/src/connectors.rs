//! Connector context hydration for the execution plane.
//!
//! MVP scope: read-only X/Twitter recent-search hydration. Before an execution starts,
//! configured X connector bindings fetch recent posts and add them to the harness input
//! as explicitly **untrusted** external context. The harness (OpenCode) then summarises
//! or analyses the posts. Writes/posting and general capability invocation are Phase 2.

use serde_json::{json, Value};

use orbit_contracts::{HarnessError, HarnessErrorKind};
use orbit_domain::AgentSpec;

use crate::client::WorkerClient;

pub struct HydratedContext {
    pub payload: Value,
    pub notes: Vec<String>,
}

const X_SEARCH_PATH: &str = "/tweets/search/recent";

/// Fetch external context for connector bindings that are allowed to read.
///
/// - Bindings without a connection are skipped (with a note) so demo agents still run.
/// - A configured X binding requires a valid connection, credential and query; failures
///   fail the execution with an actionable error instead of silently omitting data.
pub async fn hydrate(
    client: &WorkerClient,
    execution_id: uuid::Uuid,
    spec: &AgentSpec,
    task: &str,
    payload: &Value,
) -> Result<HydratedContext, HarnessError> {
    let mut payload = payload.clone();
    if !payload.is_object() {
        payload = json!({});
    }
    let mut notes = Vec::new();
    let mut context: Vec<Value> = Vec::new();

    for binding in &spec.connectors {
        if binding.connector != "x" {
            continue;
        }
        if !binding.capabilities.iter().any(|c| c == "posts.search") {
            continue;
        }
        let Some(connection_id) = binding.connection_id else {
            notes.push(
                "X connector is bound without a connection; skipping X context hydration."
                    .to_string(),
            );
            continue;
        };
        let connection = client
            .connection(execution_id, connection_id)
            .await
            .map_err(|err| {
                HarnessError::new(
                    HarnessErrorKind::InvalidConfig,
                    format!("X connection not found: {err}"),
                )
            })?;
        if connection.connector.slug != "x" {
            return Err(HarnessError::new(
                HarnessErrorKind::InvalidConfig,
                format!(
                    "connection '{}' belongs to connector '{}', not X",
                    connection.connection.name, connection.connector.slug
                ),
            ));
        }
        if connection.connection.status != "active" {
            return Err(HarnessError::new(
                HarnessErrorKind::InvalidConfig,
                format!(
                    "X connection '{}' is not active",
                    connection.connection.name
                ),
            ));
        }
        let credential = connection.credential.as_ref().ok_or_else(|| {
            HarnessError::new(
                HarnessErrorKind::InvalidConfig,
                "X connection has no credential; add a bearer token in Connections",
            )
        })?;

        let query = payload
            .get("query")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| {
                connection
                    .connection
                    .config
                    .get("query")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            })
            .filter(|q| !q.trim().is_empty())
            .unwrap_or_else(|| task.trim().to_string());
        if query.is_empty() {
            return Err(HarnessError::new(
                HarnessErrorKind::InvalidConfig,
                "X search requires a query: pass {\"query\":\"...\"} in the execution payload \
                 or set config.query on the connection",
            ));
        }

        let base_url = connection
            .connection
            .config
            .get("baseUrl")
            .and_then(|v| v.as_str())
            .unwrap_or("https://api.x.com/2")
            .trim_end_matches('/')
            .to_string();
        let url = format!("{base_url}{X_SEARCH_PATH}");

        // Guarded: the base URL is tenant-supplied and the request carries a credential.
        let client = orbit_security::guarded_client(std::time::Duration::from_secs(20));
        let response = client
            .get(&url)
            .bearer_auth(&credential.value)
            .query(&[
                ("query", query.as_str()),
                ("max_results", "10"),
                ("tweet.fields", "created_at,author_id,public_metrics,lang"),
            ])
            .send()
            .await
            .map_err(|e| {
                HarnessError::new(
                    HarnessErrorKind::Unavailable,
                    format!(
                        "X API request failed: {}",
                        orbit_security::describe_request_error(&e)
                    ),
                )
            })?;

        let status = response.status();
        let body: Value = response.json().await.unwrap_or_else(|_| json!({}));
        if !status.is_success() {
            let detail = body
                .get("detail")
                .or_else(|| body.get("title"))
                .and_then(|v| v.as_str())
                .unwrap_or("X API error");
            let hint = if status.as_u16() == 401 || status.as_u16() == 403 {
                " Check that the bearer token is valid and that your X API plan allows recent search."
            } else {
                ""
            };
            return Err(HarnessError::new(
                HarnessErrorKind::Unavailable,
                format!("X API returned {status}: {detail}.{hint}"),
            ));
        }

        let items = parse_x_posts(&body);
        notes.push(format!(
            "Fetched {} X post(s) for query '{}' (read-only, untrusted context).",
            items.len(),
            query
        ));
        context.push(json!({
            "connector": "x",
            "capability": "posts.search",
            "query": query,
            "retrievedAt": chrono::Utc::now(),
            "connectionId": connection.connection.id,
            "items": items,
            "provenance": { "source": "x", "trusted": false }
        }));
    }

    if !context.is_empty() {
        payload["externalContext"] = Value::Array(context);
    }
    Ok(HydratedContext { payload, notes })
}

fn parse_x_posts(body: &Value) -> Vec<Value> {
    body.get("data")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .take(10)
                .map(|item| {
                    json!({
                        "id": item.get("id"),
                        "text": item.get("text").and_then(Value::as_str).map(|t| t.chars().take(2000).collect::<String>()),
                        "authorId": item.get("author_id"),
                        "createdAt": item.get("created_at"),
                        "lang": item.get("lang"),
                        "metrics": item.get("public_metrics"),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_x_response_and_caps_text() {
        let body = json!({
            "data": [{
                "id": "1",
                "text": "hello world",
                "author_id": "42",
                "created_at": "2026-01-01T00:00:00Z",
                "public_metrics": { "like_count": 2 }
            }],
            "meta": { "result_count": 1 }
        });
        let posts = parse_x_posts(&body);
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0]["authorId"], "42");
        assert_eq!(posts[0]["text"], "hello world");
        assert!(parse_x_posts(&json!({})).is_empty());
    }
}
