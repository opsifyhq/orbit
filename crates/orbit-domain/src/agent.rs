use std::collections::BTreeMap;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    canonical_checksum, is_valid_slug, AutonomyLevel, ExecutionMode, HarnessKind, PlacementTarget,
    ResourceLimits, AGENT_API_VERSION, AGENT_KIND,
};

/// Portable, versioned agent definition (`orbit.io/v1`).
///
/// This is the import/export format. It is validated before persistence; the API
/// accepts and returns it, and `agent_versions.spec` stores the inner `AgentSpec`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentDefinition {
    pub api_version: String,
    pub kind: String,
    pub metadata: AgentMetadata,
    pub spec: AgentSpec,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentMetadata {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentSpec {
    /// Deprecated compatibility field. New agent versions omit it; runtime selection is
    /// per execution/trigger/team step.
    #[serde(default, rename = "harness", skip_serializing_if = "Option::is_none")]
    pub harness: Option<HarnessSpec>,
    #[serde(default)]
    pub model: ModelSpec,
    /// Harness-specific options keyed by harness kind (`opencode`, `pi`, …).
    #[serde(default)]
    pub harness_config: BTreeMap<String, Value>,
    pub instructions: Instructions,
    #[serde(default)]
    pub connectors: Vec<ConnectorBinding>,
    /// Slugs from the organization's skill library to materialize in the workspace.
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub triggers: Vec<TriggerSpec>,
    #[serde(default)]
    pub autonomy: AutonomySpec,
    #[serde(default)]
    pub resources: ResourceLimits,
    #[serde(default)]
    pub memory: MemorySpec,
    /// Defaults used when a manual run is created without an explicit task.
    #[serde(default, skip_serializing_if = "RunDefaults::is_empty")]
    pub run_defaults: RunDefaults,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunDefaults {
    /// Pre-filled task for manual runs. Never used as system instructions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
}

impl RunDefaults {
    pub fn is_empty(&self) -> bool {
        self.task
            .as_deref()
            .map(str::trim)
            .is_none_or(str::is_empty)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HarnessSpec {
    #[serde(rename = "type")]
    pub kind: HarnessKind,
    #[serde(default)]
    pub execution_mode: ExecutionMode,
    /// Where the agent may execute. `auto` lets any matching worker claim it.
    #[serde(default)]
    pub placement: PlacementTarget,
    /// Harness-specific configuration. Validated by the adapter, opaque to the core.
    #[serde(default)]
    pub config: Value,
}

impl HarnessSpec {
    pub fn new(kind: HarnessKind) -> Self {
        Self {
            kind,
            execution_mode: ExecutionMode::OnDemand,
            placement: PlacementTarget::Auto,
            config: Value::Object(Default::default()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModelManagement {
    /// The harness owns provider inventory and authentication (OpenCode Go, Claude Code).
    #[default]
    Harness,
    /// ORBIT supplies a resolved provider/model endpoint to the harness.
    Platform,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelSpec {
    #[serde(default)]
    pub management: ModelManagement,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Passthrough generation parameters for the selected provider.
    #[serde(default)]
    pub params: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Instructions {
    /// Trusted system instructions. Always transported separately from untrusted input.
    pub system: String,
    /// Additional operator constraints, appended to the trusted instruction block.
    #[serde(default)]
    pub constraints: Vec<String>,
}

/// A connector reference. The shorthand string form (`- gitlab`) parses to a binding
/// without a connection, but semantic validation rejects it: a usable connector binding
/// must name the concrete connection that supplies its credentials and access scope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(from = "ConnectorBindingRepr", into = "ConnectorBindingRepr")]
pub struct ConnectorBinding {
    pub connector: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<Uuid>,
    /// Explicit capability allowlist (`issues.list`, `messages.send`, …).
    /// Empty means "manifest default read capabilities only" (never write).
    #[serde(default)]
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum ConnectorBindingRepr {
    Shorthand(String),
    Full {
        connector: String,
        #[serde(
            default,
            rename = "connectionId",
            skip_serializing_if = "Option::is_none"
        )]
        connection_id: Option<Uuid>,
        #[serde(default)]
        capabilities: Vec<String>,
    },
}

impl From<ConnectorBindingRepr> for ConnectorBinding {
    fn from(value: ConnectorBindingRepr) -> Self {
        match value {
            ConnectorBindingRepr::Shorthand(connector) => Self {
                connector,
                connection_id: None,
                capabilities: Vec::new(),
            },
            ConnectorBindingRepr::Full {
                connector,
                connection_id,
                capabilities,
            } => Self {
                connector,
                connection_id,
                capabilities,
            },
        }
    }
}

impl From<ConnectorBinding> for ConnectorBindingRepr {
    fn from(value: ConnectorBinding) -> Self {
        ConnectorBindingRepr::Full {
            connector: value.connector,
            connection_id: value.connection_id,
            capabilities: value.capabilities,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TriggerSpec {
    Schedule {
        /// Standard 5-field cron expression, evaluated in `timezone` (UTC when omitted).
        cron: String,
        /// IANA timezone name such as `Europe/Amsterdam`; defaults to UTC.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timezone: Option<String>,
        /// What the agent is asked to do each time the schedule fires. Without it the
        /// run gets a generic "perform your configured task" prompt.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        task: Option<String>,
    },
    Webhook {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        secret_ref: Option<String>,
    },
    Event {
        subject: String,
    },
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
    /// Approval is required whenever the autonomy level demands it (level 3).
    #[default]
    Auto,
    /// Writes and side effects always require approval.
    Always,
    /// Only external side effects require approval.
    Writes,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutonomySpec {
    #[serde(default = "default_autonomy_level")]
    pub level: u8,
    #[serde(default)]
    pub approval_mode: ApprovalMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_executions_per_hour: Option<u32>,
}

fn default_autonomy_level() -> u8 {
    2
}

impl Default for AutonomySpec {
    fn default() -> Self {
        Self {
            level: default_autonomy_level(),
            approval_mode: ApprovalMode::Auto,
            max_executions_per_hour: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryStrategy {
    #[default]
    None,
    ShortTerm,
    Summary,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemorySpec {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub strategy: MemoryStrategy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention_days: Option<u32>,
}

impl Default for MemorySpec {
    fn default() -> Self {
        Self {
            enabled: false,
            strategy: MemoryStrategy::None,
            retention_days: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ValidationIssue {
    pub path: String,
    pub message: String,
}

impl ValidationIssue {
    pub fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationReport {
    pub issues: Vec<ValidationIssue>,
}

impl ValidationReport {
    pub fn is_valid(&self) -> bool {
        self.issues.is_empty()
    }

    pub fn push(&mut self, path: impl Into<String>, message: impl Into<String>) {
        self.issues.push(ValidationIssue::new(path, message));
    }

    pub fn extend(&mut self, other: ValidationReport) {
        self.issues.extend(other.issues);
    }
}

impl AgentDefinition {
    pub fn new(name: impl Into<String>, spec: AgentSpec) -> Self {
        Self {
            api_version: AGENT_API_VERSION.to_string(),
            kind: AGENT_KIND.to_string(),
            metadata: AgentMetadata {
                name: name.into(),
                description: String::new(),
                labels: BTreeMap::new(),
            },
            spec,
        }
    }

    pub fn checksum(&self) -> String {
        canonical_checksum(&self.spec)
    }

    pub fn from_yaml(input: &str) -> Result<Self, String> {
        let definition: AgentDefinition =
            serde_yaml::from_str(input).map_err(|e| format!("invalid agent YAML: {e}"))?;
        Ok(definition)
    }

    pub fn to_yaml(&self) -> Result<String, String> {
        serde_yaml::to_string(self).map_err(|e| format!("could not serialize agent: {e}"))
    }

    /// Semantic validation. Returns all issues, not just the first, so the UI can show
    /// actionable field-level errors.
    pub fn validate(&self) -> ValidationReport {
        let mut report = ValidationReport::default();
        if self.api_version != AGENT_API_VERSION {
            report.push(
                "apiVersion",
                format!("unsupported apiVersion, expected {AGENT_API_VERSION}"),
            );
        }
        if self.kind != AGENT_KIND {
            report.push("kind", format!("unsupported kind, expected {AGENT_KIND}"));
        }
        let name = self.metadata.name.trim();
        if name.is_empty() || name.chars().count() > 120 {
            report.push("metadata.name", "name must be 1..=120 characters");
        }
        if self.metadata.description.chars().count() > 2000 {
            report.push(
                "metadata.description",
                "description must be <= 2000 characters",
            );
        }
        report.extend(self.spec.validate());
        report
    }
}

impl AgentSpec {
    pub fn validate(&self) -> ValidationReport {
        let mut report = ValidationReport::default();

        // Harness
        if let Some(harness) = &self.harness {
            if !harness.config.is_object() && !harness.config.is_null() {
                report.push("spec.harness.config", "harness config must be an object");
            }
            if harness.kind == HarnessKind::Generic && harness.config.is_null() {
                report.push(
                    "spec.harness.config",
                    "generic harnesses require an explicit config object",
                );
            }
        }

        // Instructions
        if self.instructions.system.trim().is_empty() {
            report.push(
                "spec.instructions.system",
                "system instructions are required",
            );
        }
        if self.instructions.system.chars().count() > 200_000 {
            report.push(
                "spec.instructions.system",
                "system instructions must be <= 200000 characters",
            );
        }
        if self.instructions.constraints.len() > 50 {
            report.push("spec.instructions.constraints", "at most 50 constraints");
        }
        if self
            .run_defaults
            .task
            .as_deref()
            .is_some_and(|task| task.chars().count() > 200_000)
        {
            report.push(
                "spec.runDefaults.task",
                "default task must be <= 200000 characters",
            );
        }

        // Model
        match self.model.management {
            ModelManagement::Platform => {
                if self
                    .model
                    .provider
                    .as_deref()
                    .unwrap_or("")
                    .trim()
                    .is_empty()
                {
                    report.push(
                        "spec.model.provider",
                        "provider is required when model management is platform",
                    );
                }
                if self.model.model.as_deref().unwrap_or("").trim().is_empty() {
                    report.push(
                        "spec.model.model",
                        "model is required when model management is platform",
                    );
                }
            }
            ModelManagement::Harness => {
                if self
                    .model
                    .provider
                    .as_deref()
                    .is_some_and(|p| p.len() > 120)
                {
                    report.push("spec.model.provider", "provider name is too long");
                }
            }
        }

        // Skills
        let mut seen_skills = std::collections::BTreeSet::new();
        if self.skills.len() > 30 {
            report.push("spec.skills", "at most 30 skills per agent");
        }
        for (idx, skill) in self.skills.iter().enumerate() {
            if !is_valid_slug(skill) {
                report.push(format!("spec.skills[{idx}]"), "invalid skill slug");
            }
            if !seen_skills.insert(skill.clone()) {
                report.push(format!("spec.skills[{idx}]"), "duplicate skill");
            }
        }

        // Connectors
        let mut seen = std::collections::BTreeSet::new();
        for (idx, binding) in self.connectors.iter().enumerate() {
            if binding.connection_id.is_none() {
                report.push(
                    format!("spec.connectors[{idx}].connectionId"),
                    "an enabled connector requires a connectionId; remove the connector if it is not in use",
                );
            }
            if !is_valid_slug(&binding.connector) {
                report.push(
                    format!("spec.connectors[{idx}].connector"),
                    "connector must be a valid slug",
                );
            }
            if !seen.insert(binding.connector.clone()) {
                report.push(
                    format!("spec.connectors[{idx}].connector"),
                    "duplicate connector binding",
                );
            }
            for (cidx, capability) in binding.capabilities.iter().enumerate() {
                let valid = !capability.is_empty()
                    && capability.len() <= 120
                    && capability.chars().all(|c| {
                        c.is_ascii_lowercase() || c.is_ascii_digit() || "._:-".contains(c)
                    });
                if !valid {
                    report.push(
                        format!("spec.connectors[{idx}].capabilities[{cidx}]"),
                        "capability must be lowercase dot-separated (e.g. issues.list)",
                    );
                }
            }
        }

        // Triggers
        if self.triggers.len() > 20 {
            report.push("spec.triggers", "at most 20 triggers per agent");
        }
        for (idx, trigger) in self.triggers.iter().enumerate() {
            match trigger {
                TriggerSpec::Schedule {
                    cron,
                    timezone,
                    task,
                } => {
                    if task.as_deref().is_some_and(|task| task.len() > 20_000) {
                        report.push(
                            format!("spec.triggers[{idx}].task"),
                            "task is too long (max 20000 characters)",
                        );
                    }
                    if let Err(error) = crate::schedule::parse_timezone(timezone.as_deref()) {
                        report.push(format!("spec.triggers[{idx}].timezone"), error.to_string());
                    }
                    if cron.parse::<croner::Cron>().is_err() {
                        report.push(
                            format!("spec.triggers[{idx}].cron"),
                            "invalid cron expression (standard 5-field cron)",
                        );
                    }
                }
                TriggerSpec::Event { subject } => {
                    let valid = !subject.is_empty()
                        && subject.len() <= 200
                        && subject
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || "._->*".contains(c));
                    if !valid {
                        report.push(
                            format!("spec.triggers[{idx}].subject"),
                            "invalid event subject filter",
                        );
                    }
                }
                TriggerSpec::Webhook { .. } | TriggerSpec::Manual => {}
            }
        }

        // Autonomy
        if AutonomyLevel::from_level(self.autonomy.level).is_none() {
            report.push("spec.autonomy.level", "autonomy level must be 1..=4");
        }
        if let Some(max) = self.autonomy.max_executions_per_hour {
            if max == 0 || max > 10_000 {
                report.push("spec.autonomy.maxExecutionsPerHour", "must be 1..=10000");
            }
        }

        // Resources
        report.extend(self.resources.validate());

        // Memory
        if self.memory.enabled && self.memory.strategy == MemoryStrategy::None {
            report.push(
                "spec.memory.strategy",
                "memory strategy must be set when memory is enabled",
            );
        }
        if let Some(days) = self.memory.retention_days {
            if days == 0 || days > 3650 {
                report.push("spec.memory.retentionDays", "must be 1..=3650");
            }
        }

        report
    }

    /// Legacy harness kind, when reading old agent versions.
    pub fn harness_kind(&self) -> Option<HarnessKind> {
        self.harness.as_ref().map(|harness| harness.kind)
    }
}

impl FromStr for AgentDefinition {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_yaml(s)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn schedule_trigger_task_is_optional_and_round_trips() {
        let without: TriggerSpec =
            serde_json::from_value(serde_json::json!({ "type": "schedule", "cron": "0 9 * * *" }))
                .unwrap();
        assert!(matches!(without, TriggerSpec::Schedule { task: None, .. }));
        // Older stored specs have no task; they must serialize back unchanged.
        assert!(serde_json::to_value(&without)
            .unwrap()
            .get("task")
            .is_none());

        let with: TriggerSpec = serde_json::from_value(serde_json::json!({
            "type": "schedule",
            "cron": "0 9 * * *",
            "task": "Summarise open issues",
        }))
        .unwrap();
        assert_eq!(
            serde_json::to_value(&with).unwrap()["task"],
            "Summarise open issues"
        );
    }

    use super::*;
    use crate::ResourceLimits;

    fn minimal_spec() -> AgentSpec {
        AgentSpec {
            harness: Some(HarnessSpec::new(HarnessKind::Mock)),
            harness_config: Default::default(),
            model: ModelSpec::default(),
            instructions: Instructions {
                system: "Be helpful.".into(),
                constraints: vec![],
            },
            connectors: vec![],
            skills: vec![],
            triggers: vec![],
            autonomy: AutonomySpec::default(),
            resources: ResourceLimits::default(),
            memory: MemorySpec::default(),
            run_defaults: RunDefaults::default(),
        }
    }

    #[test]
    fn parses_example_definition() {
        let yaml = r#"
apiVersion: orbit.io/v1
kind: Agent
metadata:
  name: social-monitor
  description: Monitors selected topics
spec:
  harness:
    type: opencode
    executionMode: on_demand
  model:
    management: harness
    provider: opencode-go
  instructions:
    system: |
      Monitor the configured topics.
  connectors:
    - connector: telegram
      connectionId: 00000000-0000-4000-8000-000000000001
    - connector: x
      connectionId: 00000000-0000-4000-8000-000000000002
  triggers:
    - type: schedule
      cron: "*/30 * * * *"
  autonomy:
    level: 2
  resources:
    timeoutSeconds: 300
    maxConcurrentExecutions: 1
"#;
        let definition = AgentDefinition::from_yaml(yaml).expect("valid yaml");
        let report = definition.validate();
        assert!(report.is_valid(), "{report:?}");
        assert_eq!(
            definition.spec.harness.as_ref().map(|harness| harness.kind),
            Some(HarnessKind::OpenCode)
        );
        assert_eq!(definition.spec.connectors.len(), 2);
        assert_eq!(definition.spec.triggers.len(), 1);
        let roundtrip = AgentDefinition::from_yaml(&definition.to_yaml().unwrap()).unwrap();
        assert_eq!(roundtrip, definition);
        assert_eq!(roundtrip.checksum(), definition.checksum());
    }

    #[test]
    fn rejects_bad_specs() {
        let mut spec = minimal_spec();
        spec.autonomy.level = 9;
        spec.resources.timeout_seconds = 0;
        spec.triggers.push(TriggerSpec::Schedule {
            cron: "not a cron".into(),
            timezone: None,
            task: None,
        });
        let report = spec.validate();
        assert!(!report.is_valid());
        let paths: Vec<_> = report.issues.iter().map(|i| i.path.as_str()).collect();
        assert!(paths.contains(&"spec.autonomy.level"));
        assert!(paths.contains(&"spec.resources.timeoutSeconds"));
        assert!(paths.iter().any(|p| p.starts_with("spec.triggers[0]")));
    }

    #[test]
    fn requires_platform_model_ids() {
        let mut spec = minimal_spec();
        spec.model.management = ModelManagement::Platform;
        let report = spec.validate();
        assert!(report
            .issues
            .iter()
            .any(|i| i.path == "spec.model.provider"));
        assert!(report.issues.iter().any(|i| i.path == "spec.model.model"));
    }

    #[test]
    fn connector_shorthand_is_normalized() {
        let binding: ConnectorBinding = serde_json::from_str("\"gitlab\"").unwrap();
        assert_eq!(binding.connector, "gitlab");
        assert!(binding.capabilities.is_empty());
    }

    #[test]
    fn connector_binding_requires_a_connection() {
        let mut spec = minimal_spec();
        spec.connectors.push(ConnectorBinding {
            connector: "github".into(),
            connection_id: None,
            capabilities: vec!["issues.list".into()],
        });
        let report = spec.validate();
        let issue = report
            .issues
            .iter()
            .find(|issue| issue.path == "spec.connectors[0].connectionId")
            .expect("missing connector connectionId issue");
        assert_eq!(
            issue.message,
            "an enabled connector requires a connectionId; remove the connector if it is not in use"
        );

        // A shorthand binding parses but is not a valid, enabled connector on its own.
        let mut spec = minimal_spec();
        spec.connectors = vec![serde_json::from_str("\"gitlab\"").unwrap()];
        let report = spec.validate();
        assert!(report
            .issues
            .iter()
            .any(|issue| issue.path == "spec.connectors[0].connectionId"));

        spec.connectors[0].connection_id = Some(Uuid::now_v7());
        assert!(spec.validate().is_valid(), "{:?}", spec.validate());
    }

    #[test]
    fn no_connectors_is_valid() {
        let spec = minimal_spec();
        assert!(spec.connectors.is_empty());
        assert!(spec.validate().is_valid(), "{:?}", spec.validate());
    }
}

/// Agent lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Draft,
    Active,
    Paused,
    Archived,
}

impl AgentStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Archived => "archived",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "draft" => Self::Draft,
            "active" => Self::Active,
            "paused" => Self::Paused,
            "archived" => Self::Archived,
            _ => return None,
        })
    }

    pub fn can_activate(&self) -> bool {
        matches!(self, Self::Draft | Self::Paused)
    }

    pub fn can_pause(&self) -> bool {
        matches!(self, Self::Active)
    }

    pub fn can_archive(&self) -> bool {
        !matches!(self, Self::Archived)
    }
}
