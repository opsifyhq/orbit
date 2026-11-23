//! Agent teams: sequential role handoffs materialized as normal executions.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use orbit_domain::{HarnessKind, TeamAutonomy, TeamAutonomyMode, TriggerSource, ValidationIssue};
use orbit_security::{AuthContext, Permission};
use orbit_store::{AgentTeam, TeamRun, TeamStep};

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
    /// and materializes the remaining agents as harness-native subagents.
    #[serde(default = "default_mode")]
    pub mode: String,
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
    pub steps: Vec<TeamStepInput>,
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
    pub steps: Vec<TeamStepDefinition>,
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
    }
    let is_allowed = |value: &str| {
        allowed
            .iter()
            .any(|candidate| candidate.trim().eq_ignore_ascii_case(value.trim()))
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

/// Validate a fixed team autonomy and authorise any elevation above a step agent's own
/// level. Lowering is always allowed; raising requires manage access to that agent, so a
/// team cannot silently turn an L2 agent into an L4 executor.
async fn validate_team_autonomy(
    state: &AppState,
    auth: &AuthContext,
    access: &crate::access::AccessScope,
    autonomy: Option<&TeamAutonomy>,
    steps: &[TeamStepInput],
    issues: &mut Vec<ValidationIssue>,
) -> Vec<(Uuid, String, u8)> {
    let mut elevated = Vec::new();
    let Some(autonomy) = autonomy else {
        return elevated;
    };
    if autonomy.mode != TeamAutonomyMode::Fixed {
        return elevated;
    }
    let Some(level) = autonomy.level.filter(|level| (1..=4).contains(level)) else {
        issues.push(ValidationIssue::new(
            "autonomy.level",
            "a fixed team autonomy needs a level from 1 to 4",
        ));
        return elevated;
    };
    for step in steps {
        let Ok(agent) = state.store.get_agent(auth.org_id, step.agent_id).await else {
            continue;
        };
        let Ok(version) = state.store.get_current_version(auth.org_id, &agent).await else {
            continue;
        };
        let Ok(spec) = serde_json::from_value::<orbit_domain::AgentSpec>(version.spec) else {
            continue;
        };
        if level > spec.autonomy.level {
            elevated.push((agent.id, agent.name.clone(), spec.autonomy.level));
            if !access.can_manage(agent.owner_user_id, agent.owner_team_id) {
                issues.push(ValidationIssue::new(
                    "autonomy.level",
                    format!(
                        "raising agent '{}' from level {} to {} requires manage access to that agent",
                        agent.name, spec.autonomy.level, level
                    ),
                ));
            }
        }
    }
    elevated
}

async fn audit_team_autonomy_elevations(
    state: &AppState,
    auth: &AuthContext,
    team_id: Uuid,
    elevated: &[(Uuid, String, u8)],
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
            "agents": elevated
                .iter()
                .map(|(id, name, from)| serde_json::json!({ "id": id, "name": name, "from": from }))
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
    crate::extract::ApiJson(body): crate::extract::ApiJson<CreateTeamRequest>,
) -> Result<(StatusCode, Json<AgentTeam>), ApiError> {
    auth.require(Permission::SettingsWrite)?;
    if body.name.trim().is_empty() || body.steps.is_empty() {
        return Err(ApiError::validation(vec![
            ValidationIssue::new("name", "name is required"),
            ValidationIssue::new("steps", "at least one step is required"),
        ]));
    }
    if !matches!(body.mode.as_str(), "pipeline" | "native") {
        return Err(ApiError::validation(vec![ValidationIssue::new(
            "mode",
            "mode must be pipeline or native",
        )]));
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
    for step in &body.steps {
        if !agent_ids.insert(step.agent_id) {
            issues.push(ValidationIssue::new(
                "steps.agentId",
                "the same agent cannot appear twice in one team",
            ));
        }
        // An agent the caller cannot view is reported exactly like a missing one.
        let viewable = match state.store.get_agent(auth.org_id, step.agent_id).await {
            Ok(agent) => {
                access.can_view(agent.owner_user_id, agent.owner_team_id, &agent.visibility)
            }
            Err(_) => false,
        };
        if !viewable {
            issues.push(ValidationIssue::new(
                "steps.agentId",
                format!("unknown agent '{}'", step.agent_id),
            ));
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
    let elevations = validate_team_autonomy(
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
        "autonomy": body.autonomy,
        "steps": body.steps.iter().map(|step| serde_json::json!({
            "agentId": step.agent_id,
            "model": step.model,
        })).collect::<Vec<_>>()
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
    audit_team_autonomy_elevations(&state, &auth, team.id, &elevations, body.autonomy.as_ref())
        .await;
    Ok((StatusCode::CREATED, Json(team)))
}

#[utoipa::path(patch, path = "/api/v1/agent-teams/{team_id}", tag = "agent-teams", request_body = CreateTeamRequest, responses((status = 200, body = orbit_store::AgentTeam)))]
pub async fn update_team(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(team_id): Path<Uuid>,
    crate::extract::ApiJson(body): crate::extract::ApiJson<CreateTeamRequest>,
) -> Result<Json<AgentTeam>, ApiError> {
    auth.require(Permission::SettingsWrite)?;
    require_team_manage(&state, &auth, team_id).await?;
    if body.name.trim().is_empty() || body.steps.is_empty() {
        return Err(ApiError::validation(vec![
            ValidationIssue::new("name", "name is required"),
            ValidationIssue::new("steps", "at least one step is required"),
        ]));
    }
    if !matches!(body.mode.as_str(), "pipeline" | "native") {
        return Err(ApiError::validation(vec![ValidationIssue::new(
            "mode",
            "mode must be pipeline or native",
        )]));
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
    for step in &body.steps {
        if !agent_ids.insert(step.agent_id) {
            issues.push(ValidationIssue::new(
                "steps.agentId",
                "the same agent cannot appear twice in one team",
            ));
        }
        // An agent the caller cannot view is reported exactly like a missing one.
        let viewable = match state.store.get_agent(auth.org_id, step.agent_id).await {
            Ok(agent) => {
                access.can_view(agent.owner_user_id, agent.owner_team_id, &agent.visibility)
            }
            Err(_) => false,
        };
        if !viewable {
            issues.push(ValidationIssue::new(
                "steps.agentId",
                format!("unknown agent '{}'", step.agent_id),
            ));
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
    let elevations = validate_team_autonomy(
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
        "autonomy": body.autonomy,
        "steps": body.steps.iter().map(|step| serde_json::json!({
            "agentId": step.agent_id,
            "model": step.model,
        })).collect::<Vec<_>>()
    });
    let team = state
        .store
        .update_agent_team(
            auth.org_id,
            team_id,
            body.name.trim(),
            body.description.trim(),
            &definition,
        )
        .await?;
    audit_team_autonomy_elevations(&state, &auth, team.id, &elevations, body.autonomy.as_ref())
        .await;
    Ok(Json(team))
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
    state.store.delete_agent_team(auth.org_id, team_id).await?;
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
        let mut instructions = spec.instructions.system.clone();
        if !spec.instructions.constraints.is_empty() {
            instructions.push_str("\n\nConstraints:\n");
            for constraint in &spec.instructions.constraints {
                instructions.push_str(&format!("- {constraint}\n"));
            }
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
            "instructions": instructions,
            "model": model,
            "modelOverride": model_override,
        }));
    }
    Ok(subagents)
}

pub(crate) async fn start_step(
    state: &AppState,
    run: &TeamRun,
    definition: &TeamDefinition,
    step_index: usize,
    previous_output: Option<&serde_json::Value>,
) -> Result<TeamStep, ApiError> {
    // Every team step creates an execution through `create_execution_with_limit`, so
    // manual runs, scheduled/queue starts and dispatcher auto-run share the same guard.
    let step = definition
        .steps
        .get(step_index)
        .ok_or_else(|| ApiError::internal("team step index out of range"))?;
    let agent = state.store.get_agent(run.org_id, step.agent_id).await?;
    let version = state.store.get_current_version(run.org_id, &agent).await?;
    let mut spec: orbit_domain::AgentSpec = serde_json::from_value(version.spec.clone())
        .map_err(|error| ApiError::internal(format!("stored agent spec is invalid: {error}")))?;

    // Apply the step's (or the team's) model override. Preflight checks the provider of
    // the overridden model, and the override travels with the execution so the worker
    // runs that same model (#305).
    let model_override = definition.model_override(step_index).cloned();
    if let Some(override_model) = &model_override {
        override_model.apply(&mut spec.model);
    }
    // #259: the run instruction is the single task for every step. The team goal is
    // configured system context and the previous step output is delegated context;
    // neither is copied into the task the harness presents as user input.
    let task = run_instruction(&run.input).to_string();
    let instructions = configured_goal(&run.input);
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
    if let Some(autonomy) = definition.autonomy.as_ref() {
        if autonomy.mode == TeamAutonomyMode::Fixed {
            input[orbit_domain::TEAM_AUTONOMY_KEY] =
                serde_json::to_value(autonomy).unwrap_or(serde_json::Value::Null);
        }
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
            idempotency_key: Some(&format!("team:{}:{}", run.id, step_index)),
            team_run_id: Some(run.id),
            team_step_index: Some(step_index as i32),
        })
        .await?;
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
    let step = match start_step(&state, &run, &definition, 0, None).await {
        Ok(step) => step,
        Err(error) => {
            let _ = state
                .store
                .update_team_run(run.id, "failed", None, Some(&error.message))
                .await;
            return Err(error);
        }
    };
    if definition.auto_launch {
        if let (Some(profile_id), Some(execution_id)) =
            (definition.worker_profile_id, step.execution_id)
        {
            crate::handlers::worker_launches::launch_profile_internal(
                &state,
                &auth,
                profile_id,
                Some(execution_id),
            )
            .await?;
        }
    }
    Ok((
        StatusCode::CREATED,
        Json(TeamRunDetail {
            run,
            steps: vec![step],
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
const ADVANCE_LEASE_SECS: i64 = 120;
/// How long a run whose next step cannot be started is retried before it is failed.
const START_STEP_RETRY_WINDOW_SECS: i64 = 300;
/// How long a step held back by an hourly quota or the licence's daily execution limit is
/// retried. Long enough for an hourly quota to clear (#305).
const LIMITED_STEP_RETRY_WINDOW_SECS: i64 = 3_900;

/// Limits that clear by themselves: the agent's `maxExecutionsPerHour` (429) and the
/// licence's rolling daily execution limit (402).
fn is_temporary_limit(error: &ApiError) -> bool {
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

async fn advance_claimed(state: &AppState, run: TeamRun) -> anyhow::Result<()> {
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
                        Some(&execution.status),
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

/// Move a run to a terminal status (conditionally) and close the queue loop. If closing
/// the loop fails the queue item stays `running` with a terminal run; the outcome
/// reconciler (`reconcile_team_outcomes`) retries it on later ticks.
async fn finalize_run(
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
    if definition.auto_launch {
        if let (Some(profile_id), Some(execution_id)) =
            (definition.worker_profile_id, created.execution_id)
        {
            let auth = AuthContext {
                user_id: run.requested_by.unwrap_or(Uuid::nil()),
                org_id: run.org_id,
                email: String::new(),
                display_name: "Agent Team".into(),
                role: orbit_domain::UserRole::Operator,
                method: orbit_security::AuthMethod::ApiToken,
                scopes: vec![],
                token_id: None,
            };
            crate::handlers::worker_launches::launch_profile_internal(
                state,
                &auth,
                profile_id,
                Some(execution_id),
            )
            .await
            .map_err(|error| anyhow::anyhow!(error.message))?;
        }
    }
    Ok(())
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
