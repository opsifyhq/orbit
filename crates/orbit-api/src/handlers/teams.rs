//! Agent teams: sequential role handoffs materialized as normal executions.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use orbit_domain::{
    HarnessKind, TeamAutonomy, TeamAutonomyMode, ValidationIssue, WorkflowDefinition, WORKFLOW_MODE,
};
use orbit_security::{AuthContext, Permission};
use orbit_store::AgentTeam;

use crate::dto::{TeamRunListQuery, TeamRunPage};
use crate::error::ApiError;
use crate::state::AppState;
use crate::team_runs::{
    announce_cancel, approval_mode_label, auto_launch_step, default_mode, parse_definition,
    start_first_steps, team_connection_ids, validate_requested_repositories, TeamModelOverride,
    TeamRunDetail,
};
use crate::work_sources::{
    validate_repositories, validate_work_source, RepositorySpec, WorkSourceDefinition,
};

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
