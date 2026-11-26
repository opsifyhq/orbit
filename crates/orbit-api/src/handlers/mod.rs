pub mod access_teams;
pub mod account;
pub mod agents;
pub mod approvals;
pub mod auth_oidc;
pub mod bulk_lifecycle;
pub mod catalog;
pub mod dashboard;
pub mod docs;
pub mod env_mappings;
pub mod executions;
pub mod export;
pub mod health;
pub mod import;
pub mod import_portable;
pub mod license;
pub mod outputs;
pub mod roles;
pub mod runtime;
pub mod settings;
pub mod sharing;
pub mod skills;
pub mod team_queue;
pub mod teams;
pub mod tool_policies;
pub mod triggers;
pub mod worker_api;
pub mod worker_launches;
pub mod worker_profiles;

use uuid::Uuid;

use orbit_domain::AgentSpec;
use orbit_security::AuthContext;
use orbit_store::AgentVersion;

use crate::dto::AgentDetail;
use crate::error::ApiError;
use crate::state::AppState;

pub(crate) fn parse_spec(version: &AgentVersion) -> Result<AgentSpec, ApiError> {
    serde_json::from_value(version.spec.clone()).map_err(|err| {
        tracing::error!(error = %err, agent_version = %version.id, "stored agent spec is invalid");
        ApiError::internal("stored agent configuration is invalid")
    })
}

/// Existing keyed-list endpoints use the same limit range as `PageQuery`: an
/// out-of-range `limit` is a bad request, never silently clamped (#443).
pub(crate) fn limit(value: Option<u32>) -> Result<i64, ApiError> {
    let limit = value.unwrap_or(25);
    if !(1..=crate::paging::MAX_PAGE_LIMIT as u32).contains(&limit) {
        return Err(ApiError::bad_request(format!(
            "limit must be 1 to {}",
            crate::paging::MAX_PAGE_LIMIT
        )));
    }
    Ok(limit as i64)
}

/// Validate an existing `i64` limit without clamping it.
pub(crate) fn limit_i64(value: Option<i64>, default: i64) -> Result<i64, ApiError> {
    let limit = value.unwrap_or(default);
    if !(1..=crate::paging::MAX_PAGE_LIMIT).contains(&limit) {
        return Err(ApiError::bad_request(format!(
            "limit must be 1 to {}",
            crate::paging::MAX_PAGE_LIMIT
        )));
    }
    Ok(limit)
}

/// Load an agent the caller may view; invisible agents are indistinguishable from missing (404).
pub(crate) async fn load_visible_agent(
    state: &AppState,
    auth: &AuthContext,
    agent_id: Uuid,
) -> Result<orbit_store::Agent, ApiError> {
    let agent = state.store.get_agent(auth.org_id, agent_id).await?;
    let access = crate::access::AccessScope::load(state, auth).await?;
    access.require_view(
        "agent",
        agent.owner_user_id,
        agent.owner_team_id,
        &agent.visibility,
    )?;
    Ok(agent)
}

pub(crate) async fn load_agent_detail(
    state: &AppState,
    auth: &AuthContext,
    agent_id: Uuid,
) -> Result<AgentDetail, ApiError> {
    let agent = load_visible_agent(state, auth, agent_id).await?;
    let version = state.store.get_current_version(auth.org_id, &agent).await?;
    let spec = parse_spec(&version)?;
    let triggers = state
        .store
        .list_triggers(auth.org_id, Some(agent.id), None)
        .await?;
    let (executions, _, _) = state
        .store
        .list_executions(auth.org_id, 5, None, None, Some(agent.id), None, None)
        .await?;

    Ok(AgentDetail {
        id: agent.id,
        slug: agent.slug,
        name: agent.name,
        description: agent.description,
        status: agent.status,
        current_version: agent.current_version,
        labels: agent.labels,
        spec,
        definition_yaml: version.definition_yaml,
        version_id: version.id,
        version_created_at: version.created_at,
        triggers,
        recent_executions: executions,
        created_at: agent.created_at,
        updated_at: agent.updated_at,
    })
}
