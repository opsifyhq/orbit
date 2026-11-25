//! HTTP client for the worker-facing control-plane API.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use orbit_contracts::{ModelEndpoint, ModelSelection, ResolvedModel, SecretLease};
use orbit_domain::{AgentSpec, GrantedCapability};
use orbit_store::{Approval, Connection, ConnectorRecord, ExecutionRecord, Skill};

#[derive(Clone)]
pub struct WorkerClient {
    base_url: String,
    token: String,
    http: reqwest::Client,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerSession {
    pub profile_name: Option<String>,
    pub harnesses: Vec<String>,
    pub target: Option<String>,
    pub execution_driver: Option<String>,
    pub agent_ids: Vec<Uuid>,
    pub skill_slugs: Vec<String>,
    #[serde(default)]
    pub allow_web_search: bool,
    #[serde(default)]
    pub allow_web_fetch: bool,
    #[serde(default)]
    pub native_tool_permissions: serde_json::Value,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerSecret {
    pub secret_ref: String,
    pub value: String,
}

/// `Debug` never prints the plaintext value (AGENTS.md rule 4, #451): a future
/// `tracing::debug!(?context)` or error dump must not leak a credential.
impl std::fmt::Debug for WorkerSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerSecret")
            .field("secret_ref", &self.secret_ref)
            .field("value", &"[redacted]")
            .finish()
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerModel {
    pub selection: ModelSelection,
    #[serde(default)]
    pub endpoint: Option<ModelEndpoint>,
    #[serde(default)]
    pub credential: Option<WorkerSecret>,
}

impl WorkerModel {
    pub fn into_resolved(self) -> ResolvedModel {
        ResolvedModel {
            selection: self.selection,
            endpoint: self.endpoint,
            credential: self
                .credential
                .map(|secret| SecretLease::new(secret.secret_ref, secret.value)),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerExecutionContext {
    pub execution: ExecutionRecord,
    pub spec: AgentSpec,
    pub model: Option<WorkerModel>,
    pub grants: Vec<GrantedCapability>,
    pub skills: Vec<Skill>,
    #[serde(default)]
    pub tool_policies: Vec<orbit_domain::ToolPolicyRule>,
    #[serde(default)]
    pub env: Vec<WorkerEnv>,
    #[serde(default)]
    pub subagent_models: Vec<WorkerSubagentModel>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerSubagentModel {
    pub agent_id: Uuid,
    pub model: Option<WorkerModel>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerEnv {
    pub env_var: String,
    pub value: String,
}

/// `Debug` never prints the plaintext value (AGENTS.md rule 4, #451): a future
/// `tracing::debug!(?context)` or error dump must not leak a credential.
impl std::fmt::Debug for WorkerEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerEnv")
            .field("env_var", &self.env_var)
            .field("value", &"[redacted]")
            .finish()
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaimResponse {
    execution: Option<WorkerExecutionContext>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerConnection {
    pub connection: Connection,
    pub connector: ConnectorRecord,
    pub credential: Option<WorkerSecret>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeartbeatRequest<'a> {
    pub worker_id: &'a str,
    pub harnesses: &'a [String],
    pub target: &'a str,
    pub hostname: Option<&'a str>,
    pub boot_id: Uuid,
    #[serde(rename = "startedAt")]
    pub boot_started_at: chrono::DateTime<chrono::Utc>,
    pub active_execution_ids: &'a [Uuid],
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimRequest<'a> {
    pub worker_id: &'a str,
    pub harnesses: &'a [String],
    pub target: &'a str,
    pub agent_ids: &'a [Uuid],
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventRequest<'a> {
    pub kind: &'a str,
    pub level: &'a str,
    pub message: &'a str,
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompleteRequest<'a> {
    pub status: &'a str,
    pub output: Option<serde_json::Value>,
    pub error: Option<serde_json::Value>,
    pub usage: Option<serde_json::Value>,
    pub session_ref: Option<&'a str>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CancelResponse {
    cancel_requested: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateApprovalRequest {
    execution_id: Uuid,
    action: serde_json::Value,
    wait_seconds: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuditRequest<'a> {
    execution_id: Uuid,
    action: &'a str,
    connection_id: Option<Uuid>,
    metadata: serde_json::Value,
}

impl WorkerClient {
    pub fn new(base_url: impl Into<String>, token: impl Into<String>) -> anyhow::Result<Self> {
        let http = orbit_security::client_with_custom_ca(Duration::from_secs(30));
        Ok(Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            token: token.into(),
            http,
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    async fn error(response: reqwest::Response) -> anyhow::Error {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        let message = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|value| {
                value
                    .get("error")
                    .and_then(|error| error.get("message"))
                    .and_then(|message| message.as_str())
                    .map(str::to_string)
            })
            .unwrap_or(body);
        anyhow::Error::new(crate::retry::ApiStatusError {
            status: status.as_u16(),
            message,
        })
    }

    async fn get_json<T: for<'de> Deserialize<'de>>(&self, path: &str) -> anyhow::Result<T> {
        let response = self
            .http
            .get(self.url(path))
            .bearer_auth(&self.token)
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(Self::error(response).await);
        }
        Ok(response.json().await?)
    }

    async fn post_json<B: Serialize, T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        body: &B,
    ) -> anyhow::Result<T> {
        let response = self
            .http
            .post(self.url(path))
            .bearer_auth(&self.token)
            .json(body)
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(Self::error(response).await);
        }
        Ok(response.json().await?)
    }

    async fn post_empty<B: Serialize>(&self, path: &str, body: &B) -> anyhow::Result<()> {
        let response = self
            .http
            .post(self.url(path))
            .bearer_auth(&self.token)
            .json(body)
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(Self::error(response).await);
        }
        Ok(())
    }

    pub async fn session(&self) -> anyhow::Result<WorkerSession> {
        self.get_json("/api/v1/worker/session").await
    }

    pub async fn heartbeat(&self, request: &HeartbeatRequest<'_>) -> anyhow::Result<()> {
        self.post_empty("/api/v1/worker/heartbeat", request).await
    }

    pub async fn offline(&self, worker_id: &str) -> anyhow::Result<()> {
        let response = self
            .http
            .delete(self.url(&format!("/api/v1/worker/heartbeat/{worker_id}")))
            .bearer_auth(&self.token)
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(Self::error(response).await);
        }
        Ok(())
    }

    pub async fn claim(
        &self,
        request: &ClaimRequest<'_>,
    ) -> anyhow::Result<Option<WorkerExecutionContext>> {
        let response: ClaimResponse = self.post_json("/api/v1/worker/claim", request).await?;
        Ok(response.execution)
    }

    pub async fn context(&self, execution_id: Uuid) -> anyhow::Result<WorkerExecutionContext> {
        self.get_json(&format!("/api/v1/worker/executions/{execution_id}/context"))
            .await
    }

    pub async fn running(
        &self,
        execution_id: Uuid,
        session_ref: Option<&str>,
    ) -> anyhow::Result<()> {
        self.post_empty(
            &format!("/api/v1/worker/executions/{execution_id}/running"),
            &serde_json::json!({ "sessionRef": session_ref }),
        )
        .await
    }

    pub async fn event(
        &self,
        execution_id: Uuid,
        request: &EventRequest<'_>,
    ) -> anyhow::Result<()> {
        let _: serde_json::Value = self
            .post_json(
                &format!("/api/v1/worker/executions/{execution_id}/events"),
                request,
            )
            .await?;
        Ok(())
    }

    pub async fn complete(
        &self,
        execution_id: Uuid,
        request: &CompleteRequest<'_>,
    ) -> anyhow::Result<bool> {
        let response: serde_json::Value = self
            .post_json(
                &format!("/api/v1/worker/executions/{execution_id}/complete"),
                request,
            )
            .await?;
        Ok(response
            .get("updated")
            .and_then(|value| value.as_bool())
            .unwrap_or(false))
    }

    pub async fn cancel_requested(&self, execution_id: Uuid) -> anyhow::Result<bool> {
        let response: CancelResponse = self
            .get_json(&format!("/api/v1/worker/executions/{execution_id}/cancel"))
            .await?;
        Ok(response.cancel_requested)
    }

    pub async fn connection(
        &self,
        execution_id: Uuid,
        connection_id: Uuid,
    ) -> anyhow::Result<WorkerConnection> {
        self.get_json(&format!(
            "/api/v1/worker/connections/{connection_id}?executionId={execution_id}"
        ))
        .await
    }

    pub async fn create_approval(
        &self,
        execution_id: Uuid,
        action: serde_json::Value,
        wait_seconds: u64,
    ) -> anyhow::Result<Approval> {
        self.post_json(
            "/api/v1/worker/approvals",
            &CreateApprovalRequest {
                execution_id,
                action,
                wait_seconds,
            },
        )
        .await
    }

    pub async fn await_approval(
        &self,
        execution_id: Uuid,
        approval_id: Uuid,
        seconds: u64,
    ) -> anyhow::Result<Approval> {
        // Approval waits are deliberately long; the default client timeout is for
        // ordinary control-plane calls and would abort while a human is deciding.
        let response = self
            .http
            .get(self.url(&format!(
                "/api/v1/worker/approvals/{approval_id}/wait?executionId={execution_id}&seconds={seconds}"
            )))
            .bearer_auth(&self.token)
            .timeout(Duration::from_secs(seconds.clamp(1, 900) + 15))
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(Self::error(response).await);
        }
        Ok(response.json().await?)
    }

    pub async fn audit(
        &self,
        execution_id: Uuid,
        action: &str,
        connection_id: Option<Uuid>,
        metadata: serde_json::Value,
    ) -> anyhow::Result<()> {
        self.post_empty(
            "/api/v1/worker/audit",
            &AuditRequest {
                execution_id,
                action,
                connection_id,
                metadata,
            },
        )
        .await
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn token(&self) -> &str {
        &self.token
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct WorkerMemoryEntry {
    pub kind: String,
    pub content: String,
}

impl WorkerClient {
    pub async fn list_memory(
        &self,
        execution_id: uuid::Uuid,
        agent_id: uuid::Uuid,
        _retention_days: Option<u32>,
    ) -> anyhow::Result<Vec<WorkerMemoryEntry>> {
        let response = self
            .http
            .post(format!("{}/api/v1/worker/memory/list", self.base_url))
            .bearer_auth(&self.token)
            .json(&serde_json::json!({ "executionId": execution_id, "agentId": agent_id }))
            .send()
            .await?;
        if !response.status().is_success() {
            return Ok(vec![]);
        }
        Ok(response.json().await.unwrap_or_default())
    }

    pub async fn store_memory(
        &self,
        execution_id: uuid::Uuid,
        agent_id: uuid::Uuid,
        kind: &str,
        content: &str,
    ) -> anyhow::Result<()> {
        let _ = self
            .http
            .post(format!("{}/api/v1/worker/memory/store", self.base_url))
            .bearer_auth(&self.token)
            .json(&serde_json::json!({
                "executionId": execution_id,
                "agentId": agent_id,
                "kind": kind,
                "content": content,
                "metadata": {},
            }))
            .send()
            .await;
        Ok(())
    }
}

impl WorkerClient {
    /// Fetch the worker profile's resolved environment variables. Transport failures,
    /// non-2xx answers and malformed bodies are errors: an empty list means "no mappings",
    /// never "the lookup failed".
    pub async fn resolve_env(&self) -> anyhow::Result<Vec<(String, String)>> {
        let entries: Vec<WorkerEnv> = self.get_json("/api/v1/worker/env").await?;
        Ok(entries
            .into_iter()
            .map(|entry| (entry.env_var, entry.value))
            .collect())
    }
}

#[cfg(test)]
mod redaction_tests {
    use super::*;

    #[test]
    fn debug_output_never_contains_secret_values() {
        let secret = WorkerSecret {
            secret_ref: "secret://provider/1".into(),
            value: "sk-plaintext".into(),
        };
        let env = WorkerEnv {
            env_var: "GH_TOKEN".into(),
            value: "ghp-plaintext".into(),
        };
        for text in [format!("{secret:?}"), format!("{env:?}")] {
            assert!(!text.contains("plaintext"), "{text}");
            assert!(text.contains("[redacted]"), "{text}");
        }
    }
}
