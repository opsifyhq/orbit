//! Execution-scoped, policy-enforcing tool broker.
//!
//! The agent harness talks MCP to `orbit-mcp-broker`, which forwards tool calls to this
//! loopback HTTP broker. The broker validates the execution token, enforces autonomy and
//! capability allowlists, pauses for approvals, leases credentials, invokes connectors and
//! writes audit/execution events. The harness never receives provider credentials.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use orbit_contracts::x_posts_create_input_schema;
use orbit_domain::{
    evaluate_capability, evaluate_tool_call, AgentSpec, ApprovalMode, AutonomyLevel,
    CapabilityEffect, GrantedCapability, LogLevel, PolicyDecision, ToolCall, ToolPolicyOutcome,
    ToolPolicyRule,
};
use orbit_store::{Approval, Connection, ExecutionRecord};

use crate::client::WorkerClient;
use crate::github::{self, GitHubClientConfig};
use crate::gitlab::{self, GitLabClientConfig};
use crate::x::{self, XClientConfig};
use orbit_kubernetes::KubernetesClientConfig;

#[derive(Debug, Clone)]
struct ToolSpec {
    capability: String,
    name: String,
    description: String,
    effect: CapabilityEffect,
    input_schema: Value,
}

/// Execution-plane collaborators, grouped so the injectable test entry point stays small.
struct BrokerRuntime {
    http: reqwest::Client,
    pause: orbit_contracts::PauseClock,
    cancel: tokio_util::sync::CancellationToken,
}

struct BrokerState {
    client: WorkerClient,
    execution: ExecutionRecord,
    workspace: std::path::PathBuf,
    autonomy: AutonomyLevel,
    approval_mode: ApprovalMode,
    connection: Option<Connection>,
    connector_slug: String,
    connector_client: Option<ConnectorClient>,
    tools: Vec<ToolSpec>,
    tool_policies: Vec<ToolPolicyRule>,
    token: String,
    http: reqwest::Client,
    repo_specs: Vec<crate::git::RepoSpec>,
    /// The run's clock; paused while a person decides on an approval.
    pause: orbit_contracts::PauseClock,
    /// The execution's cancellation token: aborts in-flight git commit/push.
    cancel: tokio_util::sync::CancellationToken,
    /// Actions a person rejected in this run (capability + arguments). The same
    /// action is refused without asking again.
    rejected: std::sync::Mutex<std::collections::HashSet<String>>,
}

enum ConnectorClient {
    Github(GitHubClientConfig),
    Gitlab(GitLabClientConfig),
    Kubernetes(KubernetesClientConfig),
    X(XClientConfig),
}

impl BrokerState {
    fn connection_id(&self) -> Result<uuid::Uuid, String> {
        self.connection
            .as_ref()
            .map(|connection| connection.id)
            .ok_or_else(|| "execution has no connector connection".to_string())
    }

    fn repo_target(
        &self,
        repository: Option<&str>,
    ) -> Result<(std::path::PathBuf, uuid::Uuid), String> {
        if self.repo_specs.is_empty() {
            return Ok((self.workspace.clone(), self.connection_id()?));
        }
        if self.repo_specs.len() == 1 {
            let spec = &self.repo_specs[0];
            let path = if spec.path.is_empty() {
                self.workspace.clone()
            } else {
                self.workspace.join(&spec.path)
            };
            return Ok((path, spec.connection_id.unwrap_or(self.connection_id()?)));
        }
        let repository = repository
            .ok_or_else(|| {
                "multi-repository workspace requires a 'repository' argument (owner/repo)"
                    .to_string()
            })?
            .trim();
        let spec = self
            .repo_specs
            .iter()
            .find(|spec| spec.repository.eq_ignore_ascii_case(repository))
            .ok_or_else(|| {
                format!("repository '{repository}' is not part of this team workspace")
            })?;
        let path = if spec.primary && spec.path.is_empty() {
            self.workspace.clone()
        } else if spec.path.is_empty() {
            self.workspace
                .join(spec.repository.split('/').next_back().unwrap_or("repo"))
        } else {
            self.workspace.join(&spec.path)
        };
        Ok((path, spec.connection_id.unwrap_or(self.connection_id()?)))
    }
}

pub struct BrokerHandle {
    pub url: String,
    pub token: String,
    pub tool_count: usize,
    task: tokio::task::JoinHandle<()>,
}

impl BrokerHandle {
    pub fn mcp_command(&self) -> Vec<String> {
        let bin = std::env::var("ORBIT_MCP_BROKER_BIN")
            .unwrap_or_else(|_| "/usr/local/bin/orbit-worker".to_string());
        if bin.ends_with("orbit-worker") || bin.ends_with("orbit-worker.exe") {
            vec![bin, "mcp-broker".to_string()]
        } else {
            vec![bin]
        }
    }
}

impl Drop for BrokerHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn github_tool_specs(
    capabilities: &[String],
    effects: &std::collections::BTreeMap<String, CapabilityEffect>,
) -> Vec<ToolSpec> {
    capabilities
        .iter()
        .map(|capability| {
            let tool = format!("github_{}", capability.replace('.', "_"));
            let effect = effects
                .get(capability)
                .copied()
                .unwrap_or(CapabilityEffect::Read);
            let input_schema = match capability.as_str() {
                "issues.list" => json!({
                    "type": "object",
                    "properties": {
                        "owner": { "type": "string" },
                        "repo": { "type": "string" },
                        "state": { "type": "string", "enum": ["open", "closed", "all"] },
                        "labels": { "type": "string", "description": "Comma-separated labels" }
                    }
                }),
                "issues.get" => json!({
                    "type": "object",
                    "properties": {
                        "owner": { "type": "string" }, "repo": { "type": "string" },
                        "number": { "type": "integer", "minimum": 1 }
                    },
                    "required": ["number"]
                }),
                "issues.comment" => json!({
                    "type": "object",
                    "properties": {
                        "owner": { "type": "string" }, "repo": { "type": "string" },
                        "number": { "type": "integer", "minimum": 1 },
                        "body": { "type": "string", "minLength": 1, "maxLength": 65536 }
                    },
                    "required": ["number", "body"]
                }),
                "issues.create" => json!({
                    "type": "object",
                    "properties": {
                        "owner": { "type": "string" }, "repo": { "type": "string" },
                        "title": { "type": "string", "minLength": 1, "maxLength": 256 },
                        "body": { "type": "string", "maxLength": 65536 },
                        "labels": { "type": "array", "items": { "type": "string" } }
                    },
                    "required": ["title"]
                }),
                "issues.update" => json!({
                    "type": "object",
                    "properties": {
                        "owner": { "type": "string" }, "repo": { "type": "string" },
                        "number": { "type": "integer", "minimum": 1 },
                        "title": { "type": "string" },
                        "body": { "type": "string" },
                        "state": { "type": "string", "enum": ["open", "closed"] },
                        "labels": { "type": "array", "items": { "type": "string" } },
                        "assignees": { "type": "array", "items": { "type": "string" } }
                    },
                    "required": ["number"]
                }),
                "pulls.list" => json!({
                    "type": "object",
                    "properties": {
                        "owner": { "type": "string" }, "repo": { "type": "string" },
                        "state": { "type": "string", "enum": ["open", "closed", "all"] }
                    }
                }),
                "pulls.get" => json!({
                    "type": "object",
                    "properties": {
                        "owner": { "type": "string" }, "repo": { "type": "string" },
                        "number": { "type": "integer", "minimum": 1 }
                    },
                    "required": ["number"]
                }),
                "pulls.create" => json!({
                    "type": "object",
                    "properties": {
                        "repository": { "type": "string", "description": "owner/repo; alternative to owner+repo" },
                        "owner": { "type": "string" }, "repo": { "type": "string" },
                        "title": { "type": "string", "minLength": 1, "maxLength": 256 },
                        "head": { "type": "string", "description": "Source branch" },
                        "base": { "type": "string", "default": "main" },
                        "body": { "type": "string" },
                        "draft": { "type": "boolean" }
                    },
                    "required": ["title", "head"]
                }),
                "checks.list" => json!({
                    "type": "object",
                    "properties": {
                        "owner": { "type": "string" }, "repo": { "type": "string" },
                        "ref": { "type": "string", "description": "Commit SHA or branch" }
                    },
                    "required": ["ref"]
                }),
                "repo.commit" => json!({
                    "type": "object",
                    "properties": {
                        "repository": { "type": "string", "description": "owner/repo; required for multi-repo workspaces" },
                        "message": { "type": "string", "minLength": 1, "maxLength": 2000 }
                    },
                    "required": ["message"]
                }),
                "repo.push" => json!({
                    "type": "object",
                    "properties": {
                        "repository": { "type": "string", "description": "owner/repo; required for multi-repo workspaces" },
                        "branch": { "type": "string", "minLength": 1, "maxLength": 200 }
                    },
                    "required": ["branch"]
                }),
                _ => json!({ "type": "object", "properties": {} }),
            };
            let description = match capability.as_str() {
                "issues.list" => "List open issues in a GitHub repository (read-only).",
                "issues.get" => "Read one GitHub issue (read-only).",
                "issues.comment" => {
                    "Post a comment on a GitHub issue. Write action, may require human approval."
                }
                "issues.create" => {
                    "Create a GitHub issue. Write action, may require human approval."
                }
                "issues.update" => {
                    "Update a GitHub issue, including state, labels and assignees. Write action."
                }
                "pulls.list" => "List pull requests in a repository (read-only).",
                "pulls.get" => "Read one pull request (read-only).",
                "pulls.create" => {
                    "Create a pull request from a branch. Write action, may require human approval."
                }
                "checks.list" => "Read CI check runs for a commit or branch (read-only).",
                "repo.commit" => "Commit all workspace changes. Write action.",
                "repo.push" => "Push the current workspace HEAD to a branch. External side effect.",
                _ => "GitHub capability.",
            };
            ToolSpec {
                capability: capability.clone(),
                name: tool,
                description: description.to_string(),
                effect,
                input_schema,
            }
        })
        .collect()
}

fn kubernetes_tool_specs(
    capabilities: &[String],
    effects: &std::collections::BTreeMap<String, CapabilityEffect>,
) -> Vec<ToolSpec> {
    capabilities
        .iter()
        .map(|capability| {
            let effect = effects
                .get(capability)
                .copied()
                .unwrap_or(CapabilityEffect::Read);
            let input_schema = match capability.as_str() {
                "namespaces.list" => json!({ "type": "object", "properties": {} }),
                "pods.list" => json!({
                    "type": "object",
                    "properties": {
                        "namespace": { "type": "string" },
                        "labelSelector": { "type": "string" },
                        "fieldSelector": { "type": "string" }
                    }
                }),
                "pods.get" => json!({
                    "type": "object",
                    "properties": { "namespace": { "type": "string" }, "name": { "type": "string" } },
                    "required": ["name"]
                }),
                "pods.logs" => json!({
                    "type": "object",
                    "properties": {
                        "namespace": { "type": "string" },
                        "name": { "type": "string" },
                        "tailLines": { "type": "integer" },
                        "previous": { "type": "boolean" }
                    },
                    "required": ["name"]
                }),
                "deployments.list" => json!({
                    "type": "object",
                    "properties": { "namespace": { "type": "string" } }
                }),
                "deployments.get" => json!({
                    "type": "object",
                    "properties": { "namespace": { "type": "string" }, "name": { "type": "string" } },
                    "required": ["name"]
                }),
                "events.list" => json!({
                    "type": "object",
                    "properties": { "namespace": { "type": "string" }, "fieldSelector": { "type": "string" } }
                }),
                _ => json!({ "type": "object" }),
            };
            ToolSpec {
                capability: capability.clone(),
                name: format!("kubernetes_{}", capability.replace('.', "_")),
                description: format!("Kubernetes read capability {capability}."),
                effect,
                input_schema,
            }
        })
        .collect()
}

/// X broker tools. `posts.search` is intentionally excluded: it stays a read-only
/// pre-execution hydration step in `crate::connectors`, not a model-callable tool.
fn x_tool_specs(
    capabilities: &[String],
    effects: &std::collections::BTreeMap<String, CapabilityEffect>,
) -> Vec<ToolSpec> {
    capabilities
        .iter()
        .filter(|capability| capability.as_str() == "posts.create")
        .map(|capability| {
            // Posting publishes externally regardless of a stale/corrupt manifest effect.
            let effect = CapabilityEffect::ExternalSideEffect;
            let declared = effects.get(capability).copied();
            if declared.is_some_and(|declared| declared != effect) {
                tracing::warn!(
                    capability = %capability,
                    "X posts.create manifest effect is not external_side_effect; enforcing the platform effect"
                );
            }
            ToolSpec {
                capability: capability.clone(),
                name: format!("x_{}", capability.replace('.', "_")),
                description: "Publish an X post or reply. Requires a user-context access token \
                     with tweet.write; app-only bearer tokens cannot post. External side effect."
                    .to_string(),
                effect,
                input_schema: x_posts_create_input_schema(),
            }
        })
        .collect()
}

fn gitlab_tool_specs(
    capabilities: &[String],
    effects: &std::collections::BTreeMap<String, CapabilityEffect>,
) -> Vec<ToolSpec> {
    capabilities
        .iter()
        .map(|capability| {
            let tool = format!("gitlab_{}", capability.replace('.', "_"));
            let effect = effects
                .get(capability)
                .copied()
                .unwrap_or(CapabilityEffect::Read);
            let input_schema = match capability.as_str() {
                "projects.list" => json!({ "type": "object", "properties": {} }),
                "group_issues.list" => json!({
                    "type": "object",
                    "properties": {
                        "group": { "type": "string" },
                        "state": { "type": "string" },
                        "labels": { "type": "string" },
                        "search": { "type": "string" }
                    }
                }),
                "boards.list" => json!({
                    "type": "object",
                    "properties": { "group": { "type": "string" } }
                }),
                "issues.list" => json!({
                    "type": "object",
                    "properties": {
                        "project": { "type": "string" },
                        "state": { "type": "string" },
                        "labels": { "type": "string" },
                        "search": { "type": "string" }
                    }
                }),
                "issues.get" => json!({
                    "type": "object",
                    "properties": {
                        "project": { "type": "string" },
                        "iid": { "type": "integer", "minimum": 1 }
                    },
                    "required": ["iid"]
                }),
                "issues.create" => json!({
                    "type": "object",
                    "properties": {
                        "project": { "type": "string" },
                        "title": { "type": "string" },
                        "description": { "type": "string" },
                        "labels": { "type": "array", "items": { "type": "string" } },
                        "confidential": { "type": "boolean" }
                    },
                    "required": ["title"]
                }),
                "issues.update" => json!({
                    "type": "object",
                    "properties": {
                        "project": { "type": "string" },
                        "iid": { "type": "integer", "minimum": 1 },
                        "title": { "type": "string" },
                        "description": { "type": "string" },
                        "labels": { "type": "array", "items": { "type": "string" } },
                        "state_event": { "type": "string", "enum": ["close", "reopen"] },
                        "assigneeIds": { "type": "array", "items": { "type": "integer" } }
                    },
                    "required": ["iid"]
                }),
                "issues.comment" => json!({
                    "type": "object",
                    "properties": {
                        "project": { "type": "string" },
                        "iid": { "type": "integer", "minimum": 1 },
                        "body": { "type": "string" }
                    },
                    "required": ["iid", "body"]
                }),
                "merge_requests.list" => json!({
                    "type": "object",
                    "properties": {
                        "project": { "type": "string" },
                        "state": { "type": "string" }
                    }
                }),
                "merge_requests.get" => json!({
                    "type": "object",
                    "properties": {
                        "project": { "type": "string" },
                        "iid": { "type": "integer", "minimum": 1 }
                    },
                    "required": ["iid"]
                }),
                "merge_requests.create" => json!({
                    "type": "object",
                    "properties": {
                        "project": { "type": "string" },
                        "title": { "type": "string" },
                        "sourceBranch": { "type": "string" },
                        "targetBranch": { "type": "string" },
                        "description": { "type": "string" },
                        "draft": { "type": "boolean" }
                    },
                    "required": ["title", "sourceBranch"]
                }),
                "merge_requests.comment" => json!({
                    "type": "object",
                    "properties": {
                        "project": { "type": "string" },
                        "iid": { "type": "integer", "minimum": 1 },
                        "body": { "type": "string" }
                    },
                    "required": ["iid", "body"]
                }),
                "merge_requests.approve" => json!({
                    "type": "object",
                    "properties": {
                        "project": { "type": "string" },
                        "iid": { "type": "integer", "minimum": 1 }
                    },
                    "required": ["iid"]
                }),
                "merge_requests.merge" => json!({
                    "type": "object",
                    "properties": {
                        "project": { "type": "string" },
                        "iid": { "type": "integer", "minimum": 1 },
                        "sha": { "type": "string" },
                        "mergeCommitMessage": { "type": "string" }
                    },
                    "required": ["iid"]
                }),
                "pipelines.status" => json!({
                    "type": "object",
                    "properties": {
                        "project": { "type": "string" },
                        "ref": { "type": "string" }
                    }
                }),
                "repo.commit" => json!({
                    "type": "object",
                    "properties": {
                        "repository": { "type": "string", "description": "project path; required for multi-repo workspaces" },
                        "message": { "type": "string" }
                    },
                    "required": ["message"]
                }),
                "repo.push" => json!({
                    "type": "object",
                    "properties": {
                        "repository": { "type": "string", "description": "project path; required for multi-repo workspaces" },
                        "branch": { "type": "string" }
                    },
                    "required": ["branch"]
                }),
                _ => json!({ "type": "object", "properties": {} }),
            };
            let description = match capability.as_str() {
                "projects.list" => "List accessible GitLab projects.",
                "group_issues.list" => "List issues across a GitLab group or group board.",
                "boards.list" => "List issue boards for a GitLab group.",
                "issues.list" => "List issues in a GitLab project.",
                "issues.get" => "Read one GitLab issue.",
                "issues.create" => "Create a GitLab issue.",
                "issues.update" => "Update a GitLab issue, labels or state.",
                "issues.comment" => "Post a note on a GitLab issue.",
                "merge_requests.list" => "List GitLab merge requests.",
                "merge_requests.get" => "Read one GitLab merge request.",
                "merge_requests.create" => "Create a GitLab merge request.",
                "merge_requests.comment" => "Post a note on a GitLab merge request.",
                "merge_requests.approve" => "Approve a GitLab merge request.",
                "merge_requests.merge" => "Merge a GitLab merge request.",
                "pipelines.status" => "Read the latest GitLab pipeline status.",
                "repo.commit" => "Commit workspace changes.",
                "repo.push" => "Push the workspace branch to GitLab.",
                _ => "GitLab capability.",
            };
            ToolSpec {
                capability: capability.clone(),
                name: tool,
                description: description.to_string(),
                effect,
                input_schema,
            }
        })
        .collect()
}

/// Start the broker for one execution. Starts when the agent has connector tools or
/// tool-policy rules (native harness tools use the `/policy` endpoint).
pub async fn start(
    client: WorkerClient,
    spec: &AgentSpec,
    execution: &ExecutionRecord,
    workspace: &std::path::Path,
    tool_policies: &[ToolPolicyRule],
    pause: orbit_contracts::PauseClock,
    cancel: tokio_util::sync::CancellationToken,
) -> anyhow::Result<Option<BrokerHandle>> {
    start_with_http(
        client,
        spec,
        execution,
        workspace,
        tool_policies,
        BrokerRuntime {
            http: orbit_security::guarded_client(Duration::from_secs(30)),
            pause,
            cancel,
        },
    )
    .await
}

/// Same as [`start`] with injectable connector HTTP policy (used by the mock-server tests).
async fn start_with_http(
    client: WorkerClient,
    spec: &AgentSpec,
    execution: &ExecutionRecord,
    workspace: &std::path::Path,
    tool_policies: &[ToolPolicyRule],
    runtime: BrokerRuntime,
) -> anyhow::Result<Option<BrokerHandle>> {
    let binding = spec.connectors.iter().find(|b| {
        matches!(
            b.connector.as_str(),
            "github" | "gitlab" | "kubernetes" | "x"
        ) && !b.capabilities.is_empty()
    });

    let mut connection = None;
    let mut connector_slug = String::new();
    let mut connector_client = None;
    let mut tools = Vec::new();

    if let Some(binding) = binding {
        connector_slug = binding.connector.clone();
        let Some(connection_id) = binding.connection_id else {
            anyhow::bail!(
                "Connector '{connector_slug}' is bound without a connection; select a connection in Agent Studio"
            );
        };
        let worker_connection = client.connection(execution.id, connection_id).await?;
        let connector = worker_connection.connector;
        let conn = worker_connection.connection;
        if !conn.is_usable() {
            anyhow::bail!("{connector_slug} connection is {}", conn.status);
        }

        let mut effects = std::collections::BTreeMap::new();
        if let Some(items) = connector.capabilities.as_array() {
            for item in items {
                if let (Some(id), Some(effect)) = (
                    item.get("id").and_then(Value::as_str),
                    item.get("effect").and_then(Value::as_str),
                ) {
                    if let Some(effect) = CapabilityEffect::parse(effect) {
                        effects.insert(id.to_string(), effect);
                    }
                }
            }
        }
        // Only expose capabilities that exist in the connector manifest.
        let allowed: Vec<String> = binding
            .capabilities
            .iter()
            .filter(|capability| {
                effects.contains_key(*capability) && *capability != "repo.checkout"
            })
            .cloned()
            .collect();
        if !allowed.is_empty() {
            tools = match connector_slug.as_str() {
                "gitlab" => gitlab_tool_specs(&allowed, &effects),
                "kubernetes" => kubernetes_tool_specs(&allowed, &effects),
                "x" => x_tool_specs(&allowed, &effects),
                _ => github_tool_specs(&allowed, &effects),
            };
            let credential = worker_connection
                .credential
                .as_ref()
                .map(|credential| credential.value.clone());
            connector_client = Some(match connector_slug.as_str() {
                "gitlab" => ConnectorClient::Gitlab(GitLabClientConfig::from_connection(
                    &conn.config,
                    credential,
                )),
                "kubernetes" => ConnectorClient::Kubernetes(
                    KubernetesClientConfig::from_connection(&conn.config, credential)
                        .map_err(anyhow::Error::msg)?,
                ),
                "x" => ConnectorClient::X(XClientConfig::from_connection(&conn.config, credential)),
                _ => ConnectorClient::Github(GitHubClientConfig::from_connection(
                    &conn.config,
                    credential,
                )),
            });
        }
        connection = Some(conn);
    }

    // A harness with native tools (bash, edit, write) always gets the policy endpoint, so
    // autonomy and approval mode apply to those tools even when the agent has no connector
    // tools and the organization no tool policies (#303). The mock harness has none.
    if tools.is_empty() && tool_policies.is_empty() && !has_native_tools(&execution.harness_kind) {
        return Ok(None);
    }

    let autonomy = AutonomyLevel::from_level(spec.autonomy.level).unwrap_or(AutonomyLevel::OBSERVE);
    let state = Arc::new(BrokerState {
        client,
        execution: execution.clone(),
        workspace: workspace.to_path_buf(),
        autonomy,
        approval_mode: spec.autonomy.approval_mode,
        connection,
        connector_slug,
        connector_client,
        tools,
        tool_policies: tool_policies.to_vec(),
        token: format!("{}{}", Uuid::now_v7().simple(), Uuid::now_v7().simple()),
        http: runtime.http,
        repo_specs: crate::git::repo_specs(&execution.input),
        pause: runtime.pause,
        cancel: runtime.cancel,
        rejected: Default::default(),
    });

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let app = Router::new()
        .route("/tools", get(list_tools))
        .route("/tools/call", post(call_tool))
        .route("/policy", post(policy_check))
        .with_state(state.clone());
    let task = tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, app).await {
            tracing::error!(%error, "tool broker server stopped");
        }
    });
    tracing::info!(
        execution_id = %state.execution.id,
        tools = state.tools.len(),
        "tool broker started on loopback"
    );
    Ok(Some(BrokerHandle {
        url: format!("http://{address}"),
        token: state.token.clone(),
        tool_count: state.tools.len(),
        task,
    }))
}

async fn list_tools(
    State(state): State<Arc<BrokerState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !authorized(&headers, &state.token) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    }
    let tools: Vec<Value> = state
        .tools
        .iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.description,
                "inputSchema": tool.input_schema,
            })
        })
        .collect();
    (StatusCode::OK, Json(json!({ "tools": tools }))).into_response()
}

#[derive(Debug, Deserialize)]
struct BrokerToolCall {
    name: String,
    #[serde(default)]
    arguments: Value,
}

async fn call_tool(
    State(state): State<Arc<BrokerState>>,
    headers: HeaderMap,
    Json(call): Json<BrokerToolCall>,
) -> impl IntoResponse {
    if !authorized(&headers, &state.token) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    }
    let Some(tool) = state.tools.iter().find(|tool| tool.name == call.name) else {
        record_denied(&state, &call.name, "unknown or not allowed tool").await;
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": format!("tool '{}' is not allowed", call.name) })),
        )
            .into_response();
    };

    let Some(connection) = state.connection.as_ref() else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "execution has no connector connection" })),
        )
            .into_response();
    };
    let grants: Vec<GrantedCapability> = state
        .tools
        .iter()
        .map(|tool| GrantedCapability {
            connection_id: connection.id,
            connector: state.connector_slug.clone(),
            capability: tool.capability.clone(),
            effect: tool.effect,
        })
        .collect();
    let platform = evaluate_capability(
        state.autonomy,
        state.approval_mode,
        &tool.capability,
        tool.effect,
        &grants,
    );
    let tool_call = ToolCall {
        tool: call.name.clone(),
        capability: Some(tool.capability.clone()),
        effect: Some(tool.effect),
        connection: Some(connection.name.clone()),
        arguments: call.arguments.clone(),
        cwd: None,
    };
    let policy = evaluate_tool_call(platform, platform, &state.tool_policies, &tool_call);
    let decision = policy.decision;

    let mut approval_id: Option<Uuid> = None;
    if decision == PolicyDecision::Deny {
        record_denied(
            &state,
            &tool.capability,
            policy
                .reason
                .as_deref()
                .unwrap_or("denied by autonomy/capability policy"),
        )
        .await;
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": format!("capability '{}' is not allowed by policy", tool.capability) })),
        )
            .into_response();
    }
    if decision == PolicyDecision::RequireApproval {
        match request_approval(&state, tool, &call.arguments, Some(&policy)).await {
            Ok(approval) => approval_id = Some(approval.id),
            Err(error) => {
                return tool_error(&state, tool, &call.arguments, &error.to_string()).await;
            }
        }
    }

    let result = if matches!(tool.capability.as_str(), "repo.commit" | "repo.push") {
        let repository = call.arguments.get("repository").and_then(Value::as_str);
        let (workspace, connection_id) = match state.repo_target(repository) {
            Ok(target) => target,
            Err(error) => {
                return tool_error(&state, tool, &call.arguments, &error).await;
            }
        };
        let outcome = if tool.capability == "repo.commit" {
            let message = call
                .arguments
                .get("message")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty());
            match message {
                Some(message) => {
                    crate::git::commit(
                        &state.client,
                        state.execution.id,
                        connection_id,
                        &workspace,
                        message,
                        &state.cancel,
                    )
                    .await
                }
                None => Err(anyhow::anyhow!("commit message is required")),
            }
        } else {
            let branch = call.arguments.get("branch").and_then(Value::as_str);
            match branch {
                Some(branch) => {
                    crate::git::push(
                        &state.client,
                        state.execution.id,
                        connection_id,
                        &workspace,
                        branch,
                        &state.cancel,
                    )
                    .await
                }
                None => Err(anyhow::anyhow!("branch is required")),
            }
        };
        outcome.map_err(|error| error.to_string())
    } else {
        match state.connector_client.as_ref() {
            Some(ConnectorClient::Github(config)) => {
                let args = normalize_github_args(&call.arguments);
                github::invoke(&state.http, config, &tool.capability, &args).await
            }
            Some(ConnectorClient::Gitlab(config)) => {
                gitlab::invoke(&state.http, config, &tool.capability, &call.arguments).await
            }
            Some(ConnectorClient::Kubernetes(config)) => {
                config.invoke(&tool.capability, &call.arguments).await
            }
            Some(ConnectorClient::X(config)) => {
                x::invoke(&state.http, config, &tool.capability, &call.arguments).await
            }
            None => Err("execution has no connector client".to_string()),
        }
    };

    match result {
        Ok(result) => {
            record_event(
                &state,
                "tool_call",
                LogLevel::Info,
                &format!(
                    "{} {}{}",
                    tool.capability,
                    if decision == PolicyDecision::RequireApproval {
                        "executed after approval"
                    } else {
                        "executed"
                    },
                    format_approval(approval_id)
                ),
                json!({
                    "capability": tool.capability,
                    "effect": tool.effect.as_str(),
                    "decision": decision.as_str(),
                    "approvalId": approval_id,
                    "arguments": truncate_json(&call.arguments, 4000),
                    "result": truncate_json(&result, 8000),
                }),
            )
            .await;
            record_audit(
                &state,
                "tool.call.executed",
                json!({
                    "capability": tool.capability,
                    "decision": decision.as_str(),
                    "approvalId": approval_id,
                }),
            )
            .await;
            mcp_text(result, false)
        }
        Err(message) => tool_error(&state, tool, &call.arguments, &message).await,
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PolicyCheck {
    tool: String,
    /// Caller-declared metadata. Never trusted for native tools: the effect is derived
    /// from the tool name ([`native_tool_effect`]); a disagreement is only logged.
    #[serde(default)]
    capability: Option<String>,
    #[serde(default)]
    effect: Option<CapabilityEffect>,
    #[serde(default)]
    connection: Option<String>,
    #[serde(default)]
    arguments: Value,
    /// Hash ([`arguments_hash`]) of the arguments the harness hook is about to execute.
    /// When present it must equal the hash of `arguments`; the response returns the
    /// hash the approval is bound to so the hook can re-verify at execution time.
    #[serde(default)]
    arguments_hash: Option<String>,
}

/// Effect of a native harness tool, derived from its name. The caller's declaration is
/// never used. Unknown tools are treated as `write` (fail closed).
///
/// | effect | tools |
/// |---|---|
/// | read | `read`, `grep`, `glob`, `list`, `ls`, `find` |
/// | external_side_effect | `bash`, `shell`, `webfetch`, `websearch` |
/// | write | `edit`, `write`, `patch`, `multiedit`, `apply_patch`, and anything unknown |
///
/// `task`/`subagent` only dispatch an internal subagent; the subagent's own tool calls
/// are checked by the same policy hook. Treating the dispatch as an external side effect
/// blocked native Agent Teams at Observe/Recommend before any work could start.
/// Harnesses that run tools of their own, which must go through `/policy`.
fn has_native_tools(harness_kind: &str) -> bool {
    !matches!(harness_kind, "mock")
}

/// The broker's own MCP tools reach a policy hook under the harness's MCP naming
/// (OpenCode: `orbit_<tool>`, Claude-style `mcp__orbit__<tool>`), or bare. They are
/// checked by the broker when they are called, so `/policy` lets them through instead of
/// treating an unknown name as a write that needs approval.
fn broker_tool<'a>(tools: &'a [ToolSpec], name: &str) -> Option<&'a ToolSpec> {
    let name = name.trim();
    let bare = name
        .strip_prefix("mcp__orbit__")
        .or_else(|| name.strip_prefix("orbit_"))
        .unwrap_or(name);
    tools.iter().find(|tool| tool.name == bare)
}

fn native_tool_effect(tool: &str) -> CapabilityEffect {
    match tool.trim().to_ascii_lowercase().as_str() {
        "read" | "grep" | "glob" | "list" | "ls" | "find" => CapabilityEffect::Read,
        // Harness-internal state with no workspace or external effect: OpenCode's todo
        // list, clarifying questions and skill loading, and Pi's codemode, which the
        // Pi read-only baseline already allows. Every tool now goes through this check
        // (#303), so these must not need an approval.
        "todowrite" | "todoread" | "question" | "skill" | "codemode" => CapabilityEffect::Read,
        "bash" | "shell" | "webfetch" | "websearch" => CapabilityEffect::ExternalSideEffect,
        "task" | "subagent" => CapabilityEffect::Read,
        _ => CapabilityEffect::Write,
    }
}

/// Stable hash of a native tool call (tool name plus canonicalised arguments: sorted
/// object keys, compact JSON). Approvals are bound to it.
fn arguments_hash(tool: &str, arguments: &Value) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(
        action_fingerprint(tool, arguments).as_bytes(),
    ))
}

/// A hook that presents a hash must present the hash of the arguments it sends.
fn verify_presented_hash(presented: Option<&str>, expected: &str) -> Result<(), &'static str> {
    match presented {
        Some(hash) if hash != expected => {
            Err("arguments hash mismatch: the arguments differ from those that were checked")
        }
        _ => Ok(()),
    }
}

/// Build the platform-derived call and evaluate it: autonomy and approval mode through
/// `evaluate_capability` (so level changes apply automatically), then tool-policy rules.
/// The platform decision is also the default when no rule matches, so an autonomous
/// (L4/Auto) agent can use its native tools without a blanket allow rule. Rules can only
/// tighten that decision (deny / require approval), never weaken it.
fn evaluate_native_call(
    autonomy: AutonomyLevel,
    approval_mode: ApprovalMode,
    rules: &[ToolPolicyRule],
    workspace: &std::path::Path,
    request: &PolicyCheck,
) -> (ToolCall, ToolPolicyOutcome) {
    let effect = native_tool_effect(&request.tool);
    let grants = [GrantedCapability {
        connection_id: Uuid::nil(),
        connector: "harness".into(),
        capability: request.tool.clone(),
        effect,
    }];
    let platform = evaluate_capability(autonomy, approval_mode, &request.tool, effect, &grants);
    let call = ToolCall {
        tool: request.tool.clone(),
        capability: None,
        effect: Some(effect),
        connection: None,
        arguments: request.arguments.clone(),
        cwd: Some(workspace.to_string_lossy().into_owned()),
    };
    let outcome = evaluate_tool_call(platform, platform, rules, &call);
    (call, outcome)
}

/// Policy decision for native harness tools (Claude Code `PreToolUse`, OpenCode hooks).
/// Effects are derived server-side. With no matching rule the platform autonomy decision
/// applies, so reads run at every level and writes/side effects follow the autonomy level
/// and approval mode. Rules can only make the decision stricter.
async fn policy_check(
    State(state): State<Arc<BrokerState>>,
    headers: HeaderMap,
    Json(request): Json<PolicyCheck>,
) -> impl IntoResponse {
    if !authorized(&headers, &state.token) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    }
    let args_hash = arguments_hash(&request.tool, &request.arguments);
    if let Err(reason) = verify_presented_hash(request.arguments_hash.as_deref(), &args_hash) {
        record_denied(&state, &request.tool, reason).await;
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "decision": "deny",
                "reason": reason,
                "argumentsHash": args_hash,
            })),
        )
            .into_response();
    }
    if let Some(tool) = broker_tool(&state.tools, &request.tool) {
        return (
            StatusCode::OK,
            Json(json!({
                "decision": "allow",
                "reason": format!("ORBIT tool {}: checked by the broker when it runs", tool.name),
                "argumentsHash": args_hash,
            })),
        )
            .into_response();
    }
    let (call, outcome) = evaluate_native_call(
        state.autonomy,
        state.approval_mode,
        &state.tool_policies,
        &state.workspace,
        &request,
    );
    let effect = call.effect.unwrap_or(CapabilityEffect::Write);
    if request.effect.is_some_and(|declared| declared != effect)
        || request.capability.is_some()
        || request.connection.is_some()
    {
        record_event(
            &state,
            "tool_call",
            LogLevel::Warn,
            &format!(
                "ignored caller-declared metadata for native tool {}",
                call.tool
            ),
            json!({
                "tool": call.tool,
                "declaredEffect": request.effect.map(|e| e.as_str()),
                "derivedEffect": effect.as_str(),
                "declaredCapability": request.capability,
                "declaredConnection": request.connection,
            }),
        )
        .await;
    }
    let respond = |status: StatusCode, outcome: &ToolPolicyOutcome| {
        let mut body = serde_json::to_value(outcome).unwrap_or_else(|_| json!({}));
        if let Some(map) = body.as_object_mut() {
            map.insert("argumentsHash".into(), json!(args_hash));
        }
        (status, Json(body)).into_response()
    };

    match outcome.decision {
        PolicyDecision::Deny => {
            record_denied(
                &state,
                &call.tool,
                outcome
                    .reason
                    .as_deref()
                    .unwrap_or("denied by autonomy or tool policy"),
            )
            .await;
            respond(StatusCode::FORBIDDEN, &outcome)
        }
        PolicyDecision::RequireApproval => {
            let action = json!({
                "capability": call.tool,
                "connector": "harness",
                "connectionId": state.connection.as_ref().map(|connection| connection.id),
                "connectionName": state.connection.as_ref().map(|connection| connection.name.clone()),
                "effect": effect.as_str(),
                "agentName": state.execution.agent_name,
                "arguments": truncate_json(&call.arguments, 4000),
                "argumentsHash": args_hash,
                "reason": outcome.reason,
                "impact": outcome.impact,
                "rule": outcome.rule_name,
                "source": "tool_policy",
            });
            match request_approval_action(&state, &call.tool, action, &call.arguments).await {
                Ok(_) => respond(StatusCode::OK, &outcome),
                Err(error) => (
                    StatusCode::FORBIDDEN,
                    Json(json!({ "decision": "deny", "reason": error.to_string() })),
                )
                    .into_response(),
            }
        }
        PolicyDecision::Allow => respond(StatusCode::OK, &outcome),
    }
}

async fn tool_error(
    state: &BrokerState,
    tool: &ToolSpec,
    arguments: &Value,
    message: &str,
) -> axum::response::Response {
    record_event(
        state,
        "tool_call",
        LogLevel::Error,
        &format!("{} failed: {}", tool.capability, truncate(message, 300)),
        json!({
            "capability": tool.capability,
            "error": message,
            "arguments": truncate_json(arguments, 2000),
        }),
    )
    .await;
    record_audit(
        state,
        "tool.call.failed",
        json!({ "capability": tool.capability, "error": truncate(message, 500) }),
    )
    .await;
    mcp_text(json!({ "error": message }), true)
}

async fn request_approval(
    state: &BrokerState,
    tool: &ToolSpec,
    arguments: &Value,
    policy: Option<&ToolPolicyOutcome>,
) -> anyhow::Result<Approval> {
    let connection_id = state.connection.as_ref().map(|connection| connection.id);
    let connection_name = state
        .connection
        .as_ref()
        .map(|connection| connection.name.clone());
    let action = json!({
        "capability": tool.capability,
        "connector": state.connector_slug,
        "connectionId": connection_id,
        "connectionName": connection_name,
        "effect": tool.effect.as_str(),
        "agentName": state.execution.agent_name,
        "arguments": truncate_json(arguments, 4000),
        "reason": policy.and_then(|policy| policy.reason.clone()),
        "impact": policy.and_then(|policy| policy.impact.clone()),
        "rule": policy.and_then(|policy| policy.rule_name.clone()),
    });
    request_approval_action(state, &tool.capability, action, arguments).await
}

async fn request_approval_action(
    state: &BrokerState,
    capability: &str,
    action: Value,
    arguments: &Value,
) -> anyhow::Result<Approval> {
    // A person already said no to exactly this in this run. Asking again would only
    // put the same question back in the queue, so refuse it here.
    let fingerprint = action_fingerprint(capability, arguments);
    if state
        .rejected
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(&fingerprint)
    {
        record_event(
            state,
            "tool_call",
            LogLevel::Warn,
            &format!("denied tool call {capability}: already rejected in this run"),
            json!({
                "capability": capability,
                "reason": "a person already rejected this exact action in this run",
                "decision": "deny",
                "arguments": truncate_json(arguments, 4000),
            }),
        )
        .await;
        anyhow::bail!(
            "capability '{capability}' requires approval; request rejected (a person already rejected this exact action in this run; do not retry it)"
        );
    }

    // Cumulative approval time is capped: once the budget is used up no further pause is
    // granted and the call is denied; further waiting then counts toward the run timeout.
    let Some(wait_seconds) = approval_wait_seconds(state.pause.remaining_budget()) else {
        record_event(
            state,
            "tool_call",
            LogLevel::Warn,
            &format!("denied tool call {capability}: approval wait budget exhausted"),
            json!({
                "capability": capability,
                "reason": "the run used up its approval wait budget",
                "decision": "deny",
                "arguments": truncate_json(arguments, 4000),
            }),
        )
        .await;
        anyhow::bail!(
            "capability '{capability}' requires approval; denied because this run has used up its approval wait budget and will time out"
        );
    };
    let approval = state
        .client
        .create_approval(state.execution.id, action, wait_seconds)
        .await?;
    record_event(
        state,
        "tool_call",
        LogLevel::Warn,
        &format!(
            "approval requested for {capability} (approval {})",
            approval.id
        ),
        json!({
            "capability": capability,
            "approvalId": approval.id,
            "status": "pending",
            "arguments": truncate_json(arguments, 4000),
        }),
    )
    .await;
    record_audit(
        state,
        "tool.call.approval_requested",
        json!({ "capability": capability, "approvalId": approval.id }),
    )
    .await;

    // The run's clock stops while a person decides: their response time must not
    // turn into an execution timeout.
    let decided = {
        let _paused = state.pause.pause();
        state
            .client
            .await_approval(state.execution.id, approval.id, wait_seconds)
            .await?
    };
    if is_remembered_denial(&decided.status) {
        state
            .rejected
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(fingerprint);
    }
    record_event(
        state,
        "tool_call",
        if decided.status == "approved" {
            LogLevel::Info
        } else {
            LogLevel::Warn
        },
        &format!("approval {} for {capability}", decided.status),
        json!({ "approvalId": decided.id, "status": decided.status }),
    )
    .await;
    if decided.status != "approved" {
        anyhow::bail!(
            "capability '{capability}' requires approval; request {}",
            decided.status
        );
    }
    record_audit(
        state,
        "tool.call.approved",
        json!({ "capability": capability, "approvalId": decided.id }),
    )
    .await;
    Ok(decided)
}

/// How long an approval request stays open. It no longer has to fit inside the
/// execution timeout, because the run's clock is paused while it waits.
const APPROVAL_WAIT_SECONDS: u64 = 900;

/// Seconds to wait for one approval: the per-request window, capped by the remaining
/// approval budget. `None` when the budget is used up (a budgetless clock never is).
fn approval_wait_seconds(remaining_budget: Option<Duration>) -> Option<u64> {
    match remaining_budget {
        None => Some(APPROVAL_WAIT_SECONDS),
        Some(remaining) => match remaining.as_secs() {
            0 => None,
            seconds => Some(seconds.min(APPROVAL_WAIT_SECONDS)),
        },
    }
}

/// Decisions after which the same action must not be asked again in this run: an
/// unanswered (expired) request counts like a rejection, so the model cannot re-request
/// it endlessly and hold the run open.
fn is_remembered_denial(status: &str) -> bool {
    matches!(status, "rejected" | "expired")
}

/// Identity of an action for "already rejected" checks: the capability plus its
/// arguments, with object keys in a stable order.
fn action_fingerprint(capability: &str, arguments: &Value) -> String {
    fn canonical(value: &Value) -> Value {
        match value {
            Value::Object(map) => {
                let mut sorted: Vec<_> = map.iter().collect();
                sorted.sort_by(|a, b| a.0.cmp(b.0));
                Value::Object(
                    sorted
                        .into_iter()
                        .map(|(k, v)| (k.clone(), canonical(v)))
                        .collect(),
                )
            }
            Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
            other => other.clone(),
        }
    }
    format!("{capability}\u{1f}{}", canonical(arguments))
}

async fn record_denied(state: &BrokerState, capability: &str, reason: &str) {
    record_event(
        state,
        "tool_call",
        LogLevel::Warn,
        &format!("denied tool call {capability}: {reason}"),
        json!({ "capability": capability, "reason": reason, "decision": "deny" }),
    )
    .await;
    record_audit(
        state,
        "tool.call.denied",
        json!({ "capability": capability, "reason": reason }),
    )
    .await;
}

async fn record_event(
    state: &BrokerState,
    kind: &str,
    level: LogLevel,
    message: &str,
    data: Value,
) {
    let _ = state
        .client
        .event(
            state.execution.id,
            &crate::client::EventRequest {
                kind,
                level: level.as_str(),
                message,
                data,
            },
        )
        .await;
}

async fn record_audit(state: &BrokerState, action: &str, metadata: Value) {
    let connection_id = state.connection.as_ref().map(|connection| connection.id);
    if let Err(error) = state
        .client
        .audit(state.execution.id, action, connection_id, metadata)
        .await
    {
        tracing::warn!(%error, "failed to write tool audit log");
    }
}

fn normalize_github_args(arguments: &Value) -> Value {
    let mut args = arguments.clone();
    if args.get("owner").and_then(Value::as_str).is_none()
        && args.get("repo").and_then(Value::as_str).is_none()
    {
        if let Some(repository) = args.get("repository").and_then(Value::as_str) {
            if let Some((owner, repo)) = repository.split_once('/') {
                let (owner, repo) = (owner.to_string(), repo.to_string());
                args["owner"] = json!(owner);
                args["repo"] = json!(repo);
            }
        }
    }
    args
}

fn authorized(headers: &HeaderMap, token: &str) -> bool {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(|value| value == token)
        .unwrap_or(false)
}

fn mcp_text(value: Value, is_error: bool) -> axum::response::Response {
    let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".into());
    (
        StatusCode::OK,
        Json(json!({
            "content": [{ "type": "text", "text": text }],
            "isError": is_error,
        })),
    )
        .into_response()
}

fn format_approval(approval_id: Option<Uuid>) -> String {
    approval_id
        .map(|id| format!(" (approval {id})"))
        .unwrap_or_default()
}

fn truncate(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        value.to_string()
    } else {
        let mut end = limit;
        while end > 0 && !value.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &value[..end])
    }
}

fn truncate_json(value: &Value, limit: usize) -> Value {
    match serde_json::to_string(value) {
        Ok(serialized) if serialized.len() <= limit => value.clone(),
        Ok(serialized) => json!(truncate(&serialized, limit)),
        Err(_) => Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn connector_http_client_refuses_loopback_and_metadata_targets() {
        let client = orbit_security::guarded_client_with_policy(
            Duration::from_secs(5),
            orbit_security::OutboundPolicy::default(),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        for url in [
            format!("http://localhost:{port}/"),
            "http://metadata.invalid/".to_string(),
        ] {
            assert!(client.get(&url).send().await.is_err(), "{url}");
        }
    }

    #[test]
    fn approval_wait_is_capped_by_the_remaining_budget() {
        // No budget: the per-request window applies.
        assert_eq!(approval_wait_seconds(None), Some(APPROVAL_WAIT_SECONDS));
        // Plenty left: still the per-request window.
        assert_eq!(
            approval_wait_seconds(Some(Duration::from_secs(3600))),
            Some(APPROVAL_WAIT_SECONDS)
        );
        // Less than a window left: capped.
        assert_eq!(
            approval_wait_seconds(Some(Duration::from_secs(120))),
            Some(120)
        );
    }

    #[test]
    fn exhausted_budget_denies_further_pauses() {
        assert_eq!(approval_wait_seconds(Some(Duration::ZERO)), None);
        assert_eq!(
            approval_wait_seconds(Some(Duration::from_millis(900))),
            None
        );
        let clock = orbit_contracts::PauseClock::with_budget(Duration::from_millis(50));
        {
            let _paused = clock.pause();
            std::thread::sleep(Duration::from_millis(80));
        }
        assert_eq!(approval_wait_seconds(clock.remaining_budget()), None);
    }

    #[test]
    fn expired_approvals_are_remembered_like_rejected_ones() {
        assert!(is_remembered_denial("rejected"));
        assert!(is_remembered_denial("expired"));
        assert!(!is_remembered_denial("approved"));
        assert!(!is_remembered_denial("pending"));
    }

    #[test]
    fn tool_names_are_stable_and_mcp_friendly() {
        let effects = std::collections::BTreeMap::from([
            ("issues.list".to_string(), CapabilityEffect::Read),
            ("issues.comment".to_string(), CapabilityEffect::Write),
        ]);
        let tools = github_tool_specs(
            &["issues.list".to_string(), "issues.comment".to_string()],
            &effects,
        );
        assert_eq!(tools[0].name, "github_issues_list");
        assert_eq!(tools[1].name, "github_issues_comment");
        assert_eq!(tools[1].effect, CapabilityEffect::Write);
    }

    #[test]
    fn rejected_actions_are_recognised_whatever_the_key_order() {
        let first = action_fingerprint(
            "issues.comment",
            &json!({ "owner": "acme", "repo": "site", "number": 58, "body": "hi" }),
        );
        let reordered = action_fingerprint(
            "issues.comment",
            &json!({ "body": "hi", "number": 58, "repo": "site", "owner": "acme" }),
        );
        assert_eq!(first, reordered);
        // A different issue, text or capability is a different decision.
        assert_ne!(
            first,
            action_fingerprint(
                "issues.comment",
                &json!({ "owner": "acme", "repo": "site", "number": 59, "body": "hi" }),
            )
        );
        assert_ne!(
            first,
            action_fingerprint(
                "issues.update",
                &json!({ "owner": "acme", "repo": "site", "number": 58, "body": "hi" }),
            )
        );
    }

    fn request(tool: &str, effect: Option<CapabilityEffect>, arguments: Value) -> PolicyCheck {
        PolicyCheck {
            tool: tool.into(),
            capability: None,
            effect,
            connection: None,
            arguments,
            arguments_hash: None,
        }
    }

    fn allow_all() -> Vec<ToolPolicyRule> {
        vec![ToolPolicyRule {
            id: Uuid::now_v7(),
            name: "allow everything".into(),
            priority: 100,
            enabled: true,
            action: orbit_domain::ToolPolicyAction::Allow,
            matcher: Default::default(),
            reason: None,
            impact: None,
        }]
    }

    fn decide(
        level: u8,
        mode: ApprovalMode,
        rules: &[ToolPolicyRule],
        req: &PolicyCheck,
    ) -> PolicyDecision {
        let autonomy = AutonomyLevel::from_level(level).unwrap();
        evaluate_native_call(autonomy, mode, rules, std::path::Path::new("/work"), req)
            .1
            .decision
    }

    #[test]
    fn declared_read_effect_on_bash_is_ignored() {
        let req = request(
            "bash",
            Some(CapabilityEffect::Read),
            json!({ "command": "rm -rf /" }),
        );
        let (call, _) = evaluate_native_call(
            AutonomyLevel::from_level(4).unwrap(),
            ApprovalMode::Always,
            &allow_all(),
            std::path::Path::new("/work"),
            &req,
        );
        assert_eq!(call.effect, Some(CapabilityEffect::ExternalSideEffect));
        // `approval_mode=always` still requires approval despite the allow rule.
        assert_eq!(
            decide(4, ApprovalMode::Always, &allow_all(), &req),
            PolicyDecision::RequireApproval
        );
    }

    #[test]
    fn effect_matcher_cannot_be_spoofed_by_the_caller() {
        let rules = vec![ToolPolicyRule {
            matcher: orbit_domain::ToolPolicyMatcher {
                effect: Some(CapabilityEffect::Write),
                ..Default::default()
            },
            action: orbit_domain::ToolPolicyAction::Deny,
            name: "no writes".into(),
            ..allow_all().remove(0)
        }];
        let spoofed = request(
            "write",
            Some(CapabilityEffect::Read),
            json!({ "path": "a" }),
        );
        assert_eq!(
            decide(4, ApprovalMode::Auto, &rules, &spoofed),
            PolicyDecision::Deny
        );
    }

    #[test]
    fn subagent_dispatch_is_internal_not_an_external_side_effect() {
        assert_eq!(native_tool_effect("task"), CapabilityEffect::Read);
        assert_eq!(native_tool_effect("subagent"), CapabilityEffect::Read);
        // The subagent itself still cannot write at low autonomy: its own tool calls are
        // evaluated separately by the same policy hook.
        let req = request("task", None, json!({ "description": "delegate" }));
        assert_eq!(
            decide(2, ApprovalMode::Auto, &[], &req),
            PolicyDecision::Allow
        );
    }

    #[test]
    fn unknown_tools_are_treated_as_write() {
        assert_eq!(native_tool_effect("mystery_tool"), CapabilityEffect::Write);
        assert_eq!(native_tool_effect("Read"), CapabilityEffect::Read);
        assert_eq!(native_tool_effect("GREP"), CapabilityEffect::Read);
        assert_eq!(native_tool_effect("edit"), CapabilityEffect::Write);
        assert_eq!(
            native_tool_effect("Bash"),
            CapabilityEffect::ExternalSideEffect
        );
        let req = request("mystery_tool", Some(CapabilityEffect::Read), json!({}));
        // Level 3 needs approval for writes even with a blanket allow rule.
        assert_eq!(
            decide(3, ApprovalMode::Auto, &allow_all(), &req),
            PolicyDecision::RequireApproval
        );
        assert_eq!(
            decide(4, ApprovalMode::Auto, &allow_all(), &req),
            PolicyDecision::Allow
        );
    }

    #[test]
    fn autonomy_and_approval_mode_apply_to_native_tools() {
        let read = request("read", None, json!({ "path": "a" }));
        let bash = request("bash", None, json!({ "command": "ls" }));
        // Reads are allowed by an allow rule at every level.
        for level in 1..=4 {
            assert_eq!(
                decide(level, ApprovalMode::Auto, &allow_all(), &read),
                PolicyDecision::Allow
            );
        }
        // An allow rule never lifts the platform floor for writes below level 4.
        for level in 1..=3 {
            assert_ne!(
                decide(level, ApprovalMode::Auto, &allow_all(), &bash),
                PolicyDecision::Allow,
                "level {level}"
            );
        }
        assert_eq!(
            decide(4, ApprovalMode::Auto, &allow_all(), &bash),
            PolicyDecision::Allow
        );
        assert_eq!(
            decide(4, ApprovalMode::Writes, &allow_all(), &bash),
            PolicyDecision::RequireApproval
        );
        // Without rules, the platform autonomy decision applies.
        assert_eq!(
            decide(4, ApprovalMode::Auto, &[], &read),
            PolicyDecision::Allow
        );
        assert_eq!(
            decide(4, ApprovalMode::Auto, &[], &bash),
            PolicyDecision::Allow
        );
        assert_eq!(
            decide(3, ApprovalMode::Auto, &[], &bash),
            PolicyDecision::RequireApproval
        );
        assert_eq!(
            decide(1, ApprovalMode::Auto, &[], &bash),
            PolicyDecision::Deny
        );
    }

    #[test]
    fn presented_hash_mismatch_is_rejected() {
        let hash = arguments_hash("bash", &json!({ "command": "ls" }));
        assert!(verify_presented_hash(Some(&hash), &hash).is_ok());
        assert!(verify_presented_hash(None, &hash).is_ok());
        let other = arguments_hash("bash", &json!({ "command": "rm -rf /" }));
        assert!(verify_presented_hash(Some(&other), &hash).is_err());
    }

    #[test]
    fn approval_hash_binds_the_arguments() {
        let a = arguments_hash("bash", &json!({ "command": "ls", "cwd": "/w" }));
        let reordered = arguments_hash("bash", &json!({ "cwd": "/w", "command": "ls" }));
        assert_eq!(a, reordered);
        assert_ne!(
            a,
            arguments_hash("bash", &json!({ "command": "rm -rf /", "cwd": "/w" }))
        );
        assert_ne!(
            a,
            arguments_hash("edit", &json!({ "command": "ls", "cwd": "/w" }))
        );
        assert_eq!(a.len(), 64);
    }

    // --- X connector: mock control plane + mock X API -------------------------------

    const X_TOKEN: &str = "user-context-token-do-not-log";

    #[derive(Default)]
    struct MockControlPlane {
        connection: Value,
        events: std::sync::Mutex<Vec<Value>>,
        audits: std::sync::Mutex<Vec<Value>>,
    }

    async fn control_plane_connection(State(api): State<Arc<MockControlPlane>>) -> Json<Value> {
        Json(api.connection.clone())
    }

    async fn control_plane_event(
        State(api): State<Arc<MockControlPlane>>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        api.events.lock().unwrap().push(body);
        Json(json!({}))
    }

    async fn control_plane_audit(
        State(api): State<Arc<MockControlPlane>>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        api.audits.lock().unwrap().push(body);
        Json(json!({}))
    }

    async fn spawn_control_plane(connection: Value) -> (WorkerClient, Arc<MockControlPlane>) {
        let api = Arc::new(MockControlPlane {
            connection,
            ..Default::default()
        });
        let app = Router::new()
            .route(
                "/api/v1/worker/connections/{id}",
                get(control_plane_connection),
            )
            .route(
                "/api/v1/worker/executions/{id}/events",
                post(control_plane_event),
            )
            .route("/api/v1/worker/audit", post(control_plane_audit))
            .with_state(api.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (WorkerClient::new(url, "worker-token").unwrap(), api)
    }

    fn x_connection(base_url: &str) -> (Uuid, Value) {
        let connection_id = Uuid::now_v7();
        let connector_id = Uuid::now_v7();
        let now = chrono::Utc::now();
        let value = json!({
            "connection": {
                "id": connection_id,
                "orgId": Uuid::now_v7(),
                "ownerUserId": null,
                "ownerTeamId": null,
                "visibility": "private",
                "connectorId": connector_id,
                "name": "X test connection",
                "status": "active",
                "credentialRef": null,
                "config": { "baseUrl": base_url },
                "scopes": ["tweet.write"],
                "createdBy": null,
                "lastCheckedAt": null,
                "createdAt": now,
                "updatedAt": now
            },
            "connector": {
                "id": connector_id,
                "orgId": null,
                "slug": "x",
                "name": "X",
                "version": "0.2.0",
                "description": "test",
                "auth": ["oauth2"],
                "capabilities": [
                    { "id": "posts.search", "effect": "read" },
                    { "id": "posts.create", "effect": "external_side_effect" }
                ],
                "configSchema": {},
                "webhooks": false,
                "builtin": true,
                "docsUrl": null,
                "createdAt": now
            },
            "credential": { "secretRef": "secret-ref", "value": X_TOKEN }
        });
        (connection_id, value)
    }

    #[derive(Clone)]
    struct RecordedXRequest {
        path: String,
        authorization: String,
        body: Value,
    }

    #[derive(Default)]
    struct MockXApi {
        status: Option<StatusCode>,
        requests: std::sync::Mutex<Vec<RecordedXRequest>>,
    }

    async fn mock_tweets(
        State(api): State<Arc<MockXApi>>,
        uri: axum::http::Uri,
        headers: axum::http::HeaderMap,
        Json(body): Json<Value>,
    ) -> axum::response::Response {
        api.requests.lock().unwrap().push(RecordedXRequest {
            path: uri.path().to_string(),
            authorization: headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string(),
            body,
        });
        match api.status {
            Some(status) => (
                status,
                Json(json!({ "title": "Forbidden", "detail": "app-only token cannot post" })),
            )
                .into_response(),
            None => (
                StatusCode::OK,
                Json(json!({
                    "data": { "id": "123456", "text": "posted" }
                })),
            )
                .into_response(),
        }
    }

    async fn spawn_x_api(status: Option<StatusCode>) -> (String, Arc<MockXApi>) {
        let api = Arc::new(MockXApi {
            status,
            ..Default::default()
        });
        let app = Router::new()
            .route("/tweets", post(mock_tweets))
            .with_state(api.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (url, api)
    }

    fn broker_execution() -> ExecutionRecord {
        let now = chrono::Utc::now();
        ExecutionRecord {
            id: Uuid::now_v7(),
            org_id: Uuid::now_v7(),
            agent_id: Uuid::now_v7(),
            agent_version_id: Uuid::now_v7(),
            agent_name: "X agent".into(),
            harness_kind: "mock".into(),
            trigger_id: None,
            trigger_source: "manual".into(),
            placement: "local".into(),
            status: "running".into(),
            session_ref: None,
            input: json!({}),
            skills: vec![],
            output: None,
            error: None,
            usage: json!({}),
            timeout_seconds: 300,
            idempotency_key: None,
            requested_by: None,
            worker_id: None,
            team_run_id: None,
            team_step_index: None,
            created_at: now,
            started_at: Some(now),
            running_started_at: Some(now),
            finished_at: None,
            updated_at: now,
            archived_at: None,
        }
    }

    async fn start_x_broker(
        status: Option<StatusCode>,
    ) -> (
        BrokerHandle,
        Arc<MockControlPlane>,
        Arc<MockXApi>,
        tempfile::TempDir,
    ) {
        let (x_url, x_api) = spawn_x_api(status).await;
        let (connection_id, connection) = x_connection(&x_url);
        let (client, control_plane) = spawn_control_plane(connection).await;
        let spec: AgentSpec = serde_json::from_value(json!({
            "instructions": { "system": "test" },
            "connectors": [{
                "connector": "x",
                "connectionId": connection_id,
                "capabilities": ["posts.search", "posts.create"]
            }],
            "autonomy": { "level": 4, "approvalMode": "auto" }
        }))
        .unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let http = orbit_security::guarded_client_with_policy(
            Duration::from_secs(5),
            orbit_security::OutboundPolicy::new(true, "127.0.0.1"),
        );
        let handle = start_with_http(
            client,
            &spec,
            &broker_execution(),
            workspace.path(),
            &[],
            BrokerRuntime {
                http,
                pause: orbit_contracts::PauseClock::with_budget(Duration::from_secs(60)),
                cancel: tokio_util::sync::CancellationToken::new(),
            },
        )
        .await
        .unwrap()
        .expect("X binding exposes posts.create");
        (handle, control_plane, x_api, workspace)
    }

    #[test]
    fn harness_internal_tools_are_reads() {
        for tool in ["todowrite", "todoread", "question", "skill", "codemode"] {
            assert_eq!(native_tool_effect(tool), CapabilityEffect::Read, "{tool}");
        }
        assert_eq!(native_tool_effect("apply_patch"), CapabilityEffect::Write);
    }

    async fn policy_status(handle: &BrokerHandle, tool: &str, arguments: Value) -> u16 {
        reqwest::Client::new()
            .post(format!("{}/policy", handle.url))
            .bearer_auth(&handle.token)
            .json(&json!({ "tool": tool, "arguments": arguments }))
            .send()
            .await
            .unwrap()
            .status()
            .as_u16()
    }

    /// #303: a harness with native tools gets the policy endpoint even without connector
    /// tools or tool policy rules, so autonomy applies to bash, edit and write.
    #[tokio::test]
    async fn native_tool_harnesses_always_get_the_policy_endpoint() {
        let (client, _control_plane) = spawn_control_plane(json!({})).await;
        let spec: AgentSpec = serde_json::from_value(json!({
            "instructions": { "system": "test" },
            "autonomy": { "level": 1 }
        }))
        .unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let runtime = || BrokerRuntime {
            http: reqwest::Client::new(),
            pause: orbit_contracts::PauseClock::with_budget(Duration::from_secs(5)),
            cancel: tokio_util::sync::CancellationToken::new(),
        };
        let mut execution = broker_execution();
        execution.harness_kind = "opencode".into();
        let handle = start_with_http(
            client.clone(),
            &spec,
            &execution,
            workspace.path(),
            &[],
            runtime(),
        )
        .await
        .unwrap()
        .expect("an OpenCode run always gets the policy endpoint");
        assert_eq!(
            policy_status(&handle, "read", json!({ "path": "a" })).await,
            200
        );
        assert_eq!(
            policy_status(&handle, "bash", json!({ "command": "rm -rf x" })).await,
            403
        );
        // A tool the policy does not know by name is a write: it fails closed.
        assert_eq!(policy_status(&handle, "multiedit", json!({})).await, 403);

        // The mock harness has no native tools and still runs without a broker.
        let mock = broker_execution();
        assert!(
            start_with_http(client, &spec, &mock, workspace.path(), &[], runtime())
                .await
                .unwrap()
                .is_none()
        );
    }

    /// #303: ORBIT's own MCP tools pass the native policy hook under their MCP names; the
    /// broker checks them itself when they are called.
    #[tokio::test]
    async fn broker_tools_pass_the_native_policy_hook() {
        let (x_url, _x_api) = spawn_x_api(None).await;
        let (connection_id, connection) = x_connection(&x_url);
        let (client, _control_plane) = spawn_control_plane(connection).await;
        let spec: AgentSpec = serde_json::from_value(json!({
            "instructions": { "system": "test" },
            "connectors": [{
                "connector": "x",
                "connectionId": connection_id,
                "capabilities": ["posts.create"]
            }],
            "autonomy": { "level": 1 }
        }))
        .unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let mut execution = broker_execution();
        execution.harness_kind = "opencode".into();
        let handle = start_with_http(
            client,
            &spec,
            &execution,
            workspace.path(),
            &[],
            BrokerRuntime {
                http: reqwest::Client::new(),
                pause: orbit_contracts::PauseClock::with_budget(Duration::from_secs(5)),
                cancel: tokio_util::sync::CancellationToken::new(),
            },
        )
        .await
        .unwrap()
        .expect("broker");
        for name in [
            "orbit_x_posts_create",
            "mcp__orbit__x_posts_create",
            "x_posts_create",
        ] {
            assert_eq!(
                policy_status(&handle, name, json!({ "text": "hi" })).await,
                200,
                "{name}"
            );
        }
        // A look-alike name is not a broker tool.
        assert_eq!(
            policy_status(&handle, "orbit_x_posts_create_now", json!({})).await,
            403
        );
    }

    async fn broker_call(handle: &BrokerHandle, name: &str, arguments: Value) -> Value {
        let response = reqwest::Client::new()
            .post(format!("{}/tools/call", handle.url))
            .bearer_auth(&handle.token)
            .json(&json!({ "name": name, "arguments": arguments }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 200);
        response.json().await.unwrap()
    }

    fn result_payload(response: &Value) -> Value {
        serde_json::from_str(response["content"][0]["text"].as_str().unwrap()).unwrap()
    }

    fn workspace_contents(path: &std::path::Path) -> String {
        let mut combined = String::new();
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() {
                combined.push_str(&std::fs::read_to_string(path).unwrap_or_default());
            }
        }
        combined
    }

    fn assert_no_credential(control_plane: &MockControlPlane, workspace: &tempfile::TempDir) {
        let events = control_plane.events.lock().unwrap();
        let audits = control_plane.audits.lock().unwrap();
        for payload in events.iter().chain(audits.iter()) {
            assert!(
                !payload.to_string().contains(X_TOKEN),
                "credential appeared in an event or audit: {payload}"
            );
        }
        assert!(!workspace_contents(workspace.path()).contains(X_TOKEN));
    }

    #[tokio::test]
    async fn x_posts_create_uses_the_x_client_with_user_context_auth() {
        let (handle, control_plane, x_api, workspace) = start_x_broker(None).await;

        let tools: Value = reqwest::Client::new()
            .get(format!("{}/tools", handle.url))
            .bearer_auth(&handle.token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let names: Vec<&str> = tools["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        // posts.search stays pre-execution hydration; it is never an agent tool.
        assert_eq!(names, vec!["x_posts_create"]);

        let response = broker_call(
            &handle,
            "x_posts_create",
            json!({ "text": "hello from ORBIT" }),
        )
        .await;
        assert_eq!(response["isError"], false);
        assert_eq!(result_payload(&response)["id"], "123456");

        let requests = x_api.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, "/tweets");
        assert_eq!(requests[0].authorization, format!("Bearer {X_TOKEN}"));
        assert_eq!(requests[0].body, json!({ "text": "hello from ORBIT" }));
        drop(requests);
        assert_no_credential(&control_plane, &workspace);
    }

    #[tokio::test]
    async fn x_posts_create_reply_sends_in_reply_to_tweet_id() {
        let (handle, control_plane, x_api, workspace) = start_x_broker(None).await;

        for arguments in [
            json!({ "text": "reply one", "replyTo": "1234567890" }),
            json!({
                "text": "reply two",
                "reply": { "in_reply_to_tweet_id": "9876543210" }
            }),
        ] {
            let response = broker_call(&handle, "x_posts_create", arguments).await;
            assert_eq!(response["isError"], false);
        }

        let requests = x_api.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[0].body,
            json!({
                "text": "reply one",
                "reply": { "in_reply_to_tweet_id": "1234567890" }
            })
        );
        assert_eq!(
            requests[1].body,
            json!({
                "text": "reply two",
                "reply": { "in_reply_to_tweet_id": "9876543210" }
            })
        );
        drop(requests);
        assert_no_credential(&control_plane, &workspace);
    }

    #[tokio::test]
    async fn x_403_is_an_actionable_error_without_leaking_the_credential() {
        let (handle, control_plane, x_api, workspace) =
            start_x_broker(Some(StatusCode::FORBIDDEN)).await;

        let response = broker_call(&handle, "x_posts_create", json!({ "text": "nope" })).await;
        assert_eq!(response["isError"], true);
        let error = result_payload(&response)["error"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(error.contains("403"), "{error}");
        assert!(error.contains("user-context"), "{error}");
        assert!(error.contains("tweet.write"), "{error}");
        assert!(error.contains("app-only"), "{error}");
        assert!(!error.contains(X_TOKEN), "{error}");
        assert_eq!(x_api.requests.lock().unwrap().len(), 1);
        assert_no_credential(&control_plane, &workspace);
    }
}
