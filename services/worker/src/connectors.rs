//! Connector context hydration for the execution plane.
//!
//! MVP scope: read-only X/Twitter recent-search hydration. Before an execution starts,
//! configured X connector bindings fetch recent posts and add them to the harness input
//! as explicitly **untrusted** external context. The harness (OpenCode) then summarises
//! or analyses the posts. Writes/posting and general capability invocation are Phase 2.

use serde_json::{json, Value};

use orbit_contracts::{HarnessError, HarnessErrorKind};
use orbit_domain::{
    evaluate_capability, evaluate_tool_call, AgentSpec, AutonomyLevel, CapabilityEffect,
    ExecutionEventKind, GrantedCapability, LogLevel, PolicyDecision, ToolCall, ToolPolicyOutcome,
    ToolPolicyRule,
};

use crate::client::WorkerClient;

pub struct HydratedContext {
    pub payload: Value,
    pub events: Vec<HydrationEvent>,
}

/// One execution event about hydration: a note, or the `posts.search` call itself.
pub struct HydrationEvent {
    pub kind: ExecutionEventKind,
    pub level: LogLevel,
    pub message: String,
    pub data: Value,
}

impl HydrationEvent {
    fn note(message: impl Into<String>) -> Self {
        Self {
            kind: ExecutionEventKind::Log,
            level: LogLevel::Info,
            message: message.into(),
            data: json!({ "connector": "x" }),
        }
    }
}

/// The search query: from the execution payload or the connection config. The task text
/// is never used: it is the run instruction, not a search string, and would be sent to a
/// third party (#430).
fn search_query(payload: &Value, connection_config: &Value) -> Option<String> {
    [payload.get("query"), connection_config.get("query")]
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::trim)
        .find(|query| !query.is_empty())
        .map(str::to_string)
}

/// Evaluate hydration exactly like a broker call to `posts.search` on this connection:
/// autonomy and the capability allowlist first, then the organisation's tool policy
/// rules (#430).
fn evaluate_search(
    spec: &AgentSpec,
    rules: &[ToolPolicyRule],
    connection_id: uuid::Uuid,
    connection_name: &str,
    capabilities: &[String],
    query: &str,
) -> ToolPolicyOutcome {
    let grants: Vec<GrantedCapability> = capabilities
        .iter()
        .map(|capability| GrantedCapability {
            connection_id,
            connector: "x".into(),
            capability: capability.clone(),
            effect: if capability == "posts.search" {
                CapabilityEffect::Read
            } else {
                CapabilityEffect::ExternalSideEffect
            },
        })
        .collect();
    let platform = evaluate_capability(
        AutonomyLevel::from_level(spec.autonomy.level).unwrap_or(AutonomyLevel::OBSERVE),
        spec.autonomy.approval_mode,
        "posts.search",
        CapabilityEffect::Read,
        &grants,
    );
    let call = ToolCall {
        tool: "x_posts_search".into(),
        capability: Some("posts.search".into()),
        effect: Some(CapabilityEffect::Read),
        connection: Some(connection_name.to_string()),
        arguments: json!({ "query": query }),
        cwd: None,
    };
    evaluate_tool_call(platform, platform, rules, &call)
}

const X_SEARCH_PATH: &str = "/tweets/search/recent";

/// Fetch external context for connector bindings that are allowed to read.
///
/// - Bindings without a connection, or without a query in the payload or the connection
///   config, are skipped with a note so the run still starts.
/// - The search is evaluated like a broker tool call: a deny rule skips it, and a
///   require-approval rule skips it too (a run cannot wait for approval before it
///   starts). Every search is recorded as a `tool_call` event.
/// - A configured X binding requires a valid connection and credential; failures fail
///   the execution with an actionable error instead of silently omitting data.
pub async fn hydrate(
    client: &WorkerClient,
    execution_id: uuid::Uuid,
    spec: &AgentSpec,
    rules: &[ToolPolicyRule],
    payload: &Value,
) -> Result<HydratedContext, HarnessError> {
    let mut payload = payload.clone();
    if !payload.is_object() {
        payload = json!({});
    }
    let mut events = Vec::new();
    let mut context: Vec<Value> = Vec::new();

    for binding in &spec.connectors {
        if binding.connector != "x" {
            continue;
        }
        if !binding.capabilities.iter().any(|c| c == "posts.search") {
            continue;
        }
        let Some(connection_id) = binding.connection_id else {
            events.push(HydrationEvent::note(
                "X connector is bound without a connection; skipping X context hydration.",
            ));
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
        if !connection.connection.is_usable() {
            return Err(HarnessError::new(
                HarnessErrorKind::InvalidConfig,
                format!(
                    "X connection '{}' is {}",
                    connection.connection.name, connection.connection.status
                ),
            ));
        }
        let credential = connection.credential.as_ref().ok_or_else(|| {
            HarnessError::new(
                HarnessErrorKind::InvalidConfig,
                "X connection has no credential; add a bearer token in Connections",
            )
        })?;

        let Some(query) = search_query(&payload, &connection.connection.config) else {
            events.push(HydrationEvent::note(
                "X context hydration skipped: no query. Pass {\"query\":\"...\"} in the execution \
                 payload or set config.query on the connection.",
            ));
            continue;
        };
        let policy = evaluate_search(
            spec,
            rules,
            connection.connection.id,
            &connection.connection.name,
            &binding.capabilities,
            &query,
        );
        let decision_data = |decision: &str| {
            json!({
                "connector": "x",
                "capability": "posts.search",
                "source": "context hydration",
                "connection": connection.connection.name,
                "query": query,
                "decision": decision,
                "ruleId": policy.rule_id,
                "ruleName": policy.rule_name,
                "reason": policy.reason,
            })
        };
        match policy.decision {
            PolicyDecision::Deny => {
                events.push(HydrationEvent {
                    kind: ExecutionEventKind::ToolCall,
                    level: LogLevel::Warn,
                    message: format!(
                        "X context hydration skipped: posts.search is denied by policy{}",
                        policy
                            .reason
                            .as_deref()
                            .map(|reason| format!(" ({reason})"))
                            .unwrap_or_default()
                    ),
                    data: decision_data("deny"),
                });
                continue;
            }
            PolicyDecision::RequireApproval => {
                events.push(HydrationEvent {
                    kind: ExecutionEventKind::ToolCall,
                    level: LogLevel::Warn,
                    message: "X context hydration skipped: posts.search requires approval, \
                              which a run cannot wait for before it starts"
                        .into(),
                    data: decision_data("require_approval"),
                });
                continue;
            }
            PolicyDecision::Allow => {}
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
        orbit_security::check_outbound_url(&url, &orbit_security::OutboundPolicy::from_env())
            .await
            .map_err(|error| {
                HarnessError::new(
                    HarnessErrorKind::InvalidConfig,
                    format!("X base URL refused ({})", error.category()),
                )
            })?;

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
                    format!("X API request failed: {e}"),
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
        let mut data = decision_data("allow");
        data["results"] = json!(items.len());
        events.push(HydrationEvent {
            kind: ExecutionEventKind::ToolCall,
            level: LogLevel::Info,
            message: format!(
                "X posts.search for context: {} post(s) for query '{}' (read-only, untrusted)",
                items.len(),
                query
            ),
            data,
        });
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
    Ok(HydratedContext { payload, events })
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
    use orbit_domain::{ToolPolicyAction, ToolPolicyMatcher};

    fn spec() -> AgentSpec {
        serde_json::from_value(json!({
            "harness": { "type": "mock" },
            "model": { "management": "harness" },
            "instructions": { "system": "s" },
            "connectors": [], "triggers": [],
            "autonomy": { "level": 1 },
            "resources": { "timeoutSeconds": 30, "maxConcurrentExecutions": 1 }
        }))
        .unwrap()
    }

    fn rule(action: ToolPolicyAction) -> ToolPolicyRule {
        ToolPolicyRule {
            id: uuid::Uuid::now_v7(),
            name: "x search".into(),
            priority: 100,
            enabled: true,
            action,
            matcher: ToolPolicyMatcher {
                capability: Some("posts.search".into()),
                ..Default::default()
            },
            reason: Some("no third-party search".into()),
            impact: None,
        }
    }

    fn evaluate(rules: &[ToolPolicyRule]) -> PolicyDecision {
        evaluate_search(
            &spec(),
            rules,
            uuid::Uuid::now_v7(),
            "x-main",
            &["posts.search".to_string()],
            "orbit",
        )
        .decision
    }

    #[test]
    fn hydration_follows_tool_policy_rules() {
        assert_eq!(evaluate(&[]), PolicyDecision::Allow);
        assert_eq!(
            evaluate(&[rule(ToolPolicyAction::Deny)]),
            PolicyDecision::Deny
        );
        assert_eq!(
            evaluate(&[rule(ToolPolicyAction::RequireApproval)]),
            PolicyDecision::RequireApproval
        );
    }

    #[test]
    fn hydration_needs_the_capability_on_the_binding() {
        let outcome = evaluate_search(
            &spec(),
            &[],
            uuid::Uuid::now_v7(),
            "x-main",
            &["posts.create".to_string()],
            "orbit",
        );
        assert_eq!(outcome.decision, PolicyDecision::Deny);
    }

    #[test]
    fn the_query_never_falls_back_to_the_task() {
        assert_eq!(
            search_query(&json!({ "query": " rust " }), &json!({ "query": "c" })).as_deref(),
            Some("rust")
        );
        assert_eq!(
            search_query(&json!({ "query": "  " }), &json!({ "query": "config" })).as_deref(),
            Some("config")
        );
        assert_eq!(
            search_query(&json!({ "task": "do things" }), &json!({})),
            None
        );
    }

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
