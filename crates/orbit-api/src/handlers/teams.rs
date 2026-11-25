//! Agent teams: sequential role handoffs materialized as normal executions.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use orbit_domain::{
    HarnessKind, TeamAutonomy, TeamAutonomyMode, TriggerSource, ValidationIssue,
    WorkflowDefinition, WorkflowNode, WORKFLOW_MODE,
};
use orbit_security::{AuthContext, Permission};
use orbit_store::{AgentTeam, ExecutionRecord, TeamRun, TeamStep};

use crate::dto::{TeamRunListQuery, TeamRunPage};
use crate::error::ApiError;
use crate::state::AppState;
use crate::work_sources::{
    validate_repositories, validate_work_source, RepositorySpec, WorkSourceDefinition,
};

/// The explicit run instruction is the only text presented as the task to the harness.
fn run_instruction(run_input: &serde_json::Value) -> &str {
    run_input
        .get("task")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("Team run")
}

/// Team goal/instructions are configuration. They travel as trusted system context,
/// never as part of the run instruction.
fn configured_goal(run_input: &serde_json::Value) -> &str {
    run_input
        .get("instructions")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
}

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TeamStepInput {
    pub agent_id: Uuid,
    #[serde(default)]
    pub model: Option<TeamModelOverride>,
}

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateTeamRequest {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// `pipeline` runs each agent once in order. `native` runs the primary agent only
    /// and materializes the remaining agents as harness-native subagents. `workflow` runs
    /// the graph in `workflow`: steps joined by conditional lines, with parallel
    /// branches, joins and bounded loops.
    #[serde(default = "default_mode")]
    pub mode: String,
    /// The workflow graph. Required when `mode` is `workflow`; `steps` is then derived
    /// from it and may be left out.
    #[serde(default)]
    pub workflow: Option<WorkflowDefinition>,
    /// Shared goal/instruction for the team. Passed to every step as configured system
    /// context, never mixed into the run instruction shown as the task.
    #[serde(default)]
    pub instructions: String,
    /// Structured workspace repository (owner/repo). Instructions alone cannot drive
    /// repository checkout; this is passed to the worker.
    #[serde(default)]
    pub repository: Option<String>,
    /// Optional base/working branch for repository checkout.
    #[serde(default)]
    pub branch: Option<String>,
    /// Team-level model override (optional).
    #[serde(default)]
    pub model: Option<TeamModelOverride>,
    /// Automatic issue discovery/claiming configuration.
    #[serde(default)]
    pub work_source: Option<WorkSourceDefinition>,
    /// Multi-repository workspace. When empty, `repository`/`branch` are used.
    #[serde(default)]
    pub repositories: Vec<RepositorySpec>,
    /// Worker profile used for automatic launch.
    #[serde(default)]
    pub worker_profile_id: Option<Uuid>,
    #[serde(default)]
    pub auto_launch: bool,
    /// Skills materialized for every step in the team.
    #[serde(default)]
    pub skills: Vec<String>,
    /// Harness for team steps. Falls back to the legacy agent harness or org default.
    #[serde(default)]
    pub harness: Option<String>,
    #[serde(default)]
    pub connection_ids: Vec<Uuid>,
    #[serde(default)]
    pub env_mapping_ids: Vec<Uuid>,
    /// Per-step wall-clock timeout; defaults to 1 hour for team runs.
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    /// Team-wide autonomy policy. Omitted/follow_primary keeps each step agent's own level.
    #[serde(default)]
    pub autonomy: Option<TeamAutonomy>,
    /// The agents in order. Not needed for a workflow.
    #[serde(default)]
    pub steps: Vec<TeamStepInput>,
    /// PATCH only: the team's `updatedAt` as the client read it. When the team changed
    /// since, the update is refused with 409 instead of overwriting the other change.
    #[serde(default)]
    pub expected_updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateTeamRunRequest {
    /// Optional run-specific task. Falls back to the team instruction.
    #[serde(default)]
    pub task: String,
    #[serde(default)]
    pub payload: serde_json::Value,
    /// Ask the worker to auto-approve approvals for this run. It only applies when the
    /// execution's level is already L3+, and never raises the level.
    #[serde(default)]
    pub auto_approve: bool,
    /// Override the agent's stored harness for this run.
    #[serde(default)]
    pub harness_override: Option<String>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TeamRunDetail {
    pub run: TeamRun,
    pub steps: Vec<TeamStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamDefinition {
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default)]
    pub instructions: String,
    #[serde(default)]
    pub repository: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub work_source: Option<WorkSourceDefinition>,
    #[serde(default)]
    pub repositories: Vec<RepositorySpec>,
    #[serde(default)]
    pub worker_profile_id: Option<Uuid>,
    #[serde(default)]
    pub auto_launch: bool,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub harness: Option<String>,
    #[serde(default)]
    pub connection_ids: Vec<Uuid>,
    #[serde(default)]
    pub env_mapping_ids: Vec<Uuid>,
    #[serde(default)]
    pub model: Option<TeamModelOverride>,
    /// Wall-clock timeout for each step. Defaults to 1 hour for team runs.
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    /// Explicit team autonomy. Missing means follow each step agent's level.
    #[serde(default)]
    pub autonomy: Option<TeamAutonomy>,
    /// The agents in order. For a workflow: every agent it uses, once.
    #[serde(default)]
    pub steps: Vec<TeamStepDefinition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<WorkflowDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TeamModelOverride {
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub management: Option<String>,
}

impl TeamModelOverride {
    /// Apply the override to an agent's model settings. Fields the override leaves out
    /// keep the agent's own value.
    pub(crate) fn apply(&self, model: &mut orbit_domain::ModelSpec) {
        if let Some(provider) = &self.provider {
            model.provider = Some(provider.clone());
        }
        if let Some(value) = &self.model {
            model.model = Some(value.clone());
        }
        if let Some(management) = &self.management {
            model.management = match management.as_str() {
                "platform" => orbit_domain::ModelManagement::Platform,
                _ => orbit_domain::ModelManagement::Harness,
            };
        }
    }
}

impl TeamDefinition {
    /// The graph, when this team runs as a workflow.
    pub(crate) fn workflow(&self) -> Option<&WorkflowDefinition> {
        if self.mode == WORKFLOW_MODE {
            self.workflow.as_ref()
        } else {
            None
        }
    }

    /// The model override for a step: the step's own, else the team's.
    pub(crate) fn model_override(&self, step_index: usize) -> Option<&TeamModelOverride> {
        self.steps
            .get(step_index)
            .and_then(|step| step.model.as_ref())
            .or(self.model.as_ref())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamStepDefinition {
    pub agent_id: Uuid,
    #[serde(default)]
    pub model: Option<TeamModelOverride>,
}

fn default_mode() -> String {
    "pipeline".into()
}

/// Check the mode and, for a workflow, the graph; then derive `steps` from it so access
/// checks, autonomy elevation, the queue and exports see every agent the workflow uses.
fn prepare_mode(body: &mut CreateTeamRequest) -> Result<(), ApiError> {
    if !matches!(body.mode.as_str(), "pipeline" | "native" | WORKFLOW_MODE) {
        return Err(ApiError::validation(vec![ValidationIssue::new(
            "mode",
            "mode must be pipeline, native or workflow",
        )]));
    }
    if body.mode != WORKFLOW_MODE {
        // A graph kept from an earlier save survives a switch to another mode, as long
        // as it is still a valid graph.
        if body
            .workflow
            .as_ref()
            .is_some_and(|workflow| !workflow.validate().is_empty())
        {
            body.workflow = None;
        }
        return Ok(());
    }
    let Some(workflow) = body.workflow.as_mut() else {
        return Err(ApiError::validation(vec![ValidationIssue::new(
            "workflow",
            "a workflow team needs a workflow",
        )]));
    };
    for node in &mut workflow.nodes {
        node.name = node.name.trim().to_string();
        node.instructions = node.instructions.trim().to_string();
    }
    let issues = workflow.validate();
    if !issues.is_empty() {
        return Err(ApiError::validation(issues));
    }
    body.steps = workflow
        .agent_ids()
        .into_iter()
        .map(|agent_id| TeamStepInput {
            agent_id,
            model: None,
        })
        .collect();
    Ok(())
}

/// Where a step's agent was given, for a validation message (#438): `steps[i].agentId`,
/// or for a workflow every node that uses the agent (`workflow.nodes[j].agentId`), since
/// a workflow's steps are derived from its nodes.
fn step_agent_paths(body: &CreateTeamRequest, index: usize, agent_id: Uuid) -> Vec<String> {
    if let Some(workflow) = body
        .workflow
        .as_ref()
        .filter(|_| body.mode == WORKFLOW_MODE)
    {
        let paths: Vec<String> = workflow
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| node.agent_id == agent_id)
            .map(|(position, _)| format!("workflow.nodes[{position}].agentId"))
            .collect();
        if !paths.is_empty() {
            return paths;
        }
    }
    vec![format!("steps[{index}].agentId")]
}

pub(crate) fn parse_definition(team: &AgentTeam) -> Result<TeamDefinition, ApiError> {
    serde_json::from_value(team.definition.clone())
        .map_err(|error| ApiError::internal(format!("team definition is invalid: {error}")))
}

/// Connections a team run injects beyond the agent specs: the team's shared connections
/// and an enabled work source connection.
pub(crate) fn team_connection_ids(definition: &TeamDefinition) -> Vec<Uuid> {
    let mut ids = definition.connection_ids.clone();
    if let Some(source) = &definition.work_source {
        if source.enabled {
            ids.extend(source.connection_id);
        }
    }
    ids
}

/// A caller-supplied repository must be one of the team's configured repositories; the
/// run injects the team's connection credentials, so it must not retarget them.
fn validate_requested_repositories(
    definition: &TeamDefinition,
    payload: &serde_json::Value,
) -> Result<(), ApiError> {
    check_requested_repositories(definition, payload, |_| false)
}

/// The repository rule for queue items (#384): the same targets as a run, plus the
/// projects of a GitLab group work source. `stored` relaxes the group rule for items that
/// are already in the queue (the dispatcher records projects of a group given by numeric
/// id, which cannot be matched by path); callers enqueuing new items get the strict rule.
pub(crate) fn validate_queue_repositories(
    definition: &TeamDefinition,
    payload: &serde_json::Value,
    stored: bool,
) -> Result<(), ApiError> {
    check_requested_repositories(definition, payload, |value| {
        definition.work_source.as_ref().is_some_and(|source| {
            crate::work_sources::group_includes_project(source, value, stored)
        })
    })
}

fn check_requested_repositories(
    definition: &TeamDefinition,
    payload: &serde_json::Value,
    also_allowed: impl Fn(&str) -> bool,
) -> Result<(), ApiError> {
    let mut allowed: Vec<String> = Vec::new();
    allowed.extend(definition.repository.iter().cloned());
    allowed.extend(
        definition
            .repositories
            .iter()
            .map(|repository| repository.repository.clone()),
    );
    if let Some(source) = &definition.work_source {
        allowed.extend(source.repository.iter().cloned());
        allowed.extend(source.project.iter().cloned());
    }
    let is_allowed = |value: &str| {
        allowed
            .iter()
            .any(|candidate| candidate.trim().eq_ignore_ascii_case(value.trim()))
            || also_allowed(value)
    };
    if let Some(repository) = payload.get("repository").and_then(|value| value.as_str()) {
        if !is_allowed(repository) {
            return Err(ApiError::validation(vec![ValidationIssue::new(
                "payload.repository",
                "repository is not one of the team's configured repositories",
            )]));
        }
    }
    if let Some(items) = payload
        .get("repositories")
        .and_then(|value| value.as_array())
    {
        for (idx, item) in items.iter().enumerate() {
            let repository = item
                .get("repository")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            if !is_allowed(repository) {
                return Err(ApiError::validation(vec![ValidationIssue::new(
                    format!("payload.repositories[{idx}].repository"),
                    "repository is not one of the team's configured repositories",
                )]));
            }
        }
    }
    Ok(())
}

async fn validate_work_source_connection(
    state: &AppState,
    org_id: Uuid,
    access: &crate::access::AccessScope,
    source: &WorkSourceDefinition,
    issues: &mut Vec<ValidationIssue>,
) {
    if !source.enabled {
        return;
    }
    let Some(connection_id) = source.connection_id else {
        return;
    };
    match state
        .store
        .get_connection_with_connector(org_id, connection_id)
        .await
    {
        // A connection the caller cannot view is reported exactly like a missing one.
        Ok(connection)
            if !access.can_view(
                connection.connection.owner_user_id,
                connection.connection.owner_team_id,
                &connection.connection.visibility,
            ) =>
        {
            issues.push(ValidationIssue::new(
                "workSource.connectionId",
                format!("unknown connection '{connection_id}'"),
            ))
        }
        // The dispatcher polls and writes outcomes with this credential (#322).
        Ok(connection)
            if !access.can_use(
                connection.connection.owner_user_id,
                connection.connection.owner_team_id,
                &connection.connection.visibility,
            ) =>
        {
            issues.push(ValidationIssue::new(
                "workSource.connectionId",
                format!(
                    "you cannot use connection '{}' for a work source",
                    connection.connection.name
                ),
            ))
        }
        Ok(connection) => {
            if connection.connector_slug != source.provider {
                issues.push(ValidationIssue::new(
                    "workSource.connectionId",
                    format!(
                        "connection belongs to '{}', not '{}'",
                        connection.connector_slug, source.provider
                    ),
                ));
            } else if !connection.connection.is_usable() {
                issues.push(ValidationIssue::new(
                    "workSource.connectionId",
                    format!(
                        "connection '{}' is {}",
                        connection.connection.name, connection.connection.status
                    ),
                ));
            }
        }
        Err(_) => issues.push(ValidationIssue::new(
            "workSource.connectionId",
            format!("unknown connection '{connection_id}'"),
        )),
    }
}

fn work_source_to_json(source: &WorkSourceDefinition) -> serde_json::Value {
    serde_json::to_value(source).unwrap_or(serde_json::Value::Null)
}

fn repositories_to_json(repositories: &[RepositorySpec]) -> serde_json::Value {
    serde_json::json!(repositories)
}

async fn validate_native_primary(
    state: &AppState,
    org_id: Uuid,
    mode: &str,
    steps: &[TeamStepInput],
    issues: &mut Vec<ValidationIssue>,
) {
    if mode != "native" {
        return;
    }
    let Some(primary) = steps.first() else {
        return;
    };
    let Ok(agent) = state.store.get_agent(org_id, primary.agent_id).await else {
        return;
    };
    let Ok(version) = state.store.get_current_version(org_id, &agent).await else {
        return;
    };
    match serde_json::from_value::<orbit_domain::AgentSpec>(version.spec) {
        Ok(spec)
            if spec
                .harness
                .as_ref()
                .map(|harness| harness.kind)
                .unwrap_or(HarnessKind::OpenCode)
                == HarnessKind::OpenCode => {}
        Ok(_) => issues.push(ValidationIssue::new(
            "mode",
            "native Agent Teams currently require an OpenCode primary agent; Pi and other harnesses can use pipeline mode",
        )),
        Err(error) => issues.push(ValidationIssue::new(
            "mode",
            format!("primary agent spec is invalid: {error}"),
        )),
    }
}

/// A step agent a fixed team policy runs less strictly than the agent's own policy.
struct AutonomyElevation {
    agent_id: Uuid,
    name: String,
    baseline: orbit_domain::AutonomyPolicy,
}

/// Validate a fixed team autonomy and authorise any elevation over a step agent's own
/// policy (#385). An elevation is any team policy less strict than the agent's, by level
/// **or** approval mode (see `AutonomyPolicy::is_at_least_as_strict_as`); it needs manage
/// access to that agent. Returns the elevations and the policy to store, whose
/// `elevations` list is recomputed here and never taken from the request. An agent whose
/// policy cannot be read fails the save instead of being skipped.
async fn validate_team_autonomy(
    state: &AppState,
    auth: &AuthContext,
    access: &crate::access::AccessScope,
    autonomy: Option<&TeamAutonomy>,
    steps: &[TeamStepInput],
    issues: &mut Vec<ValidationIssue>,
) -> (Vec<AutonomyElevation>, Option<TeamAutonomy>) {
    let mut elevated = Vec::new();
    let Some(autonomy) = autonomy else {
        return (elevated, None);
    };
    let mut stored = TeamAutonomy {
        elevations: Vec::new(),
        ..autonomy.clone()
    };
    if autonomy.mode != TeamAutonomyMode::Fixed {
        return (elevated, Some(stored));
    }
    if !autonomy.level.is_some_and(|level| (1..=4).contains(&level)) {
        issues.push(ValidationIssue::new(
            "autonomy.level",
            "a fixed team autonomy needs a level from 1 to 4",
        ));
        return (elevated, Some(stored));
    }
    let mut seen = std::collections::HashSet::new();
    for step in steps {
        if !seen.insert(step.agent_id) {
            continue;
        }
        let spec = match state.store.get_agent(auth.org_id, step.agent_id).await {
            Ok(agent) => match state.store.get_current_version(auth.org_id, &agent).await {
                Ok(version) => serde_json::from_value::<orbit_domain::AgentSpec>(version.spec)
                    .map(|spec| (agent, spec))
                    .map_err(|error| error.to_string()),
                Err(error) => Err(error.to_string()),
            },
            Err(error) => Err(error.to_string()),
        };
        let (agent, spec) = match spec {
            Ok(found) => found,
            Err(error) => {
                tracing::warn!(agent_id = %step.agent_id, %error, "could not read step agent autonomy");
                issues.push(ValidationIssue::new(
                    "autonomy",
                    format!(
                        "could not check the autonomy of step agent '{}'; the team was not saved",
                        step.agent_id
                    ),
                ));
                continue;
            }
        };
        let own = orbit_domain::AutonomyPolicy::of(&spec.autonomy);
        if !autonomy.elevates(own) {
            continue;
        }
        if !access.can_manage(agent.owner_user_id, agent.owner_team_id) {
            let team = autonomy.policy_for(own).unwrap_or(own);
            issues.push(ValidationIssue::new(
                if team.level > own.level {
                    "autonomy.level"
                } else {
                    "autonomy.approvalMode"
                },
                format!(
                    "running agent '{}' at level {} with approval mode {} is less strict than its own policy (level {}, approval mode {}) and requires manage access to that agent",
                    agent.name,
                    team.level,
                    approval_mode_label(team.approval_mode),
                    own.level,
                    approval_mode_label(own.approval_mode),
                ),
            ));
            continue;
        }
        stored.elevations.push(orbit_domain::TeamAutonomyElevation {
            agent_id: agent.id,
            baseline: own,
        });
        elevated.push(AutonomyElevation {
            agent_id: agent.id,
            name: agent.name.clone(),
            baseline: own,
        });
    }
    (elevated, Some(stored))
}

fn approval_mode_label(mode: orbit_domain::ApprovalMode) -> &'static str {
    match mode {
        orbit_domain::ApprovalMode::Auto => "auto",
        orbit_domain::ApprovalMode::Writes => "writes",
        orbit_domain::ApprovalMode::Always => "always",
    }
}

async fn audit_team_autonomy_elevations(
    state: &AppState,
    auth: &AuthContext,
    team_id: Uuid,
    elevated: &[AutonomyElevation],
    autonomy: Option<&TeamAutonomy>,
) {
    if elevated.is_empty() {
        return;
    }
    crate::audit::record(
        state,
        auth,
        "team.autonomy_elevated",
        Some("agent_team"),
        Some(team_id),
        serde_json::json!({
            "level": autonomy.and_then(|autonomy| autonomy.level),
            "approvalMode": autonomy.and_then(|autonomy| autonomy.approval_mode),
            "agents": elevated
                .iter()
                .map(|elevation| serde_json::json!({
                    "id": elevation.agent_id,
                    "name": elevation.name,
                    "from": elevation.baseline.level,
                    "fromApprovalMode": elevation.baseline.approval_mode,
                }))
                .collect::<Vec<_>>(),
        }),
    )
    .await;
}

pub(crate) async fn require_team_view(
    state: &AppState,
    auth: &AuthContext,
    team_id: Uuid,
) -> Result<orbit_store::AgentTeam, ApiError> {
    let team = state.store.get_agent_team(auth.org_id, team_id).await?;
    let access = crate::access::AccessScope::load(state, auth).await?;
    access.require_view(
        "agent team",
        team.owner_user_id,
        team.owner_team_id,
        &team.visibility,
    )?;
    Ok(team)
}

pub(crate) async fn require_team_manage(
    state: &AppState,
    auth: &AuthContext,
    team_id: Uuid,
) -> Result<orbit_store::AgentTeam, ApiError> {
    let team = state.store.get_agent_team(auth.org_id, team_id).await?;
    let access = crate::access::AccessScope::load(state, auth).await?;
    access.require_manage(
        "agent team",
        team.owner_user_id,
        team.owner_team_id,
        &team.visibility,
    )?;
    Ok(team)
}

/// Require use on a team (view plus run). Used by the queue, whose items execute the
/// team's agents and connections.
pub(crate) async fn require_team_use(
    state: &AppState,
    auth: &AuthContext,
    team_id: Uuid,
) -> Result<orbit_store::AgentTeam, ApiError> {
    let team = state.store.get_agent_team(auth.org_id, team_id).await?;
    let access = crate::access::AccessScope::load(state, auth).await?;
    access.require_use(
        "agent team",
        team.owner_user_id,
        team.owner_team_id,
        &team.visibility,
    )?;
    Ok(team)
}

#[utoipa::path(post, path = "/api/v1/agent-teams", tag = "agent-teams", request_body = CreateTeamRequest, responses((status = 201, body = orbit_store::AgentTeam)))]
pub async fn create_team(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    crate::extract::ApiJson(mut body): crate::extract::ApiJson<CreateTeamRequest>,
) -> Result<(StatusCode, Json<AgentTeam>), ApiError> {
    auth.require(Permission::SettingsWrite)?;
    prepare_mode(&mut body)?;
    // Only the fields that are actually wrong (#438): a team with a name but no steps used
    // to be told "name: name is required" as well.
    let mut required = Vec::new();
    if body.name.trim().is_empty() {
        required.push(ValidationIssue::new("name", "name is required"));
    }
    if body.steps.is_empty() {
        required.push(ValidationIssue::new(
            "steps",
            "at least one step is required",
        ));
    }
    if !required.is_empty() {
        return Err(ApiError::validation(required));
    }
    if body
        .timeout_seconds
        .is_some_and(|seconds| !(1..=86_400).contains(&seconds))
    {
        return Err(ApiError::validation(vec![ValidationIssue::new(
            "timeoutSeconds",
            "must be 1..=86400",
        )]));
    }
    if let Some(repository) = body
        .repository
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty())
    {
        if !repository.contains('/') || repository.chars().any(char::is_whitespace) {
            return Err(ApiError::validation(vec![ValidationIssue::new(
                "repository",
                "repository must be owner/repo",
            )]));
        }
    }
    let mut issues = Vec::new();
    if let Some(source) = &body.work_source {
        issues.extend(validate_work_source(source));
    }
    if !body.repositories.is_empty() {
        issues.extend(validate_repositories(&body.repositories));
    }
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    let mut agent_ids = std::collections::BTreeSet::new();
    for (index, step) in body.steps.iter().enumerate() {
        if !agent_ids.insert(step.agent_id) {
            for path in step_agent_paths(&body, index, step.agent_id) {
                issues.push(ValidationIssue::new(
                    path,
                    "the same agent cannot appear twice in one team",
                ));
            }
        }
        // An agent the caller cannot view is reported exactly like a missing one.
        let viewable = match state.store.get_agent(auth.org_id, step.agent_id).await {
            Ok(agent) => {
                access.can_view(agent.owner_user_id, agent.owner_team_id, &agent.visibility)
            }
            Err(_) => false,
        };
        if !viewable {
            for path in step_agent_paths(&body, index, step.agent_id) {
                issues.push(ValidationIssue::new(
                    path,
                    format!("unknown agent '{}'", step.agent_id),
                ));
            }
        }
    }
    if let Some(source) = &body.work_source {
        validate_work_source_connection(&state, auth.org_id, &access, source, &mut issues).await;
    }
    if let Some(profile_id) = body.worker_profile_id {
        let viewable = match state
            .store
            .get_worker_profile(auth.org_id, profile_id)
            .await
        {
            Ok(profile) => access.can_view(
                profile.owner_user_id,
                profile.owner_team_id,
                &profile.visibility,
            ),
            Err(_) => false,
        };
        if !viewable {
            issues.push(ValidationIssue::new(
                "workerProfileId",
                format!("unknown worker profile '{profile_id}'"),
            ));
        }
    }
    issues.extend(
        crate::preflight::unusable_skill_issues(
            &state,
            auth.org_id,
            &access,
            &body.skills,
            "skills",
            false,
        )
        .await?,
    );
    for connection_id in &body.connection_ids {
        match state
            .store
            .get_connection(auth.org_id, *connection_id)
            .await
        {
            Ok(connection)
                if access.can_view(
                    connection.owner_user_id,
                    connection.owner_team_id,
                    &connection.visibility,
                ) => {}
            Ok(_) => issues.push(ValidationIssue::new(
                "connectionIds",
                format!("connection '{connection_id}' is not visible to you"),
            )),
            Err(_) => issues.push(ValidationIssue::new(
                "connectionIds",
                format!("unknown connection '{connection_id}'"),
            )),
        }
    }
    for mapping_id in &body.env_mapping_ids {
        let viewable = match state.store.get_env_mapping(auth.org_id, *mapping_id).await {
            Ok(mapping) => access.can_view(
                mapping.owner_user_id,
                mapping.owner_team_id,
                &mapping.visibility,
            ),
            Err(_) => false,
        };
        if !viewable {
            issues.push(ValidationIssue::new(
                "envMappingIds",
                format!("unknown environment mapping '{mapping_id}'"),
            ));
        }
    }
    validate_native_primary(&state, auth.org_id, &body.mode, &body.steps, &mut issues).await;
    let (elevations, autonomy) = validate_team_autonomy(
        &state,
        &auth,
        &access,
        body.autonomy.as_ref(),
        &body.steps,
        &mut issues,
    )
    .await;
    if !issues.is_empty() {
        return Err(ApiError::validation(issues));
    }
    let definition = serde_json::json!({
        "mode": body.mode,
        "instructions": body.instructions.trim(),
        "repository": body.repository.as_deref().map(str::trim).filter(|value| !value.is_empty()),
        "branch": body.branch.as_deref().map(str::trim).filter(|value| !value.is_empty()),
        "workSource": body.work_source.as_ref().map(work_source_to_json),
        "repositories": repositories_to_json(&body.repositories),
        "model": body.model,
        "workerProfileId": body.worker_profile_id,
        "autoLaunch": body.auto_launch,
        "skills": body.skills,
        "harness": body.harness,
        "connectionIds": body.connection_ids,
        "envMappingIds": body.env_mapping_ids,
        "timeoutSeconds": body.timeout_seconds,
        "autonomy": autonomy,
        "steps": body.steps.iter().map(|step| serde_json::json!({
            "agentId": step.agent_id,
            "model": step.model,
        })).collect::<Vec<_>>(),
        "workflow": body.workflow,
    });
    let team = state
        .store
        .create_agent_team(
            auth.org_id,
            body.name.trim(),
            body.description.trim(),
            &definition,
            Some(auth.user_id),
        )
        .await?;
    state
        .store
        .assign_resource_owner(auth.org_id, "agent_teams", team.id, auth.user_id)
        .await?;
    audit_team_autonomy_elevations(&state, &auth, team.id, &elevations, autonomy.as_ref()).await;
    crate::audit::record(
        &state,
        &auth,
        "team.created",
        Some("agent_team"),
        Some(team.id),
        serde_json::json!({ "name": team.name, "mode": body.mode }),
    )
    .await;
    Ok((StatusCode::CREATED, Json(team)))
}

/// Partial update: fields left out keep their stored value, an explicit `null` clears an
/// optional field. Send `expectedUpdatedAt` (the team's `updatedAt` as read) to refuse a
/// write over someone else's change with 409.
#[utoipa::path(
    patch, path = "/api/v1/agent-teams/{team_id}", tag = "agent-teams",
    request_body = CreateTeamRequest,
    responses(
        (status = 200, body = orbit_store::AgentTeam),
        (status = 409, description = "the team changed after `expectedUpdatedAt`"),
    )
)]
pub async fn update_team(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(team_id): Path<Uuid>,
    crate::extract::ApiJson(patch): crate::extract::ApiJson<serde_json::Value>,
) -> Result<Json<AgentTeam>, ApiError> {
    auth.require(Permission::SettingsWrite)?;
    let stored = require_team_manage(&state, &auth, team_id).await?;
    let (body, expected_updated_at) = merge_team_patch(&stored, patch)?;
    replace_team(&state, &auth, team_id, body, expected_updated_at)
        .await
        .map(Json)
}

/// Validate `body` and store it as the team's whole definition. Shared by PATCH (after
/// merging the patch onto the stored team) and portable import (a full replacement).
pub(crate) async fn replace_team(
    state: &AppState,
    auth: &AuthContext,
    team_id: Uuid,
    mut body: CreateTeamRequest,
    expected_updated_at: Option<chrono::DateTime<chrono::Utc>>,
) -> Result<AgentTeam, ApiError> {
    auth.require(Permission::SettingsWrite)?;
    let stored = require_team_manage(state, auth, team_id).await?;
    let expected_updated_at = expected_updated_at.or(body.expected_updated_at);
    prepare_mode(&mut body)?;
    // Only the fields that are actually wrong (#438): a team with a name but no steps used
    // to be told "name: name is required" as well.
    let mut required = Vec::new();
    if body.name.trim().is_empty() {
        required.push(ValidationIssue::new("name", "name is required"));
    }
    if body.steps.is_empty() {
        required.push(ValidationIssue::new(
            "steps",
            "at least one step is required",
        ));
    }
    if !required.is_empty() {
        return Err(ApiError::validation(required));
    }
    if body
        .timeout_seconds
        .is_some_and(|seconds| !(1..=86_400).contains(&seconds))
    {
        return Err(ApiError::validation(vec![ValidationIssue::new(
            "timeoutSeconds",
            "must be 1..=86400",
        )]));
    }
    if let Some(repository) = body
        .repository
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty())
    {
        if !repository.contains('/') || repository.chars().any(char::is_whitespace) {
            return Err(ApiError::validation(vec![ValidationIssue::new(
                "repository",
                "repository must be owner/repo",
            )]));
        }
    }
    let mut issues = Vec::new();
    if let Some(source) = &body.work_source {
        issues.extend(validate_work_source(source));
    }
    if !body.repositories.is_empty() {
        issues.extend(validate_repositories(&body.repositories));
    }
    let access = crate::access::AccessScope::load(state, auth).await?;
    let mut agent_ids = std::collections::BTreeSet::new();
    for (index, step) in body.steps.iter().enumerate() {
        if !agent_ids.insert(step.agent_id) {
            for path in step_agent_paths(&body, index, step.agent_id) {
                issues.push(ValidationIssue::new(
                    path,
                    "the same agent cannot appear twice in one team",
                ));
            }
        }
        // An agent the caller cannot view is reported exactly like a missing one.
        let viewable = match state.store.get_agent(auth.org_id, step.agent_id).await {
            Ok(agent) => {
                access.can_view(agent.owner_user_id, agent.owner_team_id, &agent.visibility)
            }
            Err(_) => false,
        };
        if !viewable {
            for path in step_agent_paths(&body, index, step.agent_id) {
                issues.push(ValidationIssue::new(
                    path,
                    format!("unknown agent '{}'", step.agent_id),
                ));
            }
        }
    }
    if let Some(source) = &body.work_source {
        validate_work_source_connection(state, auth.org_id, &access, source, &mut issues).await;
    }
    if let Some(profile_id) = body.worker_profile_id {
        let viewable = match state
            .store
            .get_worker_profile(auth.org_id, profile_id)
            .await
        {
            Ok(profile) => access.can_view(
                profile.owner_user_id,
                profile.owner_team_id,
                &profile.visibility,
            ),
            Err(_) => false,
        };
        if !viewable {
            issues.push(ValidationIssue::new(
                "workerProfileId",
                format!("unknown worker profile '{profile_id}'"),
            ));
        }
    }
    issues.extend(
        crate::preflight::unusable_skill_issues(
            state,
            auth.org_id,
            &access,
            &body.skills,
            "skills",
            false,
        )
        .await?,
    );
    for connection_id in &body.connection_ids {
        match state
            .store
            .get_connection(auth.org_id, *connection_id)
            .await
        {
            Ok(connection)
                if access.can_view(
                    connection.owner_user_id,
                    connection.owner_team_id,
                    &connection.visibility,
                ) => {}
            Ok(_) => issues.push(ValidationIssue::new(
                "connectionIds",
                format!("connection '{connection_id}' is not visible to you"),
            )),
            Err(_) => issues.push(ValidationIssue::new(
                "connectionIds",
                format!("unknown connection '{connection_id}'"),
            )),
        }
    }
    for mapping_id in &body.env_mapping_ids {
        let viewable = match state.store.get_env_mapping(auth.org_id, *mapping_id).await {
            Ok(mapping) => access.can_view(
                mapping.owner_user_id,
                mapping.owner_team_id,
                &mapping.visibility,
            ),
            Err(_) => false,
        };
        if !viewable {
            issues.push(ValidationIssue::new(
                "envMappingIds",
                format!("unknown environment mapping '{mapping_id}'"),
            ));
        }
    }
    validate_native_primary(state, auth.org_id, &body.mode, &body.steps, &mut issues).await;
    let (elevations, autonomy) = validate_team_autonomy(
        state,
        auth,
        &access,
        body.autonomy.as_ref(),
        &body.steps,
        &mut issues,
    )
    .await;
    if !issues.is_empty() {
        return Err(ApiError::validation(issues));
    }
    let definition = serde_json::json!({
        "mode": body.mode,
        "instructions": body.instructions.trim(),
        "repository": body.repository.as_deref().map(str::trim).filter(|value| !value.is_empty()),
        "branch": body.branch.as_deref().map(str::trim).filter(|value| !value.is_empty()),
        "workSource": body.work_source.as_ref().map(work_source_to_json),
        "repositories": repositories_to_json(&body.repositories),
        "model": body.model,
        "workerProfileId": body.worker_profile_id,
        "autoLaunch": body.auto_launch,
        "skills": body.skills,
        "harness": body.harness,
        "connectionIds": body.connection_ids,
        "envMappingIds": body.env_mapping_ids,
        "timeoutSeconds": body.timeout_seconds,
        "autonomy": autonomy,
        "steps": body.steps.iter().map(|step| serde_json::json!({
            "agentId": step.agent_id,
            "model": step.model,
        })).collect::<Vec<_>>(),
        "workflow": body.workflow,
    });
    let team = state
        .store
        .update_agent_team(
            auth.org_id,
            team_id,
            body.name.trim(),
            body.description.trim(),
            &definition,
            expected_updated_at,
        )
        .await?;
    audit_team_autonomy_elevations(state, auth, team.id, &elevations, autonomy.as_ref()).await;
    crate::audit::record(
        state,
        auth,
        "team.updated",
        Some("agent_team"),
        Some(team.id),
        serde_json::json!({ "name": team.name, "changed": changed_keys(&stored, &team) }),
    )
    .await;
    Ok(team)
}

/// The stored team as a request, with the fields of `patch` laid over it (#432). Before,
/// PATCH replaced the whole team, so a client that left out `autonomy`,
/// `connectionIds` or `workSource` silently cleared them.
fn merge_team_patch(
    stored: &AgentTeam,
    patch: serde_json::Value,
) -> Result<(CreateTeamRequest, Option<chrono::DateTime<chrono::Utc>>), ApiError> {
    let serde_json::Value::Object(mut patch) = patch else {
        return Err(ApiError::validation(vec![ValidationIssue::new(
            "body",
            "the request body must be a JSON object",
        )]));
    };
    let expected_updated_at = match patch.remove("expectedUpdatedAt") {
        None | Some(serde_json::Value::Null) => None,
        Some(value) => Some(serde_json::from_value(value).map_err(|_| {
            ApiError::validation(vec![ValidationIssue::new(
                "expectedUpdatedAt",
                "must be an RFC 3339 timestamp, the team's updatedAt",
            )])
        })?),
    };
    let mut merged = match &stored.definition {
        serde_json::Value::Object(map) => map.clone(),
        _ => serde_json::Map::new(),
    };
    merged.insert("name".into(), serde_json::json!(stored.name));
    merged.insert("description".into(), serde_json::json!(stored.description));
    for (key, value) in patch {
        merged.insert(key, value);
    }
    let body = serde_json::from_value(serde_json::Value::Object(merged)).map_err(|error| {
        ApiError::validation(vec![ValidationIssue::new("body", error.to_string())])
    })?;
    Ok((body, expected_updated_at))
}

/// Top-level team fields whose stored value changed, for the audit entry.
fn changed_keys(before: &AgentTeam, after: &AgentTeam) -> Vec<String> {
    let mut keys = Vec::new();
    if before.name != after.name {
        keys.push("name".to_string());
    }
    if before.description != after.description {
        keys.push("description".to_string());
    }
    let empty = serde_json::Map::new();
    let old = before.definition.as_object().unwrap_or(&empty);
    let new = after.definition.as_object().unwrap_or(&empty);
    let mut names: std::collections::BTreeSet<&String> = old.keys().collect();
    names.extend(new.keys());
    for name in names {
        if old.get(name) != new.get(name) {
            keys.push(name.clone());
        }
    }
    keys
}

#[utoipa::path(get, path = "/api/v1/agent-teams", tag = "agent-teams", responses((status = 200, body = [orbit_store::AgentTeam])))]
pub async fn list_teams(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> Result<Json<Vec<AgentTeam>>, ApiError> {
    auth.require(Permission::HarnessesRead)?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    Ok(Json(
        state
            .store
            .list_agent_teams(auth.org_id, Some(&access.store_access()))
            .await?,
    ))
}

#[utoipa::path(get, path = "/api/v1/agent-teams/{team_id}", tag = "agent-teams", responses((status = 200, body = orbit_store::AgentTeam)))]
pub async fn get_team(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(team_id): Path<Uuid>,
) -> Result<Json<AgentTeam>, ApiError> {
    auth.require(Permission::HarnessesRead)?;
    Ok(Json(require_team_view(&state, &auth, team_id).await?))
}

#[utoipa::path(delete, path = "/api/v1/agent-teams/{team_id}", tag = "agent-teams", responses((status = 204)))]
pub async fn delete_team(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(team_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::SettingsWrite)?;
    require_team_manage(&state, &auth, team_id).await?;
    // Deleting the team detaches its executions (`team_run_id` is ON DELETE SET NULL),
    // so ask live ones to stop first; nothing would stop them afterwards (#299).
    let cancelled = state
        .store
        .request_team_cancel(auth.org_id, team_id)
        .await?;
    announce_cancel(&state, auth.org_id, &cancelled).await;
    let team = state.store.get_agent_team(auth.org_id, team_id).await?;
    state.store.delete_agent_team(auth.org_id, team_id).await?;
    crate::audit::record(
        &state,
        &auth,
        "team.deleted",
        Some("agent_team"),
        Some(team_id),
        serde_json::json!({ "name": team.name, "cancelledExecutions": cancelled.len() }),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

/// Tell workers about executions whose cancellation was just requested. Workers also
/// poll the cancel flag, so a lost bus message only delays the stop.
pub(crate) async fn announce_cancel(
    state: &AppState,
    org_id: Uuid,
    executions: &[orbit_store::ExecutionRecord],
) {
    for execution in executions {
        let event = orbit_contracts::OrbitEvent::new(
            orbit_contracts::subjects::EXECUTION_CANCEL,
            "orbit-api",
            serde_json::json!({ "executionId": execution.id }),
        )
        .with_org(org_id);
        if let Err(error) = state.bus.publish(event).await {
            tracing::warn!(%error, execution_id = %execution.id, "cancel event publish failed; worker polls the cancel flag");
        }
    }
}

/// Stop a team run: request cancellation of its live executions, mark the run
/// `cancelled` and cancel the queue item it was started for, if any.
#[utoipa::path(post, path = "/api/v1/team-runs/{run_id}/cancel", tag = "agent-teams", responses((status = 200, body = TeamRunDetail), (status = 409, description = "the run already ended")))]
pub async fn cancel_run(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(run_id): Path<Uuid>,
) -> Result<Json<TeamRunDetail>, ApiError> {
    auth.require(Permission::ExecutionsCancel)?;
    let run = state.store.get_team_run(auth.org_id, run_id).await?;
    require_team_use(&state, &auth, run.team_id).await?;
    if run.status != "running" {
        return Err(ApiError::conflict("the team run already ended"));
    }
    let cancelled = state
        .store
        .request_team_run_cancel(auth.org_id, run_id)
        .await?;
    announce_cancel(&state, auth.org_id, &cancelled).await;
    let run = state
        .store
        .finish_team_run(run_id, "cancelled", None, Some("cancelled by operator"))
        .await?
        .ok_or_else(|| ApiError::conflict("the team run already ended"))?;
    // The run is over; its steps must not keep saying they are running.
    state
        .store
        .close_running_team_steps(run_id, "cancelled", None)
        .await?;
    if let Some(item) = state.store.get_team_queue_item_by_run(run_id).await? {
        if item.status == "running" {
            state
                .store
                .cancel_team_queue_item(auth.org_id, item.id, Some("team run cancelled"))
                .await?;
        }
    }
    crate::audit::record(
        &state,
        &auth,
        "team_run.cancelled",
        Some("team_run"),
        Some(run_id),
        serde_json::json!({ "executions": cancelled.iter().map(|e| e.id).collect::<Vec<_>>() }),
    )
    .await;
    let steps = state.store.list_team_steps(run.id).await?;
    Ok(Json(TeamRunDetail { run, steps }))
}

/// A subagent's system prompt: its instructions followed by its constraints. Never stored
/// in the run: execution and step input are readable by anyone who can view the team or
/// the primary agent, and a subagent may be private (#429). The worker context adds it
/// when a worker claims the step.
pub(crate) fn subagent_instructions(spec: &orbit_domain::AgentSpec) -> String {
    let mut instructions = spec.instructions.system.clone();
    if !spec.instructions.constraints.is_empty() {
        instructions.push_str("\n\nConstraints:\n");
        for constraint in &spec.instructions.constraints {
            instructions.push_str(&format!("- {constraint}\n"));
        }
    }
    instructions
}

/// The native subagents of a team, as references: id, name, description and model, but
/// no instructions (see [`subagent_instructions`]).
async fn native_subagents(
    state: &AppState,
    org_id: Uuid,
    definition: &TeamDefinition,
) -> Result<Vec<serde_json::Value>, ApiError> {
    let mut subagents = Vec::new();
    for (index, step) in definition.steps.iter().enumerate().skip(1) {
        let agent = state.store.get_agent(org_id, step.agent_id).await?;
        let version = state.store.get_current_version(org_id, &agent).await?;
        let mut spec: orbit_domain::AgentSpec = serde_json::from_value(version.spec.clone())
            .map_err(|error| {
                ApiError::internal(format!("stored agent spec is invalid: {error}"))
            })?;
        let model_override = definition.model_override(index).cloned();
        if let Some(override_model) = &model_override {
            override_model.apply(&mut spec.model);
        }
        let model = match (&spec.model.provider, &spec.model.model) {
            (Some(provider), Some(model)) if !model.contains('/') => {
                Some(format!("{provider}/{model}"))
            }
            (_, Some(model)) => Some(model.clone()),
            _ => None,
        };
        subagents.push(serde_json::json!({
            "agentId": agent.id,
            "name": agent.name,
            "slug": agent.slug,
            "description": agent.description,
            "model": model,
            "modelOverride": model_override,
        }));
    }
    Ok(subagents)
}

/// The workflow step a team step runs, for [`start_step_with`].
pub(crate) struct WorkflowStepStart<'a> {
    pub node: &'a WorkflowNode,
    /// 1 for the first time this workflow step runs in the run.
    pub attempt: u32,
    /// The deliveries this start takes (`[{ edge, step }]`).
    pub activated_by: serde_json::Value,
}

pub(crate) struct StepStart<'a> {
    pub step_index: usize,
    pub agent_id: Uuid,
    pub model_override: Option<TeamModelOverride>,
    pub previous_output: Option<&'a serde_json::Value>,
    pub workflow: Option<WorkflowStepStart<'a>>,
}

/// Start step `step_index` of a pipeline or native run.
/// A step agent whose current policy is stricter than the team's authorised elevation, so
/// the step runs with the stricter policy (#385).
struct AutonomyClamp {
    agent_id: Uuid,
    agent_name: String,
    requested: orbit_domain::AutonomyPolicy,
    agent: orbit_domain::AutonomyPolicy,
    applied: orbit_domain::AutonomyPolicy,
}

impl AutonomyClamp {
    fn message(&self) -> String {
        format!(
            "team autonomy limited for agent '{}': the team asks for level {} with approval mode {}, \
             but the agent's own policy (level {}, approval mode {}) is stricter than when the team was \
             authorised; running at level {} with approval mode {}. Re-save the team with manage \
             access to the agent to authorise it again.",
            self.agent_name,
            self.requested.level,
            approval_mode_label(self.requested.approval_mode),
            self.agent.level,
            approval_mode_label(self.agent.approval_mode),
            self.applied.level,
            approval_mode_label(self.applied.approval_mode),
        )
    }

    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "agentId": self.agent_id,
            "agentName": self.agent_name,
            "requested": self.requested,
            "agent": self.agent,
            "applied": self.applied,
        })
    }
}

/// The fixed team policy a step execution carries, resolved against the **current** policy
/// of every agent it governs: the step agent, and for a native team every subagent too,
/// because subagents inherit the primary execution's policy. `None` when the team does not
/// fix autonomy. An agent whose policy cannot be read fails the step.
async fn effective_team_autonomy(
    state: &AppState,
    run: &TeamRun,
    definition: &TeamDefinition,
    agent_id: Uuid,
    spec: &orbit_domain::AgentSpec,
    native: bool,
) -> Result<Option<(TeamAutonomy, Vec<AutonomyClamp>)>, ApiError> {
    let Some(autonomy) = definition.autonomy.as_ref() else {
        return Ok(None);
    };
    if autonomy.mode != TeamAutonomyMode::Fixed {
        return Ok(None);
    }
    let mut governed = vec![(
        agent_id,
        None,
        orbit_domain::AutonomyPolicy::of(&spec.autonomy),
    )];
    if native {
        for step in &definition.steps {
            if governed.iter().any(|(id, _, _)| *id == step.agent_id) {
                continue;
            }
            let agent = state.store.get_agent(run.org_id, step.agent_id).await?;
            let version = state.store.get_current_version(run.org_id, &agent).await?;
            let sub: orbit_domain::AgentSpec =
                serde_json::from_value(version.spec).map_err(|error| {
                    ApiError::internal(format!("stored agent spec is invalid: {error}"))
                })?;
            governed.push((
                agent.id,
                Some(agent.name),
                orbit_domain::AutonomyPolicy::of(&sub.autonomy),
            ));
        }
    }
    let mut clamps = Vec::new();
    let mut combined: Option<orbit_domain::AutonomyPolicy> = None;
    for (id, name, own) in governed {
        let applied = match orbit_domain::resolve_team_autonomy(autonomy, id, own) {
            orbit_domain::TeamAutonomyResolution::NotFixed => return Ok(None),
            orbit_domain::TeamAutonomyResolution::Applies(team) => {
                team.policy_for(own).unwrap_or(own)
            }
            orbit_domain::TeamAutonomyResolution::Clamped {
                autonomy: clamped,
                requested,
                agent,
            } => {
                let applied = clamped.policy_for(agent).unwrap_or(agent);
                clamps.push(AutonomyClamp {
                    agent_id: id,
                    agent_name: match name {
                        Some(name) => name,
                        None => state.store.get_agent(run.org_id, id).await?.name,
                    },
                    requested,
                    agent,
                    applied,
                });
                applied
            }
        };
        combined = Some(match combined {
            Some(policy) => policy.stricter_of(&applied),
            None => applied,
        });
    }
    if clamps.is_empty() {
        return Ok(Some((autonomy.snapshot(), clamps)));
    }
    let policy = combined.expect("at least the step agent is governed");
    Ok(Some((
        TeamAutonomy {
            mode: TeamAutonomyMode::Fixed,
            level: Some(policy.level),
            approval_mode: Some(policy.approval_mode),
            elevations: Vec::new(),
        },
        clamps,
    )))
}

pub(crate) async fn start_step(
    state: &AppState,
    run: &TeamRun,
    definition: &TeamDefinition,
    step_index: usize,
    previous_output: Option<&serde_json::Value>,
) -> Result<TeamStep, ApiError> {
    let step = definition
        .steps
        .get(step_index)
        .ok_or_else(|| ApiError::internal("team step index out of range"))?;
    start_step_with(
        state,
        run,
        definition,
        StepStart {
            step_index,
            agent_id: step.agent_id,
            model_override: definition.model_override(step_index).cloned(),
            previous_output,
            workflow: None,
        },
    )
    .await
}

/// The first steps of a new run.
pub(crate) struct FirstSteps {
    /// Every step the run has now.
    pub steps: Vec<TeamStep>,
    /// The steps this call created. The caller launches workers for these only; steps
    /// the background loop created were launched by the loop.
    pub started: Vec<TeamStep>,
}

/// Start the first step of a new run: step 0 of a pipeline or native team, or the
/// starting steps of a workflow.
pub(crate) async fn start_first_steps(
    state: &AppState,
    run: &TeamRun,
    definition: &TeamDefinition,
) -> Result<FirstSteps, ApiError> {
    if let Some(workflow) = definition.workflow() {
        return crate::team_workflow::start_run(state, run, definition, workflow).await;
    }
    let step = start_step(state, run, definition, 0, None).await?;
    Ok(FirstSteps {
        steps: vec![step.clone()],
        started: vec![step],
    })
}

/// Launch the team's worker profile for a step that was just created, when the team asks
/// for that. `auth` is the caller; background starts pass `None`.
pub(crate) async fn auto_launch_step(
    state: &AppState,
    run: &TeamRun,
    definition: &TeamDefinition,
    step: &TeamStep,
    auth: Option<&AuthContext>,
) -> Result<(), ApiError> {
    if !definition.auto_launch {
        return Ok(());
    }
    let (Some(profile_id), Some(execution_id)) = (definition.worker_profile_id, step.execution_id)
    else {
        return Ok(());
    };
    let background;
    let auth = match auth {
        Some(auth) => auth,
        None => {
            // Later steps start in the background: launch as the run's requester, or the
            // team owner when the requester was deleted (`requested_by` is then NULL).
            // Never a nil user id: `worker_launches.created_by` is a foreign key (#383).
            let team = state.store.get_agent_team(run.org_id, run.team_id).await?;
            background = crate::preflight::background_identity(
                state,
                run.org_id,
                &[run.requested_by, team.owner_user_id],
            )
            .await?
            .ok_or_else(|| {
                ApiError::validation(vec![ValidationIssue::new(
                    "autoLaunch",
                    "auto-launch needs the run's requester or a team owner",
                )])
            })?;
            &background
        }
    };
    crate::handlers::worker_launches::launch_profile_internal(
        state,
        auth,
        profile_id,
        Some(execution_id),
    )
    .await?;
    Ok(())
}

pub(crate) async fn start_step_with(
    state: &AppState,
    run: &TeamRun,
    definition: &TeamDefinition,
    start: StepStart<'_>,
) -> Result<TeamStep, ApiError> {
    // Every team step creates an execution through `create_execution_with_limit`, so
    // manual runs, scheduled/queue starts and dispatcher auto-run share the same guard.
    let StepStart {
        step_index,
        agent_id,
        model_override,
        previous_output,
        workflow,
    } = start;
    let agent = state.store.get_agent(run.org_id, agent_id).await?;
    let version = state.store.get_current_version(run.org_id, &agent).await?;
    let mut spec: orbit_domain::AgentSpec = serde_json::from_value(version.spec.clone())
        .map_err(|error| ApiError::internal(format!("stored agent spec is invalid: {error}")))?;

    // Apply the step's (or the team's) model override. Preflight checks the provider of
    // the overridden model, and the override travels with the execution so the worker
    // runs that same model (#305).
    if let Some(override_model) = &model_override {
        override_model.apply(&mut spec.model);
    }
    // #259: the run instruction is the single task for every step. The team goal is
    // configured system context and the previous step output is delegated context;
    // neither is copied into the task the harness presents as user input.
    let task = run_instruction(&run.input).to_string();
    // A workflow step adds its own instructions and, when it has outcomes, how to report
    // one. Both are configuration, so they travel with the goal as system context.
    let instructions = match &workflow {
        Some(step) => crate::team_workflow::step_goal(configured_goal(&run.input), step.node),
        None => configured_goal(&run.input).to_string(),
    };
    let previous = previous_output
        .map(|value| value.to_string())
        .unwrap_or_else(|| "null".into());
    let requested_harness = run
        .input
        .get("payload")
        .and_then(|value| value.get("harnessOverride"))
        .and_then(|value| value.as_str())
        .or(definition.harness.as_deref());
    let harness_kind = match requested_harness {
        Some(value) => value.parse::<HarnessKind>().map_err(|_| {
            ApiError::validation(vec![ValidationIssue::new("harness", "unknown harness")])
        })?,
        None => match spec.harness.as_ref().map(|harness| harness.kind) {
            Some(kind) => kind,
            None => state.default_harness().await.ok_or_else(|| {
                ApiError::validation(vec![ValidationIssue::new(
                    "harness",
                    "no team harness, legacy agent harness or default harness is configured",
                )])
            })?,
        },
    };
    // Shared execution preflight (#140), re-evaluated for every step with the identity
    // that requested the run (falling back to the team owner).
    let team = state.store.get_agent_team(run.org_id, run.team_id).await?;
    let team_connections = team_connection_ids(definition);
    match crate::preflight::background_access(
        state,
        run.org_id,
        &[run.requested_by, team.owner_user_id],
    )
    .await?
    {
        Some(access) => {
            access.require_use(
                "agent team",
                team.owner_user_id,
                team.owner_team_id,
                &team.visibility,
            )?;
            crate::preflight::Preflight {
                state,
                org_id: run.org_id,
                access: &access,
                agent: &agent,
                spec: &spec,
                harness: harness_kind,
                extra_connection_ids: &team_connections,
            }
            .run()
            .await?;
        }
        None => {
            crate::preflight::require_harness_available(state, run.org_id, harness_kind).await?;
            crate::preflight::require_hourly_quota(state, run.org_id, agent.id, &spec).await?;
        }
    }
    let native = definition.mode == "native";
    if native && harness_kind != HarnessKind::OpenCode {
        return Err(ApiError::conflict(
            "native Agent Teams currently require an OpenCode primary agent",
        ));
    }
    let mut team_payload = serde_json::json!({
        "runId": run.id,
        "mode": definition.mode,
        // Configured team instructions, consumed by the worker as system context.
        "goal": instructions,
        "agent": agent.name,
        "stepIndex": step_index,
        // Delegated context for later steps; never part of the run instruction.
        "previous": previous,
        "connectionIds": definition.connection_ids,
        // Set server-side like connectionIds; the worker context applies it (#305).
        "modelOverride": model_override,
        "envMappingIds": definition.env_mapping_ids,
    });
    if definition.repositories.len() > 1 {
        let manifest = definition
            .repositories
            .iter()
            .map(|repository| {
                format!(
                    "- {} (workspace path: {})",
                    repository.repository,
                    if repository.path.is_empty() {
                        "."
                    } else {
                        &repository.path
                    }
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        team_payload["workspaceRepositories"] = serde_json::json!(manifest);
    }
    if let Some(step) = &workflow {
        team_payload["step"] = serde_json::json!({
            "id": step.node.id,
            "name": step.node.name,
            "attempt": step.attempt,
        });
    }
    if native {
        team_payload["subagents"] =
            serde_json::Value::Array(native_subagents(state, run.org_id, definition).await?);
    }
    let mut input = serde_json::json!({
        "task": task,
        "payload": {
            "teamRunId": run.id,
            "stepIndex": step_index,
            "agentName": agent.name,
            "stepName": workflow.as_ref().map(|step| step.node.name.as_str()),
            // Tells the agent (and the mock harness) which run of the step this is.
            "stepAttempt": workflow.as_ref().map(|step| step.attempt),
            "previous": previous_output,
            "repository": run.input.get("payload").and_then(|value| value.get("repository")).cloned(),
            "branch": run.input.get("payload").and_then(|value| value.get("branch")).cloned(),
            "repositories": run.input.get("payload").and_then(|value| value.get("repositories")).cloned(),
            "harnessOverride": run.input.get("payload").and_then(|value| value.get("harnessOverride")).cloned(),
        },
        "provenance": {
            "source": "team",
            "trusted": true,
            "subject": run.id,
            "receivedAt": chrono::Utc::now(),
        },
        "team": team_payload,
    });
    // #385: the fixed team policy is resolved against the step agents' current policies,
    // so an agent made stricter after the team was saved is never run less strictly.
    let mut autonomy_clamps = Vec::new();
    if let Some((autonomy, clamps)) =
        effective_team_autonomy(state, run, definition, agent.id, &spec, native).await?
    {
        input[orbit_domain::TEAM_AUTONOMY_KEY] =
            serde_json::to_value(&autonomy).unwrap_or(serde_json::Value::Null);
        if !clamps.is_empty() {
            input["teamAutonomyClamped"] =
                serde_json::Value::Array(clamps.iter().map(AutonomyClamp::to_json).collect());
        }
        autonomy_clamps = clamps;
    }
    // #194: set only here, server-side, after the run was authorised with approvals:decide.
    // The worker applies this before the fixed team policy, so a fixed team policy wins.
    if run
        .input
        .get("autoApprove")
        .and_then(|value| value.as_bool())
        == Some(true)
    {
        input[orbit_domain::APPROVAL_OVERRIDE_KEY] =
            serde_json::json!(orbit_domain::APPROVAL_OVERRIDE_AUTO);
    }
    // A workflow step is identified by what it runs, not by the order it was created in:
    // a retried start must find the same execution again.
    let idempotency_key = match &workflow {
        Some(step) => format!("team:{}:{}:{}", run.id, step.node.id, step.attempt),
        None => format!("team:{}:{}", run.id, step_index),
    };
    let replayed = !autonomy_clamps.is_empty()
        && state
            .store
            .get_execution_by_idempotency_key(run.org_id, &idempotency_key)
            .await?
            .is_some();
    let execution = state
        .create_execution_with_limit(orbit_store::executions::NewExecution {
            org_id: run.org_id,
            agent_id: agent.id,
            agent_version_id: version.id,
            agent_name: &agent.name,
            harness_kind,
            trigger_source: TriggerSource::Manual,
            placement: spec
                .harness
                .as_ref()
                .map(|harness| harness.placement.as_str())
                .unwrap_or("auto"),
            trigger_id: None,
            input,
            session_ref: None,
            skills: &definition.skills,
            timeout_seconds: definition
                .timeout_seconds
                .unwrap_or_else(|| spec.resources.timeout_seconds.max(3600)),
            requested_by: run.requested_by,
            idempotency_key: Some(&idempotency_key),
            team_run_id: Some(run.id),
            team_step_index: Some(step_index as i32),
        })
        .await?;
    if !replayed {
        for clamp in &autonomy_clamps {
            if let Err(error) = state
                .store
                .append_execution_event(
                    run.org_id,
                    execution.id,
                    "log",
                    orbit_domain::LogLevel::Warn,
                    &clamp.message(),
                    &clamp.to_json(),
                )
                .await
            {
                tracing::warn!(%error, execution_id = %execution.id, "could not record the team autonomy clamp");
            }
        }
    }
    if let Some(step) = &workflow {
        return Ok(state
            .store
            .create_workflow_step(orbit_store::agent_teams::NewWorkflowStep {
                run_id: run.id,
                org_id: run.org_id,
                step_index: step_index as i32,
                name: &step.node.name,
                agent_id: agent.id,
                agent_version_id: version.id,
                skills: &definition.skills,
                input: &execution.input,
                execution_id: Some(execution.id),
                node_id: &step.node.id,
                attempt: step.attempt as i32,
                activated_by: &step.activated_by,
            })
            .await?);
    }
    let created = state
        .store
        .create_team_step(
            run.id,
            run.org_id,
            step_index as i32,
            &agent.name,
            agent.id,
            version.id,
            &definition.skills,
            &execution.input,
            Some(execution.id),
        )
        .await?;
    Ok(created)
}

#[utoipa::path(post, path = "/api/v1/agent-teams/{team_id}/runs", tag = "agent-teams", request_body = CreateTeamRunRequest, responses((status = 201, body = TeamRunDetail)))]
pub async fn create_run(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(team_id): Path<Uuid>,
    crate::extract::ApiJson(body): crate::extract::ApiJson<CreateTeamRunRequest>,
) -> Result<(StatusCode, Json<TeamRunDetail>), ApiError> {
    auth.require(Permission::ExecutionsCreate)?;
    // #194: auto-approve skips human review, so it needs the right to decide approvals.
    if body.auto_approve && !auth.can(Permission::ApprovalsDecide) {
        return Err(ApiError::forbidden(
            "autoApprove requires the approvals:decide permission",
        ));
    }
    // executions_per_day is enforced in start_step, which creates the step execution for
    // manual, queued and dispatcher auto-run starts alike.
    // Running a team needs use (not just view) on the team, its step agents' connections
    // and providers, and the team's shared connections (#140).
    let team = state.store.get_agent_team(auth.org_id, team_id).await?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    access.require_view(
        "agent team",
        team.owner_user_id,
        team.owner_team_id,
        &team.visibility,
    )?;
    let definition = parse_definition(&team)?;
    let step_agent_ids: Vec<Uuid> = definition.steps.iter().map(|step| step.agent_id).collect();
    crate::preflight::authorize_team_run(
        &state,
        auth.org_id,
        &access,
        &team,
        &step_agent_ids,
        &team_connection_ids(&definition),
    )
    .await?;
    validate_requested_repositories(&definition, &body.payload)?;
    let requested_task = body.task.trim();
    let configured_instructions = definition.instructions.trim();
    if requested_task.is_empty() && configured_instructions.is_empty() {
        return Err(ApiError::validation(vec![ValidationIssue::new(
            "task",
            "task or team instructions are required",
        )]));
    }
    // If the caller supplied no task, the configured goal becomes the run instruction
    // and is not duplicated as system context.
    let (task, instructions) = if requested_task.is_empty() {
        (configured_instructions.to_string(), String::new())
    } else {
        (requested_task.to_string(), definition.instructions.clone())
    };
    let mut payload = body.payload.clone();
    if !payload.is_object() {
        payload = serde_json::json!({});
    }
    if payload
        .get("repository")
        .and_then(|value| value.as_str())
        .is_none()
    {
        if let Some(repository) = &definition.repository {
            payload["repository"] = serde_json::json!(repository);
        }
    }
    if payload
        .get("branch")
        .and_then(|value| value.as_str())
        .is_none()
    {
        if let Some(branch) = &definition.branch {
            payload["branch"] = serde_json::json!(branch);
        }
    }
    if !definition.repositories.is_empty() && payload.get("repositories").is_none() {
        payload["repositories"] = serde_json::json!(definition.repositories);
    }
    if let Some(harness_override) = &body.harness_override {
        payload["harnessOverride"] = serde_json::json!(harness_override);
    }
    let input = serde_json::json!({
        "task": task,
        "instructions": instructions,
        "payload": payload,
        "definition": definition,
        "autoApprove": body.auto_approve,
    });
    let run = state
        .store
        .create_team_run(auth.org_id, team.id, &input, Some(auth.user_id))
        .await?;
    crate::audit::record(
        &state,
        &auth,
        "team_run.started",
        Some("team_run"),
        Some(run.id),
        serde_json::json!({ "teamId": team.id, "autoApprove": body.auto_approve }),
    )
    .await;
    let started = async {
        let first = start_first_steps(&state, &run, &definition).await?;
        for step in &first.started {
            auto_launch_step(&state, &run, &definition, step, Some(&auth)).await?;
        }
        Ok::<_, ApiError>(first)
    }
    .await;
    let first = match started {
        Ok(first) => first,
        Err(error) => {
            // The run exists and a workflow may have started some of its parallel first
            // steps: end it before reporting the error, so a client that retries does not
            // leave a second run behind (#431). Ending the run closes its steps.
            if let Ok(cancelled) = state
                .store
                .request_team_run_cancel(run.org_id, run.id)
                .await
            {
                announce_cancel(&state, run.org_id, &cancelled).await;
            }
            let _ = state
                .store
                .update_team_run(run.id, "failed", None, Some(&error.message))
                .await;
            return Err(error);
        }
    };
    Ok((
        StatusCode::CREATED,
        Json(TeamRunDetail {
            run,
            steps: first.steps,
        }),
    ))
}

#[utoipa::path(get, path = "/api/v1/team-runs", tag = "agent-teams", params(TeamRunListQuery), responses((status = 200, body = TeamRunPage)))]
pub async fn list_runs(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<TeamRunListQuery>,
) -> Result<Json<TeamRunPage>, ApiError> {
    auth.require(Permission::HarnessesRead)?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    let (items, next_cursor, total) = state
        .store
        .list_team_runs(
            auth.org_id,
            query.archived,
            query.team_id,
            query.cursor.as_deref(),
            query.q.as_deref(),
            query.limit.unwrap_or(100).clamp(1, 200) as i64,
            Some(&access.store_access()),
        )
        .await?;
    Ok(Json(TeamRunPage {
        items,
        next_cursor,
        total,
    }))
}

#[utoipa::path(get, path = "/api/v1/team-runs/{run_id}", tag = "agent-teams", responses((status = 200, body = TeamRunDetail)))]
pub async fn get_run(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(run_id): Path<Uuid>,
) -> Result<Json<TeamRunDetail>, ApiError> {
    auth.require(Permission::HarnessesRead)?;
    let run = state.store.get_team_run(auth.org_id, run_id).await?;
    require_team_view(&state, &auth, run.team_id).await?;
    let steps = state.store.list_team_steps(run.id).await?;
    Ok(Json(TeamRunDetail { run, steps }))
}

/// How long a replica may hold the per-run advance lease before another may take over.
pub(crate) const ADVANCE_LEASE_SECS: i64 = 120;
/// How long a run whose next step cannot be started is retried before it is failed.
pub(crate) const START_STEP_RETRY_WINDOW_SECS: i64 = 300;
/// How long a step held back by an hourly quota or the licence's daily execution limit is
/// retried. Long enough for an hourly quota to clear (#305).
pub(crate) const LIMITED_STEP_RETRY_WINDOW_SECS: i64 = 3_900;

/// Limits that clear by themselves: the agent's `maxExecutionsPerHour` (429) and the
/// licence's rolling daily execution limit (402).
pub(crate) fn is_temporary_limit(error: &ApiError) -> bool {
    matches!(error.code, "rate_limited" | "license_limit")
}

/// Advance a running team after its current execution reaches a terminal state.
///
/// Safe to call concurrently from every API replica: a per-run lease makes all but one
/// caller a no-op, step transitions are conditional (`WHERE status = 'running'`) and
/// `team_steps` is unique on `(run_id, step_index)`.
///
/// The state machine is resumable. If a tick dies after finishing a step but before the
/// next one exists, the next tick sees "last step succeeded, run still running" and
/// continues from there; a run is never stranded in that state.
pub async fn advance(state: &AppState, run: TeamRun) -> anyhow::Result<()> {
    // Most ticks find the current step still working. Check that with two reads before
    // taking the lease, which costs two writes per run per tick on every replica (#305).
    // The claimed path re-reads everything, so a stale answer here only delays a tick.
    if !has_work(state, &run).await? {
        return Ok(());
    }
    let Some((run, lease)) = state
        .store
        .claim_team_run_advance(run.id, ADVANCE_LEASE_SECS)
        .await?
    else {
        return Ok(());
    };
    let result = advance_claimed(state, run.clone()).await;
    if let Err(error) = state.store.release_team_run_advance(run.id, lease).await {
        tracing::warn!(%error, run_id = %run.id, "failed to release team run advance lease");
    }
    result
}

/// Whether advancing `run` can change anything: false while its current step's execution
/// is still running (or there is no step yet).
async fn has_work(state: &AppState, run: &TeamRun) -> anyhow::Result<bool> {
    if let Some(definition) = run_definition(run) {
        if let Some(workflow) = definition.workflow() {
            return crate::team_workflow::has_work(state, run, workflow).await;
        }
    }
    let Some(current) = state.store.current_team_step(run.id).await? else {
        return Ok(false);
    };
    if current.status != "running" {
        return Ok(true);
    }
    let Some(execution_id) = current.execution_id else {
        return Ok(false);
    };
    let execution = state.store.get_execution_by_id(execution_id).await?;
    Ok(matches!(
        orbit_domain::ExecutionStatus::parse(&execution.status),
        Some(status) if status.is_terminal()
    ))
}

/// The definition a run was started with, when the run carries a readable snapshot.
fn run_definition(run: &TeamRun) -> Option<TeamDefinition> {
    serde_json::from_value(run.input.get("definition")?.clone()).ok()
}

async fn advance_claimed(state: &AppState, run: TeamRun) -> anyhow::Result<()> {
    // A step whose worker went away may have had side effects, so the run ends without
    // a retry and nothing runs after that step. The reaper and the launch poller apply
    // that through `fail_team_run_for_execution`, but this tick can see the failed
    // execution first, or the API may have restarted in between (#394). Decide from the
    // stored execution so whichever path gets here first ends the run the same way.
    if let Some(execution) = state
        .store
        .team_run_execution_failed_with(run.id, &ended_without_retry_codes())
        .await?
    {
        fail_team_run_for_execution(state, &execution).await?;
        if state.store.get_team_run(run.org_id, run.id).await?.status != "running" {
            return Ok(());
        }
    }
    if let Some(definition) = run_definition(&run) {
        if let Some(workflow) = definition.workflow() {
            return crate::team_workflow::advance(state, &run, &definition, workflow).await;
        }
    }
    let Some(current) = state.store.current_team_step(run.id).await? else {
        return Ok(());
    };
    let definition = match run.input.get("definition").cloned() {
        Some(value) => serde_json::from_value::<TeamDefinition>(value)
            .map_err(|error| anyhow::anyhow!("team run definition is invalid: {error}"))?,
        None => {
            let team = state.store.get_agent_team(run.org_id, run.team_id).await?;
            parse_definition(&team).map_err(|error| anyhow::anyhow!(error.message))?
        }
    };
    match current.status.as_str() {
        "running" => {
            let Some(execution_id) = current.execution_id else {
                return Ok(());
            };
            let execution = state.store.get_execution_by_id(execution_id).await?;
            if !matches!(
                orbit_domain::ExecutionStatus::parse(&execution.status),
                Some(status) if status.is_terminal()
            ) {
                return Ok(());
            }
            if execution.status == "succeeded" {
                let Some(finished) = state
                    .store
                    .finish_team_step(current.id, "succeeded", execution.output.as_ref(), None)
                    .await?
                else {
                    return Ok(());
                };
                continue_after_success(state, &run, &definition, &finished).await
            } else {
                let Some(finished) = state
                    .store
                    .finish_team_step(
                        current.id,
                        "failed",
                        execution.output.as_ref(),
                        Some(&execution_error_message(&execution)),
                    )
                    .await?
                else {
                    return Ok(());
                };
                fail_run_for_step(state, &run, &finished).await
            }
        }
        // Resume: the previous tick finished the step but did not get further.
        "succeeded" => continue_after_success(state, &run, &definition, &current).await,
        "failed" => fail_run_for_step(state, &run, &current).await,
        _ => Ok(()),
    }
}

async fn fail_run_for_step(state: &AppState, run: &TeamRun, step: &TeamStep) -> anyhow::Result<()> {
    let message = format!(
        "step '{}' finished with {}",
        step.name,
        step.error.as_deref().unwrap_or("failure")
    );
    finalize_run(state, run, "failed", step.output.as_ref(), Some(&message)).await
}

/// Human-readable reason recorded on a step whose execution ended outside the harness.
pub(crate) fn execution_error_message(execution: &ExecutionRecord) -> String {
    execution
        .error
        .as_ref()
        .and_then(|error| error.get("message").and_then(serde_json::Value::as_str))
        .filter(|message| !message.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| "worker failed".into())
}

/// Error codes of executions the platform ended because their worker went away: a lost
/// worker or boot (`worker_lost`), a runtime termination (`worker_oom`, `worker_evicted`,
/// `worker_exited`) or a harness that stopped reporting (`harness_unresponsive`). Their
/// outcome is unknown, so their team run is neither continued nor retried (#343, #394).
/// `worker_did_not_start` is not one of them: nothing ran, so the queue may retry it.
pub(crate) fn ended_without_retry_codes() -> Vec<&'static str> {
    orbit_contracts::WorkerTerminationReason::ALL
        .iter()
        .copied()
        .filter(|code| *code != "worker_did_not_start")
        .chain(["harness_unresponsive"])
        .collect()
}

/// Fail the team run whose current step owns an execution ended by the platform (worker
/// restart, lost heartbeat or expired lease). Both writes are conditional, so repeated
/// calls from the heartbeat and reaper paths are safe.
pub(crate) async fn fail_team_run_for_execution(
    state: &AppState,
    execution: &ExecutionRecord,
) -> anyhow::Result<()> {
    let Some(run_id) = execution.team_run_id else {
        return Ok(());
    };
    let run = state.store.get_team_run(execution.org_id, run_id).await?;
    // A workflow has several steps at once, so find the step by its execution. The run
    // fails as a whole and its other executions are asked to stop.
    //
    // This holds even when a "failed" line leaves the step: the lost worker may have had
    // side effects, so nothing is run after it automatically (the same reason a queue
    // item is not retried).
    if let Some(step) = state.store.team_step_by_execution(execution.id).await? {
        if step.run_id == run_id && step.node_id.is_some() {
            if !matches!(step.status.as_str(), "running" | "failed") {
                return Ok(());
            }
            let message = execution_error_message(execution);
            let _ = state
                .store
                .finish_team_step(step.id, "failed", execution.output.as_ref(), Some(&message))
                .await?;
            let cancelled = state
                .store
                .request_team_run_cancel(run.org_id, run.id)
                .await?;
            announce_cancel(state, run.org_id, &cancelled).await;
            state
                .store
                .close_running_team_steps(
                    run.id,
                    "cancelled",
                    Some("stopped because the run ended"),
                )
                .await?;
            let error = format!("step '{}' finished with {}", step.name, message);
            return finalize_run_no_retry(
                state,
                &run,
                "failed",
                execution.output.as_ref(),
                Some(&error),
            )
            .await;
        }
    }
    let Some(step) = state.store.current_team_step(run_id).await? else {
        return Ok(());
    };
    if step.execution_id != Some(execution.id)
        || !matches!(step.status.as_str(), "running" | "failed")
    {
        return Ok(());
    }
    let message = execution_error_message(execution);
    if step.status == "running" {
        // `None` means another caller already finished this step; still finalize the run.
        let _ = state
            .store
            .finish_team_step(step.id, "failed", execution.output.as_ref(), Some(&message))
            .await?;
    }
    let error = format!("step '{}' finished with {}", step.name, message);
    finalize_run_no_retry(
        state,
        &run,
        "failed",
        execution.output.as_ref(),
        Some(&error),
    )
    .await
}

/// Move a run to a terminal status (conditionally) and close the queue loop. If closing
/// the loop fails the queue item stays `running` with a terminal run; the outcome
/// reconciler (`reconcile_team_outcomes`) retries it on later ticks.
pub(crate) async fn finalize_run(
    state: &AppState,
    run: &TeamRun,
    status: &str,
    output: Option<&serde_json::Value>,
    error: Option<&str>,
) -> anyhow::Result<()> {
    let Some(updated) = state
        .store
        .finish_team_run(run.id, status, output, error)
        .await?
    else {
        return Ok(());
    };
    crate::team_outcomes::close_queue_loop(
        state,
        &updated,
        status,
        updated.output.as_ref(),
        updated.error.as_deref(),
    )
    .await
}

/// Same terminal transition as `finalize_run`, but closes the queue item terminal without
/// offering a retry. Used when the platform - not the harness - ended the execution.
async fn finalize_run_no_retry(
    state: &AppState,
    run: &TeamRun,
    status: &str,
    output: Option<&serde_json::Value>,
    error: Option<&str>,
) -> anyhow::Result<()> {
    let Some(updated) = state
        .store
        .finish_team_run(run.id, status, output, error)
        .await?
    else {
        return Ok(());
    };
    crate::team_outcomes::close_queue_loop_without_retry(
        state,
        &updated,
        status,
        updated.output.as_ref(),
        updated.error.as_deref(),
    )
    .await
}

async fn continue_after_success(
    state: &AppState,
    run: &TeamRun,
    definition: &TeamDefinition,
    finished: &TeamStep,
) -> anyhow::Result<()> {
    let next = (finished.step_index as usize) + 1;
    if definition.mode == "native" || next >= definition.steps.len() {
        return finalize_run(state, run, "succeeded", finished.output.as_ref(), None).await;
    }
    let created = match start_step(state, run, definition, next, finished.output.as_ref()).await {
        Ok(created) => created,
        // Refused by the preflight (access lost, unknown harness): fail the run instead of
        // retrying the same step every poll. A limit that clears by itself is not a refusal.
        Err(error) if error.status.is_client_error() && !is_temporary_limit(&error) => {
            return crate::preflight::fail_team_run(state, run, &error).await;
        }
        Err(error) => {
            // The finished step stays `succeeded` and no next step exists, so the next tick
            // retries. Give up (fail the run visibly) once the retry window has passed so a
            // permanent error (e.g. the agent was deleted) does not retry forever.
            let waited = finished
                .finished_at
                .map(|at| (chrono::Utc::now() - at).num_seconds())
                .unwrap_or(0);
            let limited = is_temporary_limit(&error);
            let window = if limited {
                LIMITED_STEP_RETRY_WINDOW_SECS
            } else {
                START_STEP_RETRY_WINDOW_SECS
            };
            if limited && waited < window {
                // Waiting for an hourly quota or the daily licence limit is expected, not
                // an error to log on every tick.
                tracing::debug!(run_id = %run.id, step = next + 1, reason = %error.message, "team step waits for a limit");
                return Ok(());
            }
            if waited >= window {
                let message = format!("could not start step {}: {}", next + 1, error.message);
                return finalize_run(
                    state,
                    run,
                    "failed",
                    finished.output.as_ref(),
                    Some(&message),
                )
                .await;
            }
            return Err(anyhow::anyhow!(error.message));
        }
    };
    if let Err(error) = auto_launch_step(state, run, definition, &created, None).await {
        fail_run_after_launch_error(state, run, &created, &error).await;
    }
    Ok(())
}

/// A step was created but its worker could not be launched. Nothing would ever claim the
/// queued execution and the next tick sees a step that is already running, so the run is
/// failed now with the launch error instead of waiting for ever (#431).
pub(crate) async fn fail_run_after_launch_error(
    state: &AppState,
    run: &TeamRun,
    step: &TeamStep,
    error: &ApiError,
) {
    tracing::warn!(run_id = %run.id, step = %step.name, error = %error.message, "team step worker launch failed");
    match state
        .store
        .request_team_run_cancel(run.org_id, run.id)
        .await
    {
        Ok(cancelled) => announce_cancel(state, run.org_id, &cancelled).await,
        Err(cancel_error) => {
            tracing::warn!(error = %cancel_error, run_id = %run.id, "failed to cancel executions after a launch error")
        }
    }
    let message = format!(
        "could not launch a worker for step '{}': {}",
        step.name, error.message
    );
    // `finalize_run` also closes the queue item of a queued run, offering a retry.
    if let Err(finish_error) = finalize_run(state, run, "failed", None, Some(&message)).await {
        tracing::warn!(error = %finish_error, run_id = %run.id, "failed to fail a team run after a launch error");
    }
}

/// Retry queue outcomes that were never applied (terminal run, item still `running`) and
/// release queue claims that never got a run attached. Called from the team-run loop.
pub async fn reconcile_team_outcomes(state: &AppState) -> anyhow::Result<()> {
    for run in state.store.list_team_runs_pending_outcome(50).await? {
        if let Err(error) = crate::team_outcomes::close_queue_loop(
            state,
            &run,
            &run.status,
            run.output.as_ref(),
            run.error.as_deref(),
        )
        .await
        {
            tracing::warn!(%error, run_id = %run.id, "retrying queue outcome failed");
        }
    }
    let released = state.store.release_stale_team_queue_claims(600).await?;
    if released > 0 {
        tracing::warn!(released, "released stale team queue claims");
    }
    Ok(())
}

#[cfg(test)]
mod prompt_separation_tests {
    use super::{configured_goal, run_instruction};
    use serde_json::json;

    #[test]
    fn run_instruction_is_only_the_explicit_task() {
        let unique = "AURORA-259-TEAM-UNIQUE-RUN-INSTRUCTION";
        let configured = "AURORA-259-TEAM-CONFIGURED-GOAL";
        let input = json!({
            "task": unique,
            "instructions": configured,
            "definition": { "instructions": configured },
        });
        assert_eq!(run_instruction(&input), unique);
        assert_eq!(configured_goal(&input), configured);
        assert!(!run_instruction(&input).contains(configured));
    }

    #[test]
    fn missing_task_does_not_default_to_configured_goal_text() {
        let input = json!({ "instructions": "goal" });
        assert_eq!(run_instruction(&input), "Team run");
        assert_eq!(configured_goal(&input), "goal");
    }
}

#[cfg(test)]
mod ended_without_retry_tests {
    use super::ended_without_retry_codes;

    #[test]
    fn lost_and_terminated_workers_end_without_retry_but_a_worker_that_never_started_does_not() {
        let codes = ended_without_retry_codes();
        for code in [
            "worker_lost",
            "worker_oom",
            "worker_evicted",
            "worker_exited",
            "harness_unresponsive",
        ] {
            assert!(codes.contains(&code), "{code}");
        }
        // Nothing ran, so the queue keeps its retry (#399), also once it is a termination
        // reason of its own.
        assert!(!codes.contains(&"worker_did_not_start"));
        assert!(!codes.contains(&"harness_error"));
    }
}
