use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Organization {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub mode: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: Uuid,
    pub org_id: Uuid,
    pub email: String,
    pub display_name: String,
    pub role: String,
    pub status: String,
    pub oidc_issuer: Option<String>,
    pub oidc_subject: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
pub struct ApiTokenRecord {
    pub id: Uuid,
    pub org_id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub token_prefix: String,
    pub token_hash: String,
    pub scopes: Vec<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApiTokenView {
    pub id: Uuid,
    pub name: String,
    pub token_prefix: String,
    pub scopes: Vec<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl From<ApiTokenRecord> for ApiTokenView {
    fn from(record: ApiTokenRecord) -> Self {
        Self {
            id: record.id,
            name: record.name,
            token_prefix: record.token_prefix,
            scopes: record.scopes,
            expires_at: record.expires_at,
            last_used_at: record.last_used_at,
            revoked_at: record.revoked_at,
            created_at: record.created_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Agent {
    pub id: Uuid,
    pub org_id: Uuid,
    #[sqlx(default)]
    pub owner_user_id: Option<Uuid>,
    #[sqlx(default)]
    pub owner_team_id: Option<Uuid>,
    #[sqlx(default)]
    pub visibility: String,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub status: String,
    pub current_version: i32,
    pub labels: serde_json::Value,
    pub created_by: Option<Uuid>,
    pub archived_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
pub struct AgentWithSpec {
    #[sqlx(flatten)]
    pub agent: Agent,
    pub spec: Option<serde_json::Value>,
}

#[derive(Debug, Clone, FromRow)]
pub struct ConnectionWithConnector {
    #[sqlx(flatten)]
    pub connection: Connection,
    pub connector_slug: String,
    pub connector_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentVersion {
    pub id: Uuid,
    pub org_id: Uuid,
    pub agent_id: Uuid,
    pub version: i32,
    pub spec: serde_json::Value,
    pub definition_yaml: String,
    pub checksum: String,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HarnessRecord {
    pub id: Uuid,
    pub org_id: Option<Uuid>,
    pub kind: String,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub implementation_status: String,
    pub capabilities: serde_json::Value,
    pub config: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Provider {
    pub id: Uuid,
    pub org_id: Uuid,
    #[sqlx(default)]
    pub owner_user_id: Option<Uuid>,
    #[sqlx(default)]
    pub owner_team_id: Option<Uuid>,
    #[sqlx(default)]
    pub visibility: String,
    pub name: String,
    pub kind: String,
    pub config: serde_json::Value,
    #[serde(skip_serializing)]
    pub secret_ref: Option<String>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorRecord {
    pub id: Uuid,
    pub org_id: Option<Uuid>,
    pub slug: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub auth: serde_json::Value,
    pub capabilities: serde_json::Value,
    pub config_schema: serde_json::Value,
    pub webhooks: bool,
    pub builtin: bool,
    pub docs_url: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    pub id: Uuid,
    pub org_id: Uuid,
    #[sqlx(default)]
    pub owner_user_id: Option<Uuid>,
    #[sqlx(default)]
    pub owner_team_id: Option<Uuid>,
    #[sqlx(default)]
    pub visibility: String,
    pub connector_id: Uuid,
    pub name: String,
    pub status: String,
    #[serde(skip_serializing)]
    pub credential_ref: Option<String>,
    pub config: serde_json::Value,
    pub scopes: Vec<String>,
    pub created_by: Option<Uuid>,
    pub last_checked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Connection {
    /// Whether agents may use this connection.
    ///
    /// `expired` and `revoked` are refusals. `error` only records that the last
    /// connection test failed: the test runs from the control plane, which may not reach
    /// what the workers reach (an internal GitLab, say), and a single failed check must
    /// not take every agent that depends on the connection offline. A credential that is
    /// really wrong still fails, with the service's own error, when it is used.
    pub fn is_usable(&self) -> bool {
        matches!(self.status.as_str(), "active" | "error")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Trigger {
    pub id: Uuid,
    pub org_id: Uuid,
    #[sqlx(default)]
    pub owner_user_id: Option<Uuid>,
    #[sqlx(default)]
    pub owner_team_id: Option<Uuid>,
    #[sqlx(default)]
    pub visibility: String,
    pub agent_id: Uuid,
    pub kind: String,
    #[sqlx(default)]
    pub harness_kind: Option<String>,
    pub enabled: bool,
    pub config: serde_json::Value,
    pub last_fired_at: Option<DateTime<Utc>>,
    pub next_run_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionRecord {
    pub id: Uuid,
    pub org_id: Uuid,
    pub agent_id: Uuid,
    pub agent_version_id: Uuid,
    pub agent_name: String,
    pub harness_kind: String,
    pub trigger_id: Option<Uuid>,
    pub trigger_source: String,
    pub placement: String,
    pub status: String,
    pub session_ref: Option<String>,
    pub input: serde_json::Value,
    #[sqlx(default)]
    pub skills: Vec<String>,
    pub output: Option<serde_json::Value>,
    pub error: Option<serde_json::Value>,
    pub usage: serde_json::Value,
    pub timeout_seconds: i32,
    pub idempotency_key: Option<String>,
    pub requested_by: Option<Uuid>,
    pub worker_id: Option<String>,
    #[sqlx(default)]
    pub last_execution_heartbeat_at: Option<DateTime<Utc>>,
    #[sqlx(default)]
    pub execution_lease_until: Option<DateTime<Utc>>,
    #[sqlx(default)]
    pub worker_boot_id: Option<Uuid>,
    #[sqlx(default)]
    pub worker_boot_started_at: Option<DateTime<Utc>>,
    #[sqlx(default)]
    pub team_run_id: Option<Uuid>,
    #[sqlx(default)]
    pub team_step_index: Option<i32>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    #[sqlx(default)]
    pub running_started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
    pub archived_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionEventRecord {
    pub id: i64,
    pub org_id: Uuid,
    pub execution_id: Uuid,
    pub seq: i64,
    pub kind: String,
    pub level: String,
    pub message: String,
    pub data: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuditLogRecord {
    pub id: i64,
    pub org_id: Uuid,
    pub actor_type: String,
    pub actor_id: Option<Uuid>,
    pub actor_email: Option<String>,
    pub action: String,
    pub target_type: Option<String>,
    pub target_id: Option<Uuid>,
    pub metadata: serde_json::Value,
    pub request_id: Option<String>,
    pub ip_address: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuditLogEntry {
    pub actor_type: String,
    pub actor_id: Option<Uuid>,
    pub actor_email: Option<String>,
    pub action: String,
    pub target_type: Option<String>,
    pub target_id: Option<Uuid>,
    pub metadata: serde_json::Value,
    pub request_id: Option<String>,
    pub ip_address: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Approval {
    pub id: Uuid,
    pub org_id: Uuid,
    pub execution_id: Uuid,
    pub status: String,
    pub requested_action: serde_json::Value,
    pub decided_by: Option<Uuid>,
    pub decided_at: Option<DateTime<Utc>>,
    pub comment: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalWithExecution {
    #[sqlx(flatten)]
    #[serde(flatten)]
    #[schema(inline)]
    pub approval: Approval,
    pub agent_name: String,
    pub agent_id: Uuid,
    pub execution_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DashboardSummary {
    pub total_agents: i64,
    pub active_agents: i64,
    pub paused_agents: i64,
    pub draft_agents: i64,
    pub executions_24h: i64,
    pub failed_executions_24h: i64,
    pub running_executions: i64,
    pub total_connections: i64,
    pub enabled_triggers: i64,
    pub recent_executions: Vec<ExecutionRecord>,
    pub pending_approvals: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    pub id: Uuid,
    pub org_id: Uuid,
    #[sqlx(default)]
    pub owner_user_id: Option<Uuid>,
    #[sqlx(default)]
    pub owner_team_id: Option<Uuid>,
    #[sqlx(default)]
    pub visibility: String,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub content: String,
    pub files: serde_json::Value,
    pub version: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct WorkerTokenRecord {
    pub id: Uuid,
    pub org_id: Uuid,
    pub profile_id: Option<Uuid>,
    pub name: String,
    pub token_prefix: String,
    pub token_hash: String,
    pub harnesses: Vec<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentTeam {
    pub id: Uuid,
    pub org_id: Uuid,
    #[sqlx(default)]
    pub owner_user_id: Option<Uuid>,
    #[sqlx(default)]
    pub owner_team_id: Option<Uuid>,
    #[sqlx(default)]
    pub visibility: String,
    pub name: String,
    pub description: String,
    pub definition: serde_json::Value,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_poll_at: Option<DateTime<Utc>>,
    pub last_poll_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TeamRun {
    pub id: Uuid,
    pub org_id: Uuid,
    pub team_id: Uuid,
    pub status: String,
    pub input: serde_json::Value,
    pub output: Option<serde_json::Value>,
    pub error: Option<String>,
    pub requested_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub archived_at: Option<DateTime<Utc>>,
    #[sqlx(default)]
    pub advance_claimed_at: Option<DateTime<Utc>>,
}

/// One incoming workflow edge that activated a team step. `step` is `null` for an
/// entry edge, which has no source step.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TeamStepActivation {
    pub edge: String,
    #[serde(default)]
    pub step: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TeamStep {
    pub id: Uuid,
    pub run_id: Uuid,
    pub org_id: Uuid,
    pub step_index: i32,
    pub name: String,
    pub agent_id: Uuid,
    pub agent_version_id: Uuid,
    pub skills: Vec<String>,
    pub status: String,
    pub execution_id: Option<Uuid>,
    pub input: serde_json::Value,
    pub output: Option<serde_json::Value>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    /// Workflow runs: the id of the workflow step this ran. `null` for pipeline and
    /// native runs.
    pub node_id: Option<String>,
    /// Workflow runs: 1 for the first time the workflow step ran, 2 for the second…
    pub attempt: i32,
    /// Workflow runs: the outcome label the agent reported, if the step has outcomes.
    pub outcome: Option<String>,
    /// Workflow runs: the lines that started this step, as
    /// `[{ "edge": lineId, "step": sourceStepIndex | null }]`.
    #[schema(value_type = Vec<TeamStepActivation>)]
    pub activated_by: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkerLaunch {
    pub id: Uuid,
    pub org_id: Uuid,
    pub profile_id: Uuid,
    pub name: String,
    pub target: String,
    pub status: String,
    pub runtime_id: Option<String>,
    pub image: String,
    pub worker_id: Option<String>,
    pub worker_token_id: Option<Uuid>,
    pub error: Option<String>,
    #[sqlx(default)]
    pub relaunch_required: bool,
    /// Set for launches that exist for exactly one execution; internal, not serialized.
    #[sqlx(default)]
    #[serde(skip)]
    #[schema(ignore)]
    pub single_execution_id: Option<Uuid>,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub stopped_at: Option<DateTime<Utc>>,
    pub archived_at: Option<DateTime<Utc>>,
}

/// What a response says about a stored profile variable: never the value.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EnvVarState {
    pub has_value: bool,
}

/// Keys an inline env mapping may carry: references to secrets/connections, no values.
pub const INLINE_ENV_MAPPING_KEYS: [&str; 4] = ["envVar", "connectionId", "secretKey", "secretRef"];

fn serialize_env_redacted<S: serde::Serializer>(
    env: &serde_json::Value,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    use serde::ser::SerializeMap;
    let empty = serde_json::Map::new();
    let map = env.as_object().unwrap_or(&empty);
    let mut out = serializer.serialize_map(Some(map.len()))?;
    for (name, value) in map {
        let has_value = value.as_str().is_some_and(|text| !text.is_empty());
        out.serialize_entry(name, &EnvVarState { has_value })?;
    }
    out.end()
}

/// What a response carries for `env` is names and flags, never values, so there is
/// nothing to read back: a deserialized profile has no env. (Not `skip_deserializing`:
/// that also drops the field from the OpenAPI schema, and the response does contain it.)
fn deserialize_env_ignored<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<serde_json::Value, D::Error> {
    use serde::Deserialize;
    serde::de::IgnoredAny::deserialize(deserializer)?;
    Ok(serde_json::Value::Null)
}

fn serialize_env_mappings_refs<S: serde::Serializer>(
    mappings: &serde_json::Value,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    use serde::Serialize;
    let cleaned = match mappings {
        serde_json::Value::Array(items) => serde_json::Value::Array(
            items
                .iter()
                .map(|item| match item {
                    serde_json::Value::Object(map) => serde_json::Value::Object(
                        map.iter()
                            .filter(|(key, _)| INLINE_ENV_MAPPING_KEYS.contains(&key.as_str()))
                            .map(|(key, value)| (key.clone(), value.clone()))
                            .collect(),
                    ),
                    other => other.clone(),
                })
                .collect(),
        ),
        other => other.clone(),
    };
    cleaned.serialize(serializer)
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkerProfile {
    pub id: Uuid,
    pub org_id: Uuid,
    #[sqlx(default)]
    pub owner_user_id: Option<Uuid>,
    #[sqlx(default)]
    pub owner_team_id: Option<Uuid>,
    #[sqlx(default)]
    pub visibility: String,
    pub name: String,
    pub harness_kind: String,
    pub target: String,
    pub execution_driver: String,
    pub image: String,
    pub image_tag: String,
    #[sqlx(default)]
    pub agent_scope: String,
    pub agent_ids: Vec<Uuid>,
    pub skill_slugs: Vec<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub auth_secret_name: Option<String>,
    /// Stored variable values. Never serialized: responses carry only names and a
    /// `hasValue` flag (AGENTS.md rule 4). Read the field directly for the launcher
    /// payload and for update merging.
    #[serde(
        serialize_with = "serialize_env_redacted",
        deserialize_with = "deserialize_env_ignored",
        default
    )]
    #[schema(value_type = std::collections::BTreeMap<String, EnvVarState>)]
    pub env: serde_json::Value,
    /// References only (`envVar` plus `connectionId` / `secretKey` / `secretRef`); any
    /// other key stored by an older version is dropped from responses.
    #[sqlx(default)]
    #[serde(
        serialize_with = "serialize_env_mappings_refs",
        default,
        skip_deserializing
    )]
    pub env_mappings: serde_json::Value,
    pub resources: serde_json::Value,
    #[sqlx(default)]
    pub allow_web_search: bool,
    #[sqlx(default)]
    pub allow_web_fetch: bool,
    #[sqlx(default)]
    pub allow_glab: bool,
    #[sqlx(default)]
    pub allow_gh: bool,
    #[sqlx(default)]
    pub native_tool_permissions: serde_json::Value,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Stored tool policy rule. `matcher` is JSON so the domain type is the single source of
/// truth for matching fields; invalid rows are skipped when building execution context.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolPolicy {
    pub id: Uuid,
    pub org_id: Uuid,
    #[sqlx(default)]
    pub owner_user_id: Option<Uuid>,
    #[sqlx(default)]
    pub owner_team_id: Option<Uuid>,
    #[sqlx(default)]
    pub visibility: String,
    pub agent_id: Option<Uuid>,
    pub name: String,
    pub priority: i32,
    pub enabled: bool,
    pub action: String,
    pub matcher: serde_json::Value,
    pub reason: Option<String>,
    pub impact: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CustomRole {
    pub id: Uuid,
    pub org_id: Uuid,
    pub name: String,
    pub permissions: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EnvMapping {
    pub id: Uuid,
    pub org_id: Uuid,
    pub name: String,
    pub description: String,
    #[serde(serialize_with = "serialize_env_mappings_refs")]
    pub mappings: serde_json::Value,
    pub owner_user_id: Option<Uuid>,
    pub owner_team_id: Option<Uuid>,
    pub visibility: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Output {
    pub id: Uuid,
    pub org_id: Uuid,
    pub name: String,
    pub kind: String,
    pub agent_id: Option<Uuid>,
    /// All agent targets. Empty with an empty team_id means organisation-wide.
    #[sqlx(default)]
    pub agent_ids: Vec<Uuid>,
    pub team_id: Option<Uuid>,
    pub events: Vec<String>,
    pub secret_ref: String,
    pub template: Option<String>,
    pub telegram_chat_id: Option<String>,
    pub telegram_message_thread_id: Option<String>,
    /// S3 bucket/endpoint/prefix settings; empty for other output kinds.
    pub config: serde_json::Value,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
pub struct OutputDeliveryJob {
    pub id: Uuid,
    pub org_id: Uuid,
    pub output_id: Uuid,
    pub execution_id: Option<Uuid>,
    pub team_run_id: Option<Uuid>,
    pub attempts: i32,
    pub claim_token: Uuid,
    /// Multi-part messages (Telegram) already accepted by the destination on previous
    /// attempts. A retry resumes after this part.
    pub parts_sent: i32,
}

/// One delivery of an output, for showing its history. `task` and `agent_name` come from
/// the run it reports on (absent once that run is deleted).
#[derive(Debug, Clone, FromRow)]
pub struct OutputDelivery {
    pub id: Uuid,
    pub output_id: Uuid,
    pub execution_id: Option<Uuid>,
    pub team_run_id: Option<Uuid>,
    pub status: String,
    pub attempts: i32,
    pub response_status: Option<i32>,
    pub last_error: Option<String>,
    pub next_attempt_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub task: Option<String>,
    pub agent_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccessTeam {
    pub id: Uuid,
    pub org_id: Uuid,
    pub name: String,
    pub slug: String,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TeamMember {
    pub team_id: Uuid,
    pub user_id: Uuid,
    pub role: String,
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod redaction_tests {
    use super::*;

    fn profile(env: serde_json::Value, mappings: serde_json::Value) -> WorkerProfile {
        WorkerProfile {
            id: Uuid::nil(),
            org_id: Uuid::nil(),
            owner_user_id: None,
            owner_team_id: None,
            visibility: "org".into(),
            name: "p".into(),
            harness_kind: "mock".into(),
            target: "docker".into(),
            execution_driver: "process".into(),
            image: "i".into(),
            image_tag: "".into(),
            agent_scope: "all".into(),
            agent_ids: vec![],
            skill_slugs: vec![],
            provider: None,
            model: None,
            auth_secret_name: None,
            env,
            env_mappings: mappings,
            resources: serde_json::json!({}),
            allow_web_search: false,
            allow_web_fetch: false,
            allow_glab: false,
            allow_gh: false,
            native_tool_permissions: serde_json::json!({}),
            enabled: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn serialized_profiles_carry_names_and_flags_never_values() {
        let profile = profile(
            serde_json::json!({ "KEY": "sk-secret", "EMPTY": "" }),
            serde_json::json!([{ "envVar": "A", "secretKey": "k", "value": "sk-stray" }]),
        );
        let json = serde_json::to_value(&profile).unwrap();
        assert_eq!(
            json["env"],
            serde_json::json!({ "KEY": { "hasValue": true }, "EMPTY": { "hasValue": false } })
        );
        assert_eq!(
            json["envMappings"],
            serde_json::json!([{ "envVar": "A", "secretKey": "k" }])
        );
        assert!(!json.to_string().contains("sk-"));
        // The raw field is untouched for the launcher payload.
        assert_eq!(profile.env["KEY"], "sk-secret");
    }
}
