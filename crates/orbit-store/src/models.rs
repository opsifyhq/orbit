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
pub struct AgentInstance {
    pub id: Uuid,
    pub org_id: Uuid,
    pub agent_id: Uuid,
    pub agent_version_id: Uuid,
    pub harness_id: Uuid,
    pub name: String,
    pub deployment_target: String,
    pub harness_ref: Option<String>,
    pub state: String,
    pub health: serde_json::Value,
    pub config: serde_json::Value,
    pub last_heartbeat_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
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

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionWithAgent {
    #[serde(flatten)]
    pub execution: ExecutionRecord,
    pub agent_slug: Option<String>,
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
    pub env: serde_json::Value,
    #[sqlx(default)]
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
    pub team_id: Option<Uuid>,
    pub events: Vec<String>,
    pub secret_ref: String,
    pub template: Option<String>,
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
