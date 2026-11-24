//! Shared execution preflight (issue #140).
//!
//! Every path that starts an execution on behalf of an identity goes through the checks
//! in this module so they cannot drift apart: manual execute, rerun, runtime start and
//! ticks, scheduled triggers, team runs/steps and queue starts. The checks are:
//!
//! 1. the agent is not archived and the identity can **use** it,
//! 2. the identity can **use** every connection bound in the spec (plus any extra
//!    connection ids, e.g. a team's shared connections) and the platform-managed model
//!    provider,
//! 3. the harness is enabled and `available`,
//! 4. the agent's `autonomy.maxExecutionsPerHour` quota has headroom.
//!
//! Callers with an HTTP request build the [`AccessScope`] from the request identity.
//! Background callers have no request, so they resolve a stored identity with
//! [`background_access`] (trigger owner, runtime creator, team-run requester) and load
//! its scope with `AccessScope::load_for_user`, i.e. access is re-evaluated at run time.
//!
//! Other entry points that need the same rules (for example the trigger-create and queue
//! visibility fixes in #135 / #132) can call [`Preflight::authorize_access`] or
//! [`authorize_team_run`] directly.

use uuid::Uuid;

use orbit_domain::{AgentSpec, HarnessKind, ValidationIssue};
use orbit_store::Agent;

use crate::access::AccessScope;
use crate::error::ApiError;
use crate::state::AppState;

/// Attach a field path to an access error so clients see which reference was refused.
fn with_field(mut error: ApiError, path: impl Into<String>) -> ApiError {
    error.details = vec![ValidationIssue::new(path, error.message.clone())];
    error
}

/// Require use rights on one connection. `404` when invisible, `403` when visible but
/// the role cannot use it.
pub async fn require_connection_use(
    state: &AppState,
    org_id: Uuid,
    access: &AccessScope,
    connection_id: Uuid,
    path: &str,
) -> Result<(), ApiError> {
    let connection = state
        .store
        .get_connection(org_id, connection_id)
        .await
        .map_err(|error| with_field(error.into(), path))?;
    access
        .require_use(
            "connection",
            connection.owner_user_id,
            connection.owner_team_id,
            &connection.visibility,
        )
        .map_err(|error| with_field(error, path))
}

/// Issues for skill slugs the principal may not use. A skill the caller cannot see or use
/// is reported exactly like a missing one ("unknown skill"), so private slugs do not leak.
/// `path` is the field name; with `indexed` each issue is reported at `path[idx]`.
pub async fn unusable_skill_issues(
    state: &AppState,
    org_id: Uuid,
    access: &AccessScope,
    slugs: &[String],
    path: &str,
    indexed: bool,
) -> Result<Vec<ValidationIssue>, ApiError> {
    let mut issues = Vec::new();
    for (idx, slug) in slugs.iter().enumerate() {
        let usable = match state.store.get_skill_by_slug(org_id, slug).await? {
            Some(skill) => {
                access.can_use(skill.owner_user_id, skill.owner_team_id, &skill.visibility)
            }
            None => false,
        };
        if !usable {
            let field = if indexed {
                format!("{path}[{idx}]")
            } else {
                path.to_string()
            };
            issues.push(ValidationIssue::new(
                field,
                format!("unknown skill '{slug}'"),
            ));
        }
    }
    Ok(issues)
}

/// Require that the principal may use every skill slug (agent specs, executions, teams).
/// Failures are a 422 with an "unknown skill" field error, identical to a missing skill.
pub async fn require_skills_use(
    state: &AppState,
    org_id: Uuid,
    access: &AccessScope,
    slugs: &[String],
    path: &str,
    indexed: bool,
) -> Result<(), ApiError> {
    let issues = unusable_skill_issues(state, org_id, access, slugs, path, indexed).await?;
    if issues.is_empty() {
        Ok(())
    } else {
        Err(ApiError::validation(issues))
    }
}

/// Require use rights on every connection and the platform provider a spec references.
/// Used by agent create/update/import (the author must be able to use what they bind)
/// and by the execution preflight (the runner must be able to use what the agent binds).
/// Skills are checked at authoring time via [`require_spec_use`]: running a shared agent
/// does not require the runner to have been granted the agent's own skills.
pub async fn require_spec_references_use(
    state: &AppState,
    org_id: Uuid,
    access: &AccessScope,
    spec: &AgentSpec,
) -> Result<(), ApiError> {
    for (idx, binding) in spec.connectors.iter().enumerate() {
        if let Some(connection_id) = binding.connection_id {
            require_connection_use(
                state,
                org_id,
                access,
                connection_id,
                &format!("spec.connectors[{idx}].connectionId"),
            )
            .await?;
        }
    }
    // Both management modes: a harness-managed model that names a provider still gets
    // that provider's credential injected by the worker context (issue #193).
    if let Some(name) = &spec.model.provider {
        require_provider_use(state, org_id, access, name, "spec.model.provider").await?;
    }
    Ok(())
}

/// Authoring-time check: connections, provider and skills of a spec the caller is saving.
pub async fn require_spec_use(
    state: &AppState,
    org_id: Uuid,
    access: &AccessScope,
    spec: &AgentSpec,
) -> Result<(), ApiError> {
    require_skills_use(state, org_id, access, &spec.skills, "spec.skills", true).await?;
    require_spec_references_use(state, org_id, access, spec).await
}

/// Require use rights on a provider named in a model binding. Names that match no
/// provider row are harness-local (nothing platform-owned to protect) and pass; the
/// platform-managed path rejects unknown names separately.
pub async fn require_provider_use(
    state: &AppState,
    org_id: Uuid,
    access: &AccessScope,
    name: &str,
    path: &str,
) -> Result<(), ApiError> {
    if let Some(provider) = state.store.get_provider_by_name(org_id, name).await? {
        access
            .require_use(
                "provider",
                provider.owner_user_id,
                provider.owner_team_id,
                &provider.visibility,
            )
            .map_err(|error| with_field(error, path))?;
    }
    Ok(())
}

/// Reject a disabled or not-yet-available harness.
pub async fn require_harness_available(
    state: &AppState,
    org_id: Uuid,
    harness: HarnessKind,
) -> Result<(), ApiError> {
    if let Some(record) = state
        .store
        .get_harness_by_kind(org_id, harness.as_str())
        .await?
    {
        if !record.enabled || record.implementation_status != "available" {
            let reason = if record.enabled {
                record.implementation_status.clone()
            } else {
                "disabled".to_string()
            };
            return Err(ApiError::validation(vec![ValidationIssue::new(
                "harness",
                format!("harness '{harness}' is {reason} and cannot run executions"),
            )]));
        }
    }
    Ok(())
}

/// Enforce `autonomy.maxExecutionsPerHour` for the agent (`429` when reached).
pub async fn require_hourly_quota(
    state: &AppState,
    org_id: Uuid,
    agent_id: Uuid,
    spec: &AgentSpec,
) -> Result<(), ApiError> {
    if let Some(max_per_hour) = spec.autonomy.max_executions_per_hour {
        let since = chrono::Utc::now() - chrono::Duration::hours(1);
        let count = state
            .store
            .count_executions_since(org_id, agent_id, since)
            .await?;
        if count >= max_per_hour as i64 {
            return Err(ApiError::rate_limited(format!(
                "agent execution quota reached ({max_per_hour}/hour)"
            )));
        }
    }
    Ok(())
}

/// The single execution preflight. Build it with the agent, the spec that will actually
/// run (after any overrides) and the harness that will run it.
pub struct Preflight<'a> {
    pub state: &'a AppState,
    pub org_id: Uuid,
    pub access: &'a AccessScope,
    pub agent: &'a Agent,
    pub spec: &'a AgentSpec,
    pub harness: HarnessKind,
    /// Connections used outside the agent spec (team shared connections, work source).
    pub extra_connection_ids: &'a [Uuid],
}

impl Preflight<'_> {
    /// Agent state + use, spec connections/provider use, extra connections use.
    pub async fn authorize_access(&self) -> Result<(), ApiError> {
        if self.agent.status == "archived" {
            return Err(ApiError::bad_request("archived agents cannot be executed"));
        }
        self.access.require_use(
            "agent",
            self.agent.owner_user_id,
            self.agent.owner_team_id,
            &self.agent.visibility,
        )?;
        require_spec_references_use(self.state, self.org_id, self.access, self.spec).await?;
        for (idx, connection_id) in self.extra_connection_ids.iter().enumerate() {
            require_connection_use(
                self.state,
                self.org_id,
                self.access,
                *connection_id,
                &format!("connectionIds[{idx}]"),
            )
            .await?;
        }
        Ok(())
    }

    pub async fn check_harness(&self) -> Result<(), ApiError> {
        require_harness_available(self.state, self.org_id, self.harness).await
    }

    pub async fn check_quota(&self) -> Result<(), ApiError> {
        require_hourly_quota(self.state, self.org_id, self.agent.id, self.spec).await
    }

    /// Access, harness and quota, in that order.
    pub async fn run(&self) -> Result<(), ApiError> {
        self.authorize_access().await?;
        self.check_harness().await?;
        self.check_quota().await
    }
}

/// Team-level preflight before a run is created: the identity must be able to use the
/// team, every step agent (with its connections/provider) and the team's connections.
/// Harness and quota are evaluated per step by [`Preflight::run`] when the step starts.
pub async fn authorize_team_run(
    state: &AppState,
    org_id: Uuid,
    access: &AccessScope,
    team: &orbit_store::AgentTeam,
    step_agent_ids: &[Uuid],
    connection_ids: &[Uuid],
) -> Result<(), ApiError> {
    access.require_use(
        "agent team",
        team.owner_user_id,
        team.owner_team_id,
        &team.visibility,
    )?;
    let team_skills: Vec<String> = team
        .definition
        .get("skills")
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    require_skills_use(state, org_id, access, &team_skills, "skills", false).await?;
    for (idx, connection_id) in connection_ids.iter().enumerate() {
        require_connection_use(
            state,
            org_id,
            access,
            *connection_id,
            &format!("connectionIds[{idx}]"),
        )
        .await?;
    }
    for agent_id in step_agent_ids {
        let agent = state.store.get_agent(org_id, *agent_id).await?;
        access.require_use(
            "agent",
            agent.owner_user_id,
            agent.owner_team_id,
            &agent.visibility,
        )?;
        let version = state.store.get_current_version(org_id, &agent).await?;
        let spec: AgentSpec = serde_json::from_value(version.spec).map_err(|error| {
            ApiError::internal(format!("stored agent spec is invalid: {error}"))
        })?;
        require_spec_references_use(state, org_id, access, &spec).await?;
    }
    Ok(())
}

/// Identity for runs that have no HTTP caller: the first listed owner that exists.
/// `None` means a legacy, unowned resource; those keep running under platform authority
/// (only the non-identity checks apply). Pass the most specific owner first.
pub async fn background_access(
    state: &AppState,
    org_id: Uuid,
    owners: &[Option<Uuid>],
) -> Result<Option<AccessScope>, ApiError> {
    match owners.iter().flatten().next() {
        Some(user_id) => Ok(Some(
            AccessScope::load_for_user(state, org_id, *user_id).await?,
        )),
        None => Ok(None),
    }
}

/// The identity a background start acts as when it needs a full [`AuthContext`] (queue
/// auto-run, team-step auto-launch): the first of `owners` that is set, loaded from the
/// store so the user's current role and status apply. `None` when no owner is set.
///
/// Background paths never act as a made-up user: the nil UUID is not a user, and every
/// `requested_by` / `created_by` column is a foreign key to `users` (#383).
pub async fn background_identity(
    state: &AppState,
    org_id: Uuid,
    owners: &[Option<Uuid>],
) -> Result<Option<orbit_security::AuthContext>, ApiError> {
    let Some(user_id) = owners.iter().flatten().next().copied() else {
        return Ok(None);
    };
    let user = state.store.get_user(org_id, user_id).await?;
    if user.status != "active" {
        return Err(ApiError::forbidden("the run's identity is disabled"));
    }
    let role = user
        .role
        .parse::<orbit_domain::UserRole>()
        .unwrap_or(orbit_domain::UserRole::Viewer);
    Ok(Some(orbit_security::AuthContext {
        user_id,
        org_id,
        email: user.email,
        display_name: user.display_name,
        role,
        method: orbit_security::AuthMethod::ApiToken,
        scopes: Vec::new(),
        token_id: None,
    }))
}

/// Record a refused background run (trigger fire, runtime tick) in the audit log with a
/// system actor so operators can see why it did not start.
pub async fn audit_denied_run(
    state: &AppState,
    org_id: Uuid,
    action: &str,
    target_type: &str,
    target_id: Uuid,
    error: &ApiError,
    identity: Option<Uuid>,
) {
    let result = state
        .store
        .append_audit_log(
            org_id,
            "system",
            identity,
            None,
            action,
            Some(target_type),
            Some(target_id),
            serde_json::json!({ "error": error.message, "code": error.code }),
            None,
            None,
        )
        .await;
    if let Err(err) = result {
        tracing::error!(error = %err, action, "failed to write audit log");
    }
}

/// Fail a team run whose next step cannot start because the requester lost access (or
/// the step is otherwise refused with a client error). Without this the 2 second team
/// poller would retry the same refused step forever.
pub async fn fail_team_run(
    state: &AppState,
    run: &orbit_store::TeamRun,
    error: &ApiError,
) -> anyhow::Result<()> {
    let message = format!("next step could not start: {}", error.message);
    state
        .store
        .update_team_run(run.id, "failed", None, Some(&message))
        .await?;
    crate::team_outcomes::close_queue_loop(state, run, "failed", None, Some(&message)).await?;
    Ok(())
}
