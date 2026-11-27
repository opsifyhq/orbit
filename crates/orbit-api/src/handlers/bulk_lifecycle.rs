//! Bulk archive/delete for agents and bulk delete for skills.
//!
//! These endpoints deliberately mirror the single-resource handlers: the same
//! `AccessScope::require_manage` check, the same `set_agent_status` transition rules and
//! the same FK-enforced `DELETE` statements. One failure never rolls back the other ids;
//! the response lists exactly what changed and what did not.

use axum::extract::State;
use axum::Extension;
use axum::Json;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use uuid::Uuid;

use orbit_domain::{AgentStatus, ValidationIssue};
use orbit_security::{AuthContext, Permission};

use crate::access::AccessScope;
use crate::audit;
use crate::error::ApiError;
use crate::state::AppState;

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BulkLifecycleRequest {
    pub ids: Vec<Uuid>,
    /// Evaluate eligibility and report results without changing anything. Used by the web
    /// UI to explain mixed selections before asking for confirmation.
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BulkLifecycleFailure {
    pub id: Uuid,
    /// Stable envelope code from the equivalent single-resource request (`forbidden`,
    /// `not_found`, `conflict`, `bad_request`).
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BulkLifecycleResponse {
    /// Ids archived by this request (empty for delete requests).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub updated: Vec<Uuid>,
    /// Ids deleted by this request (empty for archive requests).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub deleted: Vec<Uuid>,
    /// Ids already in the requested state; no change was made.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unchanged: Vec<Uuid>,
    pub failed: Vec<BulkLifecycleFailure>,
}

#[derive(Debug, Clone, Copy)]
enum Operation {
    ActivateAgent,
    ArchiveAgent,
    DeleteAgent,
    DeleteSkill,
}

#[derive(Debug, Clone, Copy)]
enum Outcome {
    Changed,
    Unchanged,
}

impl Operation {
    fn audit_action(self) -> &'static str {
        match self {
            Self::ActivateAgent => "agent.bulk_activated",
            Self::ArchiveAgent => "agent.bulk_archived",
            Self::DeleteAgent => "agent.bulk_deleted",
            Self::DeleteSkill => "skill.bulk_deleted",
        }
    }
}

/// Shared implementation. `auth.require` and id validation happen once; manage access,
/// transition rules and FK checks happen per id so one bad id cannot hide the others.
async fn run_bulk(
    state: &AppState,
    auth: &AuthContext,
    body: BulkLifecycleRequest,
    operation: Operation,
) -> Result<Json<BulkLifecycleResponse>, ApiError> {
    auth.require(Permission::AgentsWrite)?;
    let ids: Vec<Uuid> = body
        .ids
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if ids.is_empty() || ids.len() > 500 {
        return Err(ApiError::validation(vec![ValidationIssue::new(
            "ids",
            "select between 1 and 500 resources",
        )]));
    }
    let access = AccessScope::load(state, auth).await?;
    let requested = ids.len();
    let mut result = BulkLifecycleResponse::default();

    for id in ids {
        let outcome: Result<Outcome, ApiError> = async {
            match operation {
                Operation::ActivateAgent => {
                    let agent = state.store.get_agent(auth.org_id, id).await?;
                    access.require_manage(
                        "agent",
                        agent.owner_user_id,
                        agent.owner_team_id,
                        &agent.visibility,
                    )?;
                    let status = AgentStatus::parse(&agent.status).unwrap_or(AgentStatus::Draft);
                    if status == AgentStatus::Active {
                        return Ok(Outcome::Unchanged);
                    }
                    if !status.can_activate() {
                        return Err(ApiError::bad_request(format!(
                            "cannot transition agent from {} to active",
                            status.as_str()
                        )));
                    }
                    // Restoring an archived agent consumes a seat. Hold the limit lock
                    // while counting and writing, like the single-agent endpoint.
                    let agent_limit = state.license_limit_lock("agents").await?;
                    if agent_limit.is_some() && status == AgentStatus::Archived {
                        state.enforce_agent_limit().await?;
                    }
                    if !body.dry_run {
                        state
                            .store
                            .set_agent_status(auth.org_id, id, AgentStatus::Active)
                            .await?;
                    }
                    drop(agent_limit);
                }
                Operation::ArchiveAgent | Operation::DeleteAgent => {
                    let agent = state.store.get_agent(auth.org_id, id).await?;
                    access.require_manage(
                        "agent",
                        agent.owner_user_id,
                        agent.owner_team_id,
                        &agent.visibility,
                    )?;
                    if matches!(operation, Operation::ArchiveAgent) {
                        let status =
                            AgentStatus::parse(&agent.status).unwrap_or(AgentStatus::Draft);
                        if status == AgentStatus::Archived {
                            return Ok(Outcome::Unchanged);
                        }
                        // Same rule as `set_agent_status`; checked here so a preview can
                        // report no-op/invalid transitions without writing.
                        if !status.can_archive() {
                            return Err(ApiError::bad_request(format!(
                                "cannot transition agent from {} to archived",
                                status.as_str()
                            )));
                        }
                        if !body.dry_run {
                            state
                                .store
                                .set_agent_status(auth.org_id, id, AgentStatus::Archived)
                                .await?;
                        }
                    } else if body.dry_run {
                        state.store.check_delete_agent(auth.org_id, id).await?;
                    } else {
                        state.store.delete_agent(auth.org_id, id).await?;
                    }
                }
                Operation::DeleteSkill => {
                    let skill = state.store.get_skill(auth.org_id, id).await?;
                    access.require_manage(
                        "skill",
                        skill.owner_user_id,
                        skill.owner_team_id,
                        &skill.visibility,
                    )?;
                    if body.dry_run {
                        state.store.check_delete_skill(auth.org_id, id).await?;
                    } else {
                        state.store.delete_skill(auth.org_id, id).await?;
                    }
                }
            }
            Ok(Outcome::Changed)
        }
        .await;

        match outcome {
            Ok(Outcome::Changed) => match operation {
                Operation::ActivateAgent | Operation::ArchiveAgent => result.updated.push(id),
                Operation::DeleteAgent | Operation::DeleteSkill => result.deleted.push(id),
            },
            Ok(Outcome::Unchanged) => result.unchanged.push(id),
            Err(error) => result.failed.push(BulkLifecycleFailure {
                id,
                code: error.code.to_string(),
                message: error.message,
            }),
        }
    }

    // A preview is read-only and must not pollute the audit log.
    if !body.dry_run {
        audit::record(
            state,
            auth,
            operation.audit_action(),
            None,
            None,
            serde_json::json!({
                "requested": requested,
                "updated": result.updated.len(),
                "deleted": result.deleted.len(),
                "unchanged": result.unchanged.len(),
                "failed": result.failed.len(),
                "failures": result.failed,
            }),
        )
        .await;
    }
    Ok(Json(result))
}

#[utoipa::path(post, path = "/api/v1/agents/bulk/activate", tag = "agents", request_body = BulkLifecycleRequest, responses((status = 200, body = BulkLifecycleResponse)))]
pub async fn bulk_activate_agents(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    crate::extract::ApiJson(body): crate::extract::ApiJson<BulkLifecycleRequest>,
) -> Result<Json<BulkLifecycleResponse>, ApiError> {
    run_bulk(&state, &auth, body, Operation::ActivateAgent).await
}

#[utoipa::path(post, path = "/api/v1/agents/bulk/archive", tag = "agents", request_body = BulkLifecycleRequest, responses((status = 200, body = BulkLifecycleResponse)))]
pub async fn bulk_archive_agents(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    crate::extract::ApiJson(body): crate::extract::ApiJson<BulkLifecycleRequest>,
) -> Result<Json<BulkLifecycleResponse>, ApiError> {
    run_bulk(&state, &auth, body, Operation::ArchiveAgent).await
}

#[utoipa::path(post, path = "/api/v1/agents/bulk/delete", tag = "agents", request_body = BulkLifecycleRequest, responses((status = 200, body = BulkLifecycleResponse)))]
pub async fn bulk_delete_agents(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    crate::extract::ApiJson(body): crate::extract::ApiJson<BulkLifecycleRequest>,
) -> Result<Json<BulkLifecycleResponse>, ApiError> {
    run_bulk(&state, &auth, body, Operation::DeleteAgent).await
}

#[utoipa::path(post, path = "/api/v1/skills/bulk/delete", tag = "skills", request_body = BulkLifecycleRequest, responses((status = 200, body = BulkLifecycleResponse)))]
pub async fn bulk_delete_skills(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    crate::extract::ApiJson(body): crate::extract::ApiJson<BulkLifecycleRequest>,
) -> Result<Json<BulkLifecycleResponse>, ApiError> {
    run_bulk(&state, &auth, body, Operation::DeleteSkill).await
}
