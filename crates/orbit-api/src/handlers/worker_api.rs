//! Worker-facing control-plane API.
//!
//! Every endpoint is authenticated with an `orbit_wk_` worker token and scoped to the
//! token's organization (and profile, when present). The worker never receives database,
//! NATS or master-key credentials.

use std::collections::BTreeMap;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use orbit_contracts::{
    subjects, ModelEndpoint, ModelProtocol, ModelSelection, OrbitEvent, ProviderCompat,
    ProviderKind, SecretLease,
};
use orbit_domain::{
    AgentSpec, CapabilityEffect, ExecutionEventKind, ExecutionStatus, GrantedCapability, LogLevel,
    ModelManagement, ToolPolicyRule, TriggerSpec,
};
use orbit_store::{
    Approval, Connection, ConnectorRecord, ExecutionRecord, Skill, Store, WorkerProfile,
};

use crate::access::AccessScope;
use crate::error::ApiError;
use crate::state::AppState;
use crate::worker_auth::WorkerAuth;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerSecret {
    pub secret_ref: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerModel {
    pub selection: ModelSelection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<ModelEndpoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<WorkerSecret>,
}

impl WorkerModel {
    pub fn into_resolved(self) -> orbit_contracts::ResolvedModel {
        orbit_contracts::ResolvedModel {
            selection: self.selection,
            endpoint: self.endpoint,
            credential: self
                .credential
                .map(|secret| SecretLease::new(secret.secret_ref, secret.value)),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerSubagentModel {
    pub agent_id: Uuid,
    pub model: Option<WorkerModel>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerExecutionContext {
    pub execution: ExecutionRecord,
    pub spec: AgentSpec,
    pub model: Option<WorkerModel>,
    /// Resolved models for native subagents, keyed by their agent id.
    #[serde(default)]
    pub subagent_models: Vec<WorkerSubagentModel>,
    pub grants: Vec<GrantedCapability>,
    pub skills: Vec<Skill>,
    /// Effective tool policy rules (org defaults + agent rules), enforced by the worker
    /// broker and exposed to native harness hooks.
    pub tool_policies: Vec<ToolPolicyRule>,
    /// Environment values from team-level environment mappings.
    #[serde(default)]
    pub env: Vec<ResolvedEnv>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerSession {
    pub org_id: Uuid,
    pub profile_id: Option<Uuid>,
    pub profile_name: Option<String>,
    pub harnesses: Vec<String>,
    pub target: Option<String>,
    pub execution_driver: Option<String>,
    pub agent_ids: Vec<Uuid>,
    pub skill_slugs: Vec<String>,
    pub allow_web_search: bool,
    pub allow_web_fetch: bool,
    pub native_tool_permissions: serde_json::Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeartbeatRequest {
    pub worker_id: String,
    pub harnesses: Vec<String>,
    pub target: String,
    #[serde(default)]
    pub hostname: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimRequest {
    pub worker_id: String,
    #[serde(default)]
    pub harnesses: Vec<String>,
    pub target: String,
    #[serde(default)]
    pub agent_ids: Vec<Uuid>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimResponse {
    pub execution: Option<WorkerExecutionContext>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunningRequest {
    #[serde(default)]
    pub session_ref: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventRequest {
    pub kind: String,
    pub level: String,
    pub message: String,
    #[serde(default)]
    pub data: serde_json::Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompleteRequest {
    pub status: String,
    #[serde(default)]
    pub output: Option<serde_json::Value>,
    #[serde(default)]
    pub error: Option<serde_json::Value>,
    #[serde(default)]
    pub usage: Option<serde_json::Value>,
    #[serde(default)]
    pub session_ref: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateApprovalRequest {
    pub execution_id: Uuid,
    pub action: serde_json::Value,
    #[serde(default = "default_approval_wait")]
    pub wait_seconds: u64,
}

/// Upper bound for how long an approval request stays decidable (24 h).
const MAX_APPROVAL_WAIT_SECONDS: u64 = 24 * 60 * 60;
/// Cap on simultaneously pending approvals per execution.
const MAX_PENDING_APPROVALS_PER_EXECUTION: i64 = 50;

fn default_approval_wait() -> u64 {
    300
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalQuery {
    pub execution_id: Uuid,
    #[serde(default = "default_approval_wait")]
    pub seconds: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditRequest {
    pub execution_id: Uuid,
    pub action: String,
    #[serde(default)]
    pub connection_id: Option<Uuid>,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelResponse {
    pub cancel_requested: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerConnection {
    pub connection: Connection,
    pub connector: ConnectorRecord,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<WorkerSecret>,
}

async fn active_profile(
    state: &AppState,
    auth: &WorkerAuth,
) -> Result<Option<WorkerProfile>, ApiError> {
    let Some(profile_id) = auth.profile_id else {
        return Ok(None);
    };
    let profile = state
        .store
        .get_worker_profile(auth.org_id, profile_id)
        .await?;
    if !profile.enabled {
        return Err(ApiError::forbidden("worker profile is disabled"));
    }
    Ok(Some(profile))
}

fn intersect_harnesses(token: &WorkerAuth, requested: &[String]) -> Vec<String> {
    let mut requested: Vec<String> = requested
        .iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect();
    if !token.harnesses.is_empty() {
        requested.retain(|harness| token.harnesses.contains(harness));
    }
    requested
}

/// Agent IDs a worker profile may claim. `all` stays dynamic (empty filter), so agents
/// created after the profile are included automatically.
fn profile_agent_ids(profile: &WorkerProfile) -> Vec<Uuid> {
    if profile.agent_scope == "selected" {
        profile.agent_ids.clone()
    } else {
        Vec::new()
    }
}

pub async fn session(
    State(state): State<AppState>,
    auth: WorkerAuth,
) -> Result<Json<WorkerSession>, ApiError> {
    let profile = active_profile(&state, &auth).await?;
    let (
        harnesses,
        target,
        execution_driver,
        agent_ids,
        skill_slugs,
        profile_name,
        allow_web_search,
        allow_web_fetch,
        native_tool_permissions,
    ) = match &profile {
        Some(profile) => (
            vec![profile.harness_kind.clone()],
            Some(profile.target.clone()),
            Some(profile.execution_driver.clone()),
            profile_agent_ids(profile),
            profile.skill_slugs.clone(),
            Some(profile.name.clone()),
            profile.allow_web_search,
            profile.allow_web_fetch,
            profile.native_tool_permissions.clone(),
        ),
        None => (
            auth.harnesses.clone(),
            None,
            None,
            Vec::new(),
            Vec::new(),
            None,
            false,
            false,
            serde_json::json!({}),
        ),
    };
    if harnesses.is_empty() {
        return Err(ApiError::forbidden(
            "worker token has no allowed harnesses; issue a token with a harness list",
        ));
    }
    Ok(Json(WorkerSession {
        org_id: auth.org_id,
        profile_id: auth.profile_id,
        profile_name,
        harnesses,
        target,
        execution_driver,
        agent_ids,
        skill_slugs,
        allow_web_search,
        allow_web_fetch,
        native_tool_permissions,
    }))
}

pub async fn offline(
    State(state): State<AppState>,
    auth: WorkerAuth,
    Path(worker_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let _ = active_profile(&state, &auth).await?;
    // Only the token that owns the worker record may remove it; anything else looks
    // like a missing worker.
    let removed = state
        .store
        .delete_worker_heartbeat(auth.org_id, auth.token_id, &worker_id)
        .await?;
    if !removed {
        return Err(ApiError::not_found("worker"));
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn heartbeat(
    State(state): State<AppState>,
    auth: WorkerAuth,
    crate::extract::ApiJson(body): crate::extract::ApiJson<HeartbeatRequest>,
) -> Result<StatusCode, ApiError> {
    let _ = active_profile(&state, &auth).await?;
    let harnesses = intersect_harnesses(&auth, &body.harnesses);
    let accepted = state
        .store
        .upsert_worker_heartbeat(
            auth.org_id,
            auth.token_id,
            &body.worker_id,
            &harnesses,
            &body.target,
            body.hostname.as_deref(),
        )
        .await?;
    if !accepted {
        return Err(ApiError::conflict(
            "worker id is already registered by another worker token",
        ));
    }
    state.metrics.worker_heartbeat();
    Ok(StatusCode::NO_CONTENT)
}

pub async fn claim(
    State(state): State<AppState>,
    auth: WorkerAuth,
    crate::extract::ApiJson(body): crate::extract::ApiJson<ClaimRequest>,
) -> Result<Json<ClaimResponse>, ApiError> {
    let profile = active_profile(&state, &auth).await?;
    let harnesses = match &profile {
        Some(profile) => vec![profile.harness_kind.clone()],
        None => intersect_harnesses(&auth, &body.harnesses),
    };
    if harnesses.is_empty() {
        return Ok(Json(ClaimResponse { execution: None }));
    }
    let agent_ids = profile
        .as_ref()
        .map(profile_agent_ids)
        .unwrap_or(body.agent_ids);
    let claimed = match &profile {
        Some(profile) => {
            state
                .store
                .claim_next_queued_for_profile(
                    auth.org_id,
                    &body.worker_id,
                    &harnesses,
                    &body.target,
                    &agent_ids,
                    profile.owner_team_id,
                    Some(auth.token_id),
                )
                .await?
        }
        None => {
            state
                .store
                .claim_next_queued(
                    auth.org_id,
                    &body.worker_id,
                    &harnesses,
                    &body.target,
                    &agent_ids,
                    None,
                    Some(auth.token_id),
                )
                .await?
        }
    };
    let execution = match claimed {
        Some(execution) => execution,
        None => {
            state.metrics.worker_claim("empty");
            return Ok(Json(ClaimResponse { execution: None }));
        }
    };
    match build_context(&state, auth.org_id, profile.as_ref(), &execution).await {
        Ok(context) => {
            state.metrics.worker_claim("claimed");
            Ok(Json(ClaimResponse {
                execution: Some(context),
            }))
        }
        Err(error) => {
            state.metrics.worker_claim("error");
            let message = error.message.clone();
            fail_claimed(&state, &execution, &message).await;
            Err(error)
        }
    }
}

pub async fn context(
    State(state): State<AppState>,
    auth: WorkerAuth,
    Path(execution_id): Path<Uuid>,
) -> Result<Json<WorkerExecutionContext>, ApiError> {
    let execution = ensure_execution(&state, &auth, execution_id).await?;
    let profile = active_profile(&state, &auth).await?;
    Ok(Json(
        build_context(&state, auth.org_id, profile.as_ref(), &execution).await?,
    ))
}

async fn build_context(
    state: &AppState,
    org_id: Uuid,
    profile: Option<&WorkerProfile>,
    execution: &ExecutionRecord,
) -> Result<WorkerExecutionContext, ApiError> {
    let version = state
        .store
        .get_agent_version_by_id(org_id, execution.agent_version_id)
        .await?;
    let mut spec: AgentSpec = serde_json::from_value(version.spec)
        .map_err(|error| ApiError::internal(format!("stored agent spec is invalid: {error}")))?;
    merge_team_connections(state, org_id, execution, &mut spec).await;
    omit_fields_workers_ignore(&mut spec);
    if let Some(record) = state
        .store
        .get_harness_by_kind(org_id, &execution.harness_kind)
        .await?
    {
        if !record.enabled {
            return Err(ApiError::conflict(format!(
                "harness '{}' is disabled",
                execution.harness_kind
            )));
        }
    }
    // Defence in depth (#193): the credential is only handed out when the execution's
    // principal can use the provider. The principal is the requester; background runs
    // (triggers, system) have none, so the agent's owner stands in, the same
    // stored-identity approach as the #140 preflight. Legacy unowned agents keep
    // platform authority. Re-evaluated on every context build, so lost access fails
    // the run closed instead of leaking the credential.
    let principal_id = match execution.requested_by {
        Some(user_id) => Some(user_id),
        None => {
            state
                .store
                .get_agent(org_id, execution.agent_id)
                .await?
                .owner_user_id
        }
    };
    let access = match principal_id {
        Some(user_id) => Some(AccessScope::load_for_user(state, org_id, user_id).await?),
        None => None,
    };
    // A team step runs the model its team configured, not only the agent's own (#305).
    // Applied before resolution, so the provider access check covers the model used.
    if let Some(override_model) = execution.input.get("team").and_then(model_override_in) {
        override_model.apply(&mut spec.model);
    }
    let model = resolve_model(&state.store, org_id, &spec, access.as_ref()).await?;
    let mut subagent_models = Vec::new();
    for (agent_id, name, subagent_override) in team_subagents(execution) {
        let agent = state
            .store
            .get_agent(org_id, agent_id)
            .await
            .map_err(|_| ApiError::conflict(format!("subagent '{name}' no longer exists")))?;
        let version = state
            .store
            .get_current_version(org_id, &agent)
            .await
            .map_err(|_| ApiError::conflict(format!("subagent '{name}' has no current version")))?;
        let mut sub_spec: AgentSpec = serde_json::from_value(version.spec).map_err(|error| {
            ApiError::internal(format!("subagent '{name}' spec is invalid: {error}"))
        })?;
        if let Some(override_model) = &subagent_override {
            override_model.apply(&mut sub_spec.model);
        }
        let resolved = resolve_model(&state.store, org_id, &sub_spec, access.as_ref()).await?;
        subagent_models.push(WorkerSubagentModel {
            agent_id,
            model: resolved,
        });
    }
    let grants = resolve_grants(&state.store, org_id, &spec).await;
    let mut slugs = spec.skills.clone();
    slugs.extend(execution.skills.iter().cloned());
    if let Some(profile) = profile {
        slugs.extend(profile.skill_slugs.iter().cloned());
    }
    slugs.sort();
    slugs.dedup();
    let skills = state.store.skills_by_slugs(org_id, &slugs).await?;
    let env = resolve_team_env(&state.store, org_id, execution, &spec, profile).await;
    let tool_policies = require_loaded_policies(
        execution.id,
        state
            .store
            .effective_tool_policy_rules(org_id, execution.agent_id)
            .await,
    )?;
    Ok(WorkerExecutionContext {
        execution: execution.clone(),
        spec,
        model,
        subagent_models,
        grants,
        skills,
        tool_policies,
        env,
    })
}

fn team_ids(execution: &ExecutionRecord, key: &str) -> Vec<Uuid> {
    execution
        .input
        .get("team")
        .and_then(|team| team.get(key))
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().and_then(|id| id.parse().ok()))
                .collect()
        })
        .unwrap_or_default()
}

type SubagentRef = (
    Uuid,
    String,
    Option<crate::handlers::teams::TeamModelOverride>,
);

fn team_subagents(execution: &ExecutionRecord) -> Vec<SubagentRef> {
    execution
        .input
        .get("team")
        .and_then(|team| team.get("subagents"))
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let agent_id = item.get("agentId")?.as_str()?.parse().ok()?;
                    let name = item
                        .get("name")
                        .and_then(|value| value.as_str())
                        .unwrap_or("subagent")
                        .to_string();
                    Some((agent_id, name, model_override_in(item)))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The team or step model override the API recorded with a team execution (#305).
fn model_override_in(
    value: &serde_json::Value,
) -> Option<crate::handlers::teams::TeamModelOverride> {
    value
        .get("modelOverride")
        .filter(|value| !value.is_null())
        .and_then(|value| serde_json::from_value(value.clone()).ok())
}

async fn merge_team_connections(
    state: &AppState,
    org_id: Uuid,
    execution: &ExecutionRecord,
    spec: &mut AgentSpec,
) {
    for connection_id in team_ids(execution, "connectionIds") {
        if spec
            .connectors
            .iter()
            .any(|binding| binding.connection_id == Some(connection_id))
        {
            continue;
        }
        let Ok(row) = state
            .store
            .get_connection_with_connector(org_id, connection_id)
            .await
        else {
            continue;
        };
        let mut capabilities = row.connection.scopes.clone();
        if capabilities.is_empty() {
            if let Ok(Some(connector)) = state
                .store
                .get_connector_by_slug(org_id, &row.connector_slug)
                .await
            {
                if let Some(items) = connector.capabilities.as_array() {
                    capabilities = items
                        .iter()
                        .filter(|item| item.get("effect").and_then(|e| e.as_str()) == Some("read"))
                        .filter_map(|item| item.get("id").and_then(|id| id.as_str()))
                        .map(str::to_string)
                        .collect();
                }
            }
        }
        spec.connectors.push(orbit_domain::ConnectorBinding {
            connector: row.connector_slug,
            connection_id: Some(connection_id),
            capabilities,
        });
    }
}

async fn resolve_team_env(
    store: &Store,
    org_id: Uuid,
    execution: &ExecutionRecord,
    spec: &AgentSpec,
    profile: Option<&WorkerProfile>,
) -> Vec<ResolvedEnv> {
    let mut resolved = Vec::new();
    if let Some(profile) = profile {
        // CLI credentials come from the team's shared connections *and* the agent's own
        // connector bindings. Gh/glab are worker tools, so without the agent bindings a
        // normal (non-team) execution had no GH_TOKEN even though the connection was
        // configured and `allowGh` was on.
        let connection_ids: std::collections::BTreeSet<Uuid> = team_ids(execution, "connectionIds")
            .into_iter()
            .chain(
                spec.connectors
                    .iter()
                    .filter_map(|binding| binding.connection_id),
            )
            .collect();
        for connection_id in connection_ids {
            let Ok(row) = store
                .get_connection_with_connector(org_id, connection_id)
                .await
            else {
                continue;
            };
            let Some(reference) = row.connection.credential_ref.as_deref() else {
                continue;
            };
            let Ok(Some(secret)) = store.get_secret_value_for_org(org_id, reference).await else {
                continue;
            };
            let token = secret.expose().to_string();
            if profile.allow_glab && row.connector_slug == "gitlab" {
                resolved.push(ResolvedEnv {
                    env_var: "GITLAB_TOKEN".into(),
                    value: token.clone(),
                });
                if let Some(root) = row
                    .connection
                    .config
                    .get("cloneBaseUrl")
                    .or_else(|| row.connection.config.get("baseUrl"))
                    .and_then(|value| value.as_str())
                {
                    resolved.push(ResolvedEnv {
                        env_var: "GITLAB_HOST".into(),
                        value: root
                            .trim_end_matches("/api/v4")
                            .trim_end_matches('/')
                            .to_string(),
                    });
                }
            }
            if profile.allow_gh && row.connector_slug == "github" {
                resolved.push(ResolvedEnv {
                    env_var: "GH_TOKEN".into(),
                    value: token,
                });
                if let Some(host) = row
                    .connection
                    .config
                    .get("baseUrl")
                    .and_then(|value| value.as_str())
                {
                    resolved.push(ResolvedEnv {
                        env_var: "GH_HOST".into(),
                        value: host
                            .trim_start_matches("https://")
                            .trim_start_matches("http://")
                            .trim_end_matches("/api/v3")
                            .trim_end_matches('/')
                            .to_string(),
                    });
                }
            }
        }
    }
    for mapping_id in team_ids(execution, "envMappingIds") {
        let Ok(mapping) = store.get_env_mapping(org_id, mapping_id).await else {
            continue;
        };
        let Ok(items) = serde_json::from_value::<Vec<EnvMapping>>(mapping.mappings) else {
            continue;
        };
        for item in items {
            let value = if let Some(connection_id) = item.connection_id {
                match store.get_connection(org_id, connection_id).await {
                    Ok(connection) => match connection.credential_ref {
                        Some(reference) => store
                            .get_secret_value_for_org(org_id, &reference)
                            .await
                            .ok()
                            .flatten()
                            .map(|value| value.expose().to_string()),
                        None => None,
                    },
                    Err(_) => None,
                }
            } else if let Some(reference) = item.secret_ref.as_ref().or(item.secret_key.as_ref()) {
                store
                    .get_secret_value_for_org(org_id, reference)
                    .await
                    .ok()
                    .flatten()
                    .map(|value| value.expose().to_string())
            } else {
                None
            };
            if let Some(value) = value {
                resolved.push(ResolvedEnv {
                    env_var: item.env_var,
                    value,
                });
            }
        }
    }
    resolved
}

async fn fail_claimed(state: &AppState, execution: &ExecutionRecord, message: &str) {
    let event = state
        .store
        .append_execution_event(
            execution.org_id,
            execution.id,
            ExecutionEventKind::Error.as_str(),
            LogLevel::Error,
            message,
            &serde_json::json!({ "error": message }),
        )
        .await;
    match &event {
        Ok(event) => publish_event(state, execution, event).await,
        Err(err) => {
            tracing::warn!(execution_id = %execution.id, error = %err, "failed to append execution event");
        }
    }
    if let Ok(Some(record)) = state
        .store
        .finish_execution(
            execution.id,
            ExecutionStatus::Failed,
            None,
            Some(&serde_json::json!({ "code": "invalid_config", "message": message })),
            None,
            None,
        )
        .await
    {
        publish_status(state, &record).await;
    }
}

pub async fn running(
    State(state): State<AppState>,
    auth: WorkerAuth,
    Path(execution_id): Path<Uuid>,
    crate::extract::ApiJson(body): crate::extract::ApiJson<RunningRequest>,
) -> Result<StatusCode, ApiError> {
    let execution = ensure_execution(&state, &auth, execution_id).await?;
    let updated = state
        .store
        .mark_execution_running(execution_id, body.session_ref.as_deref())
        .await?;
    // The platform may have ended a run its worker took too long to start (or that was
    // cancelled meanwhile). Refuse, so the worker stops instead of running the harness
    // for an execution everyone else already sees as finished.
    if updated.is_none() {
        let current = state
            .store
            .get_execution(execution.org_id, execution_id)
            .await?;
        if ExecutionStatus::parse(&current.status).is_some_and(|status| status.is_terminal()) {
            return Err(ApiError::conflict(format!(
                "execution already ended with status {}",
                current.status
            )));
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn event(
    State(state): State<AppState>,
    auth: WorkerAuth,
    Path(execution_id): Path<Uuid>,
    crate::extract::ApiJson(body): crate::extract::ApiJson<EventRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let execution = ensure_execution(&state, &auth, execution_id).await?;
    let level = match body.level.as_str() {
        "debug" => LogLevel::Debug,
        "info" => LogLevel::Info,
        "warn" => LogLevel::Warn,
        "error" => LogLevel::Error,
        other => {
            return Err(ApiError::bad_request(format!(
                "unknown log level '{other}'"
            )))
        }
    };
    // Unknown kinds would otherwise reach the CHECK constraint and surface as a 500.
    let kind = orbit_domain::ExecutionEventKind::parse(&body.kind).ok_or_else(|| {
        ApiError::validation(vec![orbit_domain::ValidationIssue::new(
            "kind",
            format!("unknown event kind '{}'", body.kind),
        )])
    })?;
    let event = state
        .store
        .append_execution_event(
            execution.org_id,
            execution.id,
            kind.as_str(),
            level,
            &body.message,
            &body.data,
        )
        .await?;
    publish_event(&state, &execution, &event).await;
    Ok(Json(serde_json::to_value(event).unwrap_or_default()))
}

pub async fn complete(
    State(state): State<AppState>,
    auth: WorkerAuth,
    Path(execution_id): Path<Uuid>,
    crate::extract::ApiJson(body): crate::extract::ApiJson<CompleteRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    ensure_execution(&state, &auth, execution_id).await?;
    let status = ExecutionStatus::parse(&body.status)
        .ok_or_else(|| ApiError::bad_request(format!("unknown status '{}'", body.status)))?;
    if !status.is_terminal() {
        return Err(ApiError::bad_request(
            "worker completion requires a terminal status",
        ));
    }
    let updated = state
        .store
        .finish_execution(
            execution_id,
            status,
            body.output.as_ref(),
            body.error.as_ref(),
            body.usage.as_ref(),
            body.session_ref.as_deref(),
        )
        .await?;
    if let Some(record) = &updated {
        publish_status(&state, record).await;
    }
    Ok(Json(serde_json::json!({
        "updated": updated.is_some(),
        "execution": updated,
    })))
}

pub async fn cancel_requested(
    State(state): State<AppState>,
    auth: WorkerAuth,
    Path(execution_id): Path<Uuid>,
) -> Result<Json<CancelResponse>, ApiError> {
    ensure_execution(&state, &auth, execution_id).await?;
    Ok(Json(CancelResponse {
        cancel_requested: state.store.is_cancel_requested(execution_id).await?,
    }))
}

pub async fn create_approval(
    State(state): State<AppState>,
    auth: WorkerAuth,
    crate::extract::ApiJson(body): crate::extract::ApiJson<CreateApprovalRequest>,
) -> Result<Json<Approval>, ApiError> {
    let execution = ensure_execution(&state, &auth, body.execution_id).await?;
    if state
        .store
        .count_pending_approvals(auth.org_id, execution.id)
        .await?
        >= MAX_PENDING_APPROVALS_PER_EXECUTION
    {
        return Err(ApiError::rate_limited(
            "too many pending approvals for this execution",
        ));
    }
    let wait_seconds = body.wait_seconds.clamp(1, MAX_APPROVAL_WAIT_SECONDS) as i64;
    let approval = state
        .store
        .create_approval(
            auth.org_id,
            execution.id,
            body.action,
            Some(Utc::now() + chrono::Duration::seconds(wait_seconds)),
        )
        .await?;
    Ok(Json(approval))
}

pub async fn await_approval(
    State(state): State<AppState>,
    auth: WorkerAuth,
    Path(approval_id): Path<Uuid>,
    Query(query): Query<ApprovalQuery>,
) -> Result<Json<Approval>, ApiError> {
    ensure_execution(&state, &auth, query.execution_id).await?;
    let existing = state.store.get_approval(auth.org_id, approval_id).await?;
    if existing.execution_id != query.execution_id {
        return Err(ApiError::not_found("approval"));
    }
    Ok(Json(
        state
            .store
            .await_approval(
                auth.org_id,
                approval_id,
                std::time::Duration::from_secs(query.seconds.clamp(1, 900)),
            )
            .await?,
    ))
}

pub async fn audit(
    State(state): State<AppState>,
    auth: WorkerAuth,
    crate::extract::ApiJson(body): crate::extract::ApiJson<AuditRequest>,
) -> Result<StatusCode, ApiError> {
    ensure_execution(&state, &auth, body.execution_id).await?;
    state
        .store
        .append_audit_log(
            auth.org_id,
            "worker",
            None,
            None,
            &body.action,
            Some("connection"),
            body.connection_id,
            body.metadata,
            None,
            None,
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionQuery {
    pub execution_id: Uuid,
}

pub async fn connection(
    State(state): State<AppState>,
    auth: WorkerAuth,
    Path(connection_id): Path<Uuid>,
    Query(query): Query<ConnectionQuery>,
) -> Result<Json<WorkerConnection>, ApiError> {
    let execution = ensure_execution(&state, &auth, query.execution_id).await?;
    let version = state
        .store
        .get_agent_version_by_id(auth.org_id, execution.agent_version_id)
        .await?;
    let spec: AgentSpec = serde_json::from_value(version.spec)
        .map_err(|error| ApiError::internal(format!("stored agent spec is invalid: {error}")))?;
    if !spec
        .connectors
        .iter()
        .any(|binding| binding.connection_id == Some(connection_id))
        && !team_ids(&execution, "connectionIds").contains(&connection_id)
    {
        return Err(ApiError::forbidden(
            "connection is not bound to this execution",
        ));
    }
    let connection = state
        .store
        .get_connection_with_connector(auth.org_id, connection_id)
        .await?;
    let connector = state
        .store
        .get_connector_by_slug(auth.org_id, &connection.connector_slug)
        .await?
        .ok_or_else(|| ApiError::not_found("connector"))?;
    let credential = match &connection.connection.credential_ref {
        Some(reference) => {
            state
                .store
                .get_secret_value(reference)
                .await?
                .map(|value| WorkerSecret {
                    secret_ref: reference.clone(),
                    value: value.expose().to_string(),
                })
        }
        None => None,
    };
    Ok(Json(WorkerConnection {
        connection: connection.connection,
        connector,
        credential,
    }))
}

async fn ensure_execution(
    state: &AppState,
    auth: &WorkerAuth,
    execution_id: Uuid,
) -> Result<ExecutionRecord, ApiError> {
    let execution = state.store.get_execution_by_id(execution_id).await?;
    if execution.org_id != auth.org_id {
        return Err(ApiError::not_found("execution"));
    }
    // Bind to the claiming worker. The claim records the token that took the execution;
    // any other token (even of the same org/profile team) sees it as nonexistent.
    // Rows claimed before token binding existed (NULL token, worker id set) stay
    // reachable by org/profile scope so in-flight runs survive an upgrade.
    match state.store.execution_worker_token(execution_id).await? {
        Some(bound) if bound != auth.token_id => return Err(ApiError::not_found("execution")),
        Some(_) => {}
        None if execution.worker_id.is_none() => return Err(ApiError::not_found("execution")),
        None => {}
    }
    if let Some(profile) = active_profile(state, auth).await? {
        if profile.agent_scope == "selected"
            && !profile.agent_ids.is_empty()
            && !profile.agent_ids.contains(&execution.agent_id)
        {
            return Err(ApiError::forbidden(
                "execution agent is outside this worker profile",
            ));
        }
        let agent = state
            .store
            .get_agent(auth.org_id, execution.agent_id)
            .await?;
        let visible = agent.visibility == "org"
            || (profile.owner_team_id.is_some() && agent.owner_team_id == profile.owner_team_id);
        if !visible {
            return Err(ApiError::forbidden(
                "execution agent is outside this worker profile's team",
            ));
        }
    }
    Ok(execution)
}

async fn publish_event(
    state: &AppState,
    execution: &ExecutionRecord,
    event: &orbit_store::ExecutionEventRecord,
) {
    let envelope = OrbitEvent::new(
        subjects::EXECUTION_EVENT,
        "orbit-api-worker",
        serde_json::to_value(event).unwrap_or_default(),
    )
    .with_org(execution.org_id)
    .with_idempotency(format!("event:{}", event.id));
    let _ = state.bus.publish(envelope).await;
}

async fn publish_status(state: &AppState, execution: &ExecutionRecord) {
    let event = OrbitEvent::new(
        subjects::EXECUTION_UPDATED,
        "orbit-api-worker",
        serde_json::json!({
            "executionId": execution.id,
            "agentId": execution.agent_id,
            "status": execution.status,
            "finishedAt": execution.finished_at,
        }),
    )
    .with_org(execution.org_id)
    .with_idempotency(format!("status:{}:{}", execution.id, execution.status));
    let _ = state.bus.publish(event).await;
}

/// Fail closed when the execution's principal may not use the provider.
fn require_principal_can_use(
    access: Option<&AccessScope>,
    provider: &orbit_store::Provider,
) -> Result<(), ApiError> {
    match access {
        Some(access) => access.require_use(
            "provider",
            provider.owner_user_id,
            provider.owner_team_id,
            &provider.visibility,
        ),
        None => Ok(()),
    }
}

async fn resolve_model(
    store: &Store,
    org_id: Uuid,
    spec: &AgentSpec,
    access: Option<&AccessScope>,
) -> Result<Option<WorkerModel>, ApiError> {
    let provider_name = spec.model.provider.clone();
    let model = spec.model.model.clone();

    if spec.model.management == ModelManagement::Harness {
        let mut resolved = WorkerModel {
            selection: ModelSelection::Harness {
                provider: provider_name.clone(),
                model,
            },
            endpoint: None,
            credential: None,
        };
        if let Some(name) = &provider_name {
            if let Some(provider) = store.get_provider_by_name(org_id, name).await? {
                require_principal_can_use(access, &provider)?;
                if let Some(reference) = &provider.secret_ref {
                    let value = store.get_secret_value(reference).await?.ok_or_else(|| {
                        ApiError::conflict(format!(
                            "credential for provider '{name}' is missing from the secret store"
                        ))
                    })?;
                    resolved.credential = Some(WorkerSecret {
                        secret_ref: reference.clone(),
                        value: value.expose().to_string(),
                    });
                }
            }
        }
        return Ok(Some(resolved));
    }

    let name = provider_name.ok_or_else(|| {
        ApiError::conflict("model management is 'platform' but no provider is configured")
    })?;
    let provider = store
        .get_provider_by_name(org_id, &name)
        .await?
        .ok_or_else(|| ApiError::conflict(format!("provider '{name}' does not exist")))?;
    require_principal_can_use(access, &provider)?;
    let kind = ProviderKind::parse(&provider.kind)
        .ok_or_else(|| ApiError::internal("provider has an unknown kind"))?;
    let protocol = match kind {
        ProviderKind::OpenAiCompatible => ModelProtocol::OpenAiCompatible,
        ProviderKind::Anthropic => ModelProtocol::AnthropicMessages,
        ProviderKind::HarnessManaged => {
            return Err(ApiError::conflict(format!(
                "provider '{name}' is harness-managed and cannot be used with platform model management"
            )))
        }
    };
    if !provider.enabled {
        return Err(ApiError::conflict(format!("provider '{name}' is disabled")));
    }
    let base_url = match provider
        .config
        .get("baseUrl")
        .and_then(|value| value.as_str())
    {
        Some(value) => value.to_string(),
        None if protocol == ModelProtocol::AnthropicMessages => {
            "https://api.anthropic.com/v1".to_string()
        }
        None => {
            return Err(ApiError::conflict(format!(
                "provider '{name}' is missing baseUrl"
            )))
        }
    };
    let model_id = model
        .or_else(|| {
            provider
                .config
                .get("model")
                .and_then(|value| value.as_str())
                .map(str::to_string)
        })
        .ok_or_else(|| ApiError::conflict(format!("provider '{name}' is missing a model id")))?;
    let mut headers = BTreeMap::new();
    if let Some(map) = provider
        .config
        .get("headers")
        .and_then(|value| value.as_object())
    {
        for (key, value) in map {
            if let Some(value) = value.as_str() {
                headers.insert(key.clone(), value.to_string());
            }
        }
    }
    let compat: ProviderCompat = provider
        .config
        .get("compat")
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();
    let request_params = provider
        .config
        .get("requestParams")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let context_window = provider
        .config
        .get("contextWindow")
        .and_then(|value| value.as_u64())
        .map(|value| value as u32);
    let max_output_tokens = provider
        .config
        .get("maxOutputTokens")
        .and_then(|value| value.as_u64())
        .map(|value| value as u32);
    let credential = match &provider.secret_ref {
        Some(reference) => Some(WorkerSecret {
            secret_ref: reference.clone(),
            value: store
                .get_secret_value(reference)
                .await?
                .ok_or_else(|| {
                    ApiError::conflict(format!(
                        "credential for provider '{name}' is missing from the secret store"
                    ))
                })?
                .expose()
                .to_string(),
        }),
        None => None,
    };
    Ok(Some(WorkerModel {
        selection: ModelSelection::Platform {
            provider_id: provider.id,
            provider_name: provider.name,
            model: model_id.clone(),
        },
        endpoint: Some(ModelEndpoint {
            base_url,
            model: model_id,
            protocol,
            headers,
            request_params,
            compat,
            context_window,
            max_output_tokens,
        }),
        credential,
    }))
}

/// Tool policies only ever restrict, so a load failure must stop the run instead of
/// continuing with "no rules" (fail closed).
fn require_loaded_policies(
    execution_id: Uuid,
    loaded: Result<Vec<ToolPolicyRule>, orbit_store::StoreError>,
) -> Result<Vec<ToolPolicyRule>, ApiError> {
    loaded.map_err(|error| {
        tracing::error!(%error, %execution_id, "failed to load tool policies; refusing to build context");
        ApiError::internal("failed to load tool policies; run not started")
    })
}

/// Effect of `capability` in a connector catalog, or `None` when it is absent or its effect
/// string does not parse.
fn capability_effect(catalog: &serde_json::Value, capability: &str) -> Option<CapabilityEffect> {
    catalog
        .as_array()?
        .iter()
        .find(|item| item.get("id").and_then(|id| id.as_str()) == Some(capability))?
        .get("effect")?
        .as_str()
        .and_then(CapabilityEffect::parse)
}

async fn resolve_grants(store: &Store, org_id: Uuid, spec: &AgentSpec) -> Vec<GrantedCapability> {
    let mut grants = Vec::new();
    for binding in &spec.connectors {
        let connector = match store
            .get_connector_by_slug(org_id, &binding.connector)
            .await
        {
            Ok(Some(connector)) => connector,
            Ok(None) => continue,
            Err(error) => {
                tracing::warn!(%error, connector = %binding.connector, "grant resolution failed");
                continue;
            }
        };
        for capability in &binding.capabilities {
            // An unknown capability or unparseable effect grants nothing (fail closed).
            let Some(effect) = capability_effect(&connector.capabilities, capability) else {
                tracing::warn!(
                    connector = %binding.connector,
                    %capability,
                    "capability missing from the connector catalog or has an unparseable effect; no grant issued"
                );
                continue;
            };
            grants.push(GrantedCapability {
                connection_id: binding.connection_id.unwrap_or(Uuid::nil()),
                connector: binding.connector.clone(),
                capability: capability.clone(),
                effect,
            });
        }
    }
    grants
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEntry {
    pub id: uuid::Uuid,
    pub kind: String,
    pub content: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Largest memory entry a worker may store.
pub const MAX_MEMORY_CONTENT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListMemoryRequest {
    pub execution_id: uuid::Uuid,
    /// Accepted for backward compatibility; must match the execution's agent if sent.
    #[serde(default)]
    pub agent_id: Option<uuid::Uuid>,
    /// Ignored: retention comes from the execution's agent version, not the caller.
    #[serde(default)]
    pub retention_days: Option<u32>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreMemoryRequest {
    pub execution_id: uuid::Uuid,
    /// Accepted for backward compatibility; must match the execution's agent if sent.
    #[serde(default)]
    pub agent_id: Option<uuid::Uuid>,
    pub kind: String,
    pub content: String,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

/// Memory is always the claimed execution's own agent memory, never a caller-chosen one.
/// Returns the agent and the retention window (`spec.memory.retentionDays`) of the
/// version the execution runs, so retention is platform-enforced rather than worker-sent.
async fn memory_agent(
    state: &AppState,
    auth: &WorkerAuth,
    execution_id: Uuid,
    claimed_agent: Option<Uuid>,
) -> Result<(Uuid, Option<i64>), ApiError> {
    let execution = ensure_execution(state, auth, execution_id).await?;
    if claimed_agent.is_some_and(|agent_id| agent_id != execution.agent_id) {
        return Err(ApiError::forbidden(
            "agentId does not match the execution's agent",
        ));
    }
    let version = state
        .store
        .get_agent_version_by_id(auth.org_id, execution.agent_version_id)
        .await?;
    let retention_days = version
        .spec
        .pointer("/memory/retentionDays")
        .and_then(serde_json::Value::as_i64)
        .filter(|days| *days > 0);
    Ok((execution.agent_id, retention_days))
}

pub async fn list_memory(
    State(state): State<AppState>,
    auth: WorkerAuth,
    crate::extract::ApiJson(body): crate::extract::ApiJson<ListMemoryRequest>,
) -> Result<Json<Vec<MemoryEntry>>, ApiError> {
    let (agent_id, retention_days) =
        memory_agent(&state, &auth, body.execution_id, body.agent_id).await?;
    let entries = state
        .store
        .list_memory(auth.org_id, agent_id, None, 20)
        .await?;
    // Entries are newest first, so filtering after the limit drops exactly the stale tail.
    let cutoff = retention_days.map(|days| Utc::now() - chrono::Duration::days(days));
    Ok(Json(
        entries
            .into_iter()
            .filter(|e| cutoff.is_none_or(|cutoff| e.created_at >= cutoff))
            .map(|e| MemoryEntry {
                id: e.id,
                kind: e.kind,
                content: e.content,
                created_at: e.created_at,
            })
            .collect(),
    ))
}

pub async fn store_memory(
    State(state): State<AppState>,
    auth: WorkerAuth,
    crate::extract::ApiJson(body): crate::extract::ApiJson<StoreMemoryRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if body.content.len() > MAX_MEMORY_CONTENT_BYTES {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload_too_large",
            format!("memory content exceeds {MAX_MEMORY_CONTENT_BYTES} bytes"),
        ));
    }
    let (agent_id, retention_days) =
        memory_agent(&state, &auth, body.execution_id, body.agent_id).await?;
    // After the ownership check, so an unbound execution is a 404 whatever the body says.
    if !matches!(body.kind.as_str(), "short_term" | "summary") {
        return Err(ApiError::validation(vec![
            orbit_domain::ValidationIssue::new("kind", "memory kind must be short_term or summary"),
        ]));
    }
    let expires_at = retention_days.map(|days| Utc::now() + chrono::Duration::days(days));
    state
        .store
        .store_memory(
            auth.org_id,
            agent_id,
            &body.kind,
            &body.content,
            &body.metadata,
            expires_at,
        )
        .await?;
    Ok(Json(serde_json::json!({ "stored": true })))
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvMapping {
    pub env_var: String,
    #[serde(default)]
    pub connection_id: Option<uuid::Uuid>,
    #[serde(default)]
    pub secret_key: Option<String>,
    #[serde(default)]
    pub secret_ref: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedEnv {
    pub env_var: String,
    pub value: String,
}

pub async fn resolve_env(
    State(state): State<AppState>,
    auth: WorkerAuth,
) -> Result<Json<Vec<ResolvedEnv>>, ApiError> {
    // Get the worker's profile to find env_mappings
    let profile = match auth.profile_id {
        Some(id) => state.store.get_worker_profile(auth.org_id, id).await.ok(),
        None => None,
    };
    let mut mappings: Vec<EnvMapping> = Vec::new();
    if let Some(profile) = &profile {
        // Reusable mappings first, then inline mappings (inline wins on duplicates).
        if let Ok(attached) = state
            .store
            .list_worker_profile_env_mappings(profile.id)
            .await
        {
            for mapping in attached {
                if let Ok(items) = serde_json::from_value::<Vec<EnvMapping>>(mapping.mappings) {
                    mappings.extend(items);
                }
            }
        }
        if let Ok(items) = serde_json::from_value::<Vec<EnvMapping>>(profile.env_mappings.clone()) {
            mappings.extend(items);
        }
    }

    let mut resolved = Vec::new();
    for mapping in mappings {
        let value = if let Some(conn_id) = mapping.connection_id {
            // Fetch connection and extract the secret value
            match state.store.get_connection(auth.org_id, conn_id).await {
                Ok(conn) => {
                    if let Some(ref secret_ref) = conn.credential_ref {
                        state
                            .store
                            .get_secret_value_for_org(auth.org_id, secret_ref)
                            .await
                            .ok()
                            .flatten()
                            .map(|s| s.expose().to_string())
                    } else {
                        None
                    }
                }
                Err(_) => None,
            }
        } else if let Some(secret_ref) = mapping.secret_ref.as_ref().or(mapping.secret_key.as_ref())
        {
            state
                .store
                .get_secret_value_for_org(auth.org_id, secret_ref)
                .await
                .ok()
                .flatten()
                .map(|s| s.expose().to_string())
        } else {
            None
        };
        if let Some(value) = value {
            resolved.push(ResolvedEnv {
                env_var: mapping.env_var,
                value,
            });
        }
    }
    Ok(Json(resolved))
}

/// Workers never read triggers, and workers older than the schedule `task` field refuse
/// it (`TriggerSpec` denies unknown fields). Leaving it out keeps those workers able to
/// claim runs of a scheduled agent while a platform upgrade rolls out.
fn omit_fields_workers_ignore(spec: &mut AgentSpec) {
    for trigger in &mut spec.triggers {
        if let TriggerSpec::Schedule { task, .. } = trigger {
            *task = None;
        }
    }
}

#[cfg(test)]
mod worker_payload_tests {
    use super::*;

    #[test]
    fn schedule_task_is_left_out_of_worker_payloads() {
        let mut spec: AgentSpec = serde_json::from_value(serde_json::json!({
            "instructions": { "system": "Help." },
            "triggers": [
                { "type": "manual" },
                { "type": "schedule", "cron": "0 8 * * 1-5", "task": "Summarise new issues" }
            ]
        }))
        .expect("valid spec");
        omit_fields_workers_ignore(&mut spec);
        let json = serde_json::to_value(&spec).unwrap();
        assert_eq!(
            json["triggers"][1],
            serde_json::json!({ "type": "schedule", "cron": "0 8 * * 1-5" })
        );
    }
}

#[cfg(test)]
mod fail_closed_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unknown_or_unparseable_capabilities_grant_nothing() {
        let catalog = json!([
            { "id": "issues.list", "effect": "read" },
            { "id": "issues.create", "effect": "write" },
            { "id": "broken", "effect": "sideways" },
            { "id": "no-effect" }
        ]);
        assert_eq!(
            capability_effect(&catalog, "issues.list"),
            Some(CapabilityEffect::Read)
        );
        assert_eq!(
            capability_effect(&catalog, "issues.create"),
            Some(CapabilityEffect::Write)
        );
        assert_eq!(capability_effect(&catalog, "missing"), None);
        assert_eq!(capability_effect(&catalog, "broken"), None);
        assert_eq!(capability_effect(&catalog, "no-effect"), None);
        assert_eq!(capability_effect(&json!({}), "issues.list"), None);
    }

    #[test]
    fn policy_load_failure_fails_the_context_request() {
        let error = require_loaded_policies(
            Uuid::nil(),
            Err(orbit_store::StoreError::Internal("boom".into())),
        )
        .unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::INTERNAL_SERVER_ERROR);
        assert!(require_loaded_policies(Uuid::nil(), Ok(Vec::new())).is_ok());
    }
}
