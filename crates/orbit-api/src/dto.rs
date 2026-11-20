use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use orbit_contracts::{HarnessDescriptor, ProviderKind};
use orbit_domain::{AgentDefinition, AgentSpec, HarnessKind};
use orbit_store::{
    Agent, AgentInstance, AgentVersion, ConnectionWithConnector, ConnectorRecord, DashboardSummary,
    ExecutionEventRecord, ExecutionRecord, Organization, Provider, Trigger, User,
};

// ---------------------------------------------------------------- pagination

#[derive(Debug, Clone, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct PageQuery {
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct AgentListQuery {
    pub limit: Option<u32>,
    pub cursor: Option<String>,
    pub status: Option<String>,
    pub q: Option<String>,
}

#[derive(Debug, Clone, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct ExecutionListQuery {
    pub limit: Option<u32>,
    pub cursor: Option<String>,
    pub status: Option<String>,
    pub agent_id: Option<Uuid>,
    pub archived: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct ArchiveListQuery {
    pub archived: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct EventListQuery {
    pub after_seq: Option<i64>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct AuditListQuery {
    pub limit: Option<u32>,
    pub before_id: Option<i64>,
    pub action: Option<String>,
}

// ------------------------------------------------------------------- agents

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentSummary {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub status: String,
    pub current_version: i32,
    pub labels: serde_json::Value,
    #[schema(value_type = Option<String>)]
    pub harness: Option<HarnessKind>,
    pub autonomy_level: Option<u8>,
    pub model: Option<String>,
    pub connector_count: usize,
    /// Slugs of the skills in the current version, so a skill can show which agents use it.
    pub skills: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl AgentSummary {
    pub fn from_parts(agent: &Agent, spec: Option<&AgentSpec>) -> Self {
        Self {
            id: agent.id,
            slug: agent.slug.clone(),
            name: agent.name.clone(),
            description: agent.description.clone(),
            status: agent.status.clone(),
            current_version: agent.current_version,
            labels: agent.labels.clone(),
            harness: spec.and_then(|s| s.harness.as_ref().map(|harness| harness.kind)),
            autonomy_level: spec.map(|s| s.autonomy.level),
            model: spec.and_then(|s| s.model.model.clone().or_else(|| s.model.provider.clone())),
            connector_count: spec.map(|s| s.connectors.len()).unwrap_or(0),
            skills: spec.map(|s| s.skills.clone()).unwrap_or_default(),
            created_at: agent.created_at,
            updated_at: agent.updated_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentDetail {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub status: String,
    pub current_version: i32,
    pub labels: serde_json::Value,
    pub spec: AgentSpec,
    pub definition_yaml: String,
    pub version_id: Uuid,
    pub version_created_at: DateTime<Utc>,
    pub triggers: Vec<Trigger>,
    pub recent_executions: Vec<ExecutionRecord>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentPage {
    pub items: Vec<AgentSummary>,
    pub next_cursor: Option<String>,
    pub total: i64,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentVersionView {
    pub id: Uuid,
    pub version: i32,
    pub checksum: String,
    pub spec: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

impl From<AgentVersion> for AgentVersionView {
    fn from(version: AgentVersion) -> Self {
        Self {
            id: version.id,
            version: version.version,
            checksum: version.checksum,
            spec: version.spec,
            created_at: version.created_at,
        }
    }
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateAgentRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub labels: Option<serde_json::Value>,
    /// A new full definition; creates a new immutable version when the spec changed.
    #[serde(default)]
    pub definition: Option<AgentDefinition>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentInstanceView {
    #[serde(flatten)]
    pub instance: AgentInstance,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentInstancePage {
    pub items: Vec<AgentInstanceView>,
    pub next_cursor: Option<String>,
    pub total: i64,
}

// ---------------------------------------------------------------- executions

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateExecutionRequest {
    /// The task or message for the agent.
    pub task: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub payload: serde_json::Value,
    #[serde(default)]
    pub idempotency_key: Option<String>,
    /// Skills to materialize for this execution, in addition to the agent spec.
    #[serde(default)]
    pub skills: Vec<String>,
    /// Harness for this execution. `harnessOverride` is accepted as a legacy alias.
    #[serde(default)]
    pub harness: Option<String>,
    #[serde(default)]
    pub harness_override: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionPage {
    pub items: Vec<ExecutionRecord>,
    pub next_cursor: Option<String>,
    pub total: i64,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionEventPage {
    pub items: Vec<ExecutionEventRecord>,
}

// ------------------------------------------------------------------ catalog

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HarnessView {
    pub id: Uuid,
    #[schema(value_type = String)]
    pub kind: HarnessKind,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub implementation_status: String,
    pub capabilities: serde_json::Value,
    pub descriptor: HarnessDescriptor,
    pub config: serde_json::Value,
    pub health: HarnessHealthView,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HarnessHealthView {
    /// `unknown` in Phase 0: harness health is reported by workers in a later phase.
    pub status: String,
    pub message: Option<String>,
    pub reported_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateProviderRequest {
    pub name: String,
    #[serde(rename = "kind")]
    pub kind: String,
    pub config: serde_json::Value,
    /// Raw credential, encrypted at rest. Never returned by the API.
    #[serde(default)]
    pub credential: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProviderRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub config: Option<serde_json::Value>,
    #[serde(default)]
    pub credential: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderView {
    pub id: Uuid,
    pub name: String,
    pub kind: String,
    pub config: serde_json::Value,
    pub enabled: bool,
    pub has_credential: bool,
    pub capabilities: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ProviderView {
    pub fn from_record(provider: Provider) -> Self {
        let capabilities = provider
            .config
            .get("compat")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        Self {
            id: provider.id,
            name: provider.name,
            kind: provider.kind,
            config: provider.config,
            enabled: provider.enabled,
            has_credential: provider.secret_ref.is_some(),
            capabilities,
            created_at: provider.created_at,
            updated_at: provider.updated_at,
        }
    }
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProbeProviderRequest {
    #[serde(rename = "kind")]
    pub kind: String,
    pub config: serde_json::Value,
    #[serde(default)]
    pub credential: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderProbeResponse {
    pub ok: bool,
    pub latency_ms: u64,
    pub models: Vec<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorView {
    pub id: Uuid,
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
}

impl From<ConnectorRecord> for ConnectorView {
    fn from(record: ConnectorRecord) -> Self {
        Self {
            id: record.id,
            slug: record.slug,
            name: record.name,
            version: record.version,
            description: record.description,
            auth: record.auth,
            capabilities: record.capabilities,
            config_schema: record.config_schema,
            webhooks: record.webhooks,
            builtin: record.builtin,
            docs_url: record.docs_url,
        }
    }
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateConnectionRequest {
    pub connector_slug: String,
    pub name: String,
    /// Raw credential (API token, bot token, …). Encrypted at rest.
    #[serde(default)]
    pub credential: Option<String>,
    #[serde(default)]
    pub config: serde_json::Value,
    #[serde(default)]
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateConnectionRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub config: Option<serde_json::Value>,
    #[serde(default)]
    pub scopes: Option<Vec<String>>,
    /// Replacement credential. Omit to keep the current secret.
    #[serde(default)]
    pub credential: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionView {
    pub id: Uuid,
    pub name: String,
    pub status: String,
    pub connector_slug: String,
    pub connector_name: String,
    pub config: serde_json::Value,
    pub scopes: Vec<String>,
    pub has_credential: bool,
    pub last_checked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<ConnectionWithConnector> for ConnectionView {
    fn from(row: ConnectionWithConnector) -> Self {
        Self {
            id: row.connection.id,
            name: row.connection.name,
            status: row.connection.status,
            connector_slug: row.connector_slug,
            connector_name: row.connector_name,
            config: row.connection.config,
            scopes: row.connection.scopes,
            has_credential: row.connection.credential_ref.is_some(),
            last_checked_at: row.connection.last_checked_at,
            created_at: row.connection.created_at,
            updated_at: row.connection.updated_at,
        }
    }
}

// ----------------------------------------------------------------- triggers

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateTriggerRequest {
    pub agent_id: Uuid,
    pub kind: String,
    /// Optional harness for executions fired by this trigger.
    #[serde(default)]
    pub harness: Option<String>,
    #[serde(default)]
    pub config: serde_json::Value,
    #[serde(default)]
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateTriggerRequest {
    #[serde(default)]
    pub enabled: Option<bool>,
}

// ------------------------------------------------------------------ account

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeResponse {
    pub user: User,
    pub organization: Organization,
    pub permissions: Vec<String>,
    pub auth_method: String,
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MetaFeatures {
    pub harness_execution: bool,
    pub connectors_invocation: bool,
    pub approvals: bool,
    pub scheduled_triggers: bool,
    pub webhook_triggers: bool,
    pub oidc: bool,
    pub api_tokens: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MetaResponse {
    pub version: String,
    pub api_version: String,
    pub environment: String,
    pub auth: serde_json::Value,
    pub events_driver: String,
    pub features: MetaFeatures,
    pub license: orbit_security::LicenseStatus,
    /// Route of the embedded documentation index (authenticated).
    pub docs_url: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DashboardResponse {
    pub summary: DashboardSummary,
    pub usage: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateApiTokenRequest {
    pub name: String,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default)]
    pub expires_in_days: Option<i64>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreatedApiToken {
    pub id: Uuid,
    pub name: String,
    /// Plaintext token, shown exactly once.
    pub token: String,
    pub token_prefix: String,
    pub scopes: Vec<String>,
    pub expires_at: Option<DateTime<Utc>>,
}

/// Helper used by handlers to build the effective provider kind.
pub fn parse_provider_kind(value: &str) -> Result<ProviderKind, String> {
    ProviderKind::parse(value).ok_or_else(|| {
        format!("unknown provider kind '{value}'; expected openai_compatible or harness_managed")
    })
}
