//! Generic visibility sharing for shareable resources.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use uuid::Uuid;

use orbit_domain::ValidationIssue;
use orbit_security::{AuthContext, Permission};

use crate::audit;
use crate::error::ApiError;
use crate::state::AppState;

#[derive(Debug, Clone, Deserialize, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ShareRequest {
    pub visibility: String,
    #[serde(default)]
    pub owner_team_id: Option<Uuid>,
}

fn may_target_team(access: &crate::access::AccessScope, team_id: Uuid) -> bool {
    access.is_privileged() || access.teams.contains_key(&team_id)
}

/// A resource may only be handed to an access team the caller belongs to (owners and
/// admins, who bypass access checks, may pick any team). A team the caller is not in is
/// reported exactly like a missing one.
async fn require_target_team(
    state: &AppState,
    auth: &AuthContext,
    access: &crate::access::AccessScope,
    team_id: Uuid,
) -> Result<(), ApiError> {
    state.store.get_access_team(auth.org_id, team_id).await?;
    if may_target_team(access, team_id) {
        Ok(())
    } else {
        Err(ApiError::not_found("access team"))
    }
}

#[utoipa::path(patch, path = "/api/v1/sharing/{resource}/{resource_id}", tag = "sharing", request_body = ShareRequest, responses((status = 204)))]
pub async fn share(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path((resource, resource_id)): Path<(String, Uuid)>,
    crate::extract::ApiJson(body): crate::extract::ApiJson<ShareRequest>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::SettingsWrite)?;
    state.require_feature("resource_permissions")?;
    if !matches!(body.visibility.as_str(), "private" | "team" | "org") {
        return Err(ApiError::validation(vec![ValidationIssue::new(
            "visibility",
            "must be private, team or org",
        )]));
    }
    if body.visibility == "team" && body.owner_team_id.is_none() {
        return Err(ApiError::validation(vec![ValidationIssue::new(
            "ownerTeamId",
            "ownerTeamId is required for team visibility",
        )]));
    }
    let (owner_user, owner_team, visibility) = state
        .store
        .resource_ownership(auth.org_id, &resource, resource_id)
        .await?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    access.require_manage(&resource, owner_user, owner_team, &visibility)?;
    if let Some(team_id) = body.owner_team_id {
        require_target_team(&state, &auth, &access, team_id).await?;
    }
    state
        .store
        .set_resource_visibility(
            auth.org_id,
            &resource,
            resource_id,
            &body.visibility,
            body.owner_team_id,
        )
        .await?;
    audit::record(
        &state,
        &auth,
        "resource.shared",
        Some(&resource),
        Some(resource_id),
        serde_json::json!({
            "visibility": body.visibility,
            "ownerTeamId": body.owner_team_id,
        }),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(get, path = "/api/v1/sharing/{resource}/{resource_id}", tag = "sharing", responses((status = 200, body = ShareRequest)))]
pub async fn get(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path((resource, resource_id)): Path<(String, Uuid)>,
) -> Result<Json<ShareRequest>, ApiError> {
    auth.require(Permission::SettingsRead)?;
    let (owner_user, owner_team, visibility) = state
        .store
        .resource_ownership(auth.org_id, &resource, resource_id)
        .await?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    access.require_view(&resource, owner_user, owner_team, &visibility)?;
    Ok(Json(ShareRequest {
        visibility,
        owner_team_id: owner_team,
    }))
}

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BulkShareRequest {
    pub ids: Vec<Uuid>,
    #[serde(default)]
    pub visibility: Option<String>,
    #[serde(default)]
    pub owner_team_id: Option<Uuid>,
    /// Explicitly clear the owning team when no ownerTeamId is sent.
    #[serde(default)]
    pub clear_owner_team: bool,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BulkShareFailure {
    pub id: Uuid,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BulkShareResponse {
    pub updated: Vec<Uuid>,
    pub failed: Vec<BulkShareFailure>,
}

/// Bulk visibility/ownership update for agents, skills and other shareable resources.
///
/// Partial success is intentional: one resource the caller cannot manage must not block
/// the rest, and the response says exactly which ids failed.
#[utoipa::path(patch, path = "/api/v1/sharing/{resource}/bulk", tag = "sharing", request_body = BulkShareRequest, responses((status = 200, body = BulkShareResponse)))]
pub async fn bulk_share(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(resource): Path<String>,
    crate::extract::ApiJson(body): crate::extract::ApiJson<BulkShareRequest>,
) -> Result<Json<BulkShareResponse>, ApiError> {
    auth.require(Permission::SettingsWrite)?;
    state.require_feature("resource_permissions")?;
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
    if let Some(visibility) = &body.visibility {
        if !matches!(visibility.as_str(), "private" | "team" | "org") {
            return Err(ApiError::validation(vec![ValidationIssue::new(
                "visibility",
                "must be private, team or org",
            )]));
        }
    }
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    if let Some(team_id) = body.owner_team_id {
        require_target_team(&state, &auth, &access, team_id).await?;
    }
    let mut updated = Vec::new();
    let mut failed = Vec::new();
    for id in ids {
        let result: Result<(), ApiError> = async {
            let (owner_user, owner_team, current_visibility) = state
                .store
                .resource_ownership(auth.org_id, &resource, id)
                .await?;
            access.require_manage(&resource, owner_user, owner_team, &current_visibility)?;
            let visibility = body
                .visibility
                .clone()
                .unwrap_or_else(|| current_visibility.clone());
            let owner_team_id = if body.clear_owner_team {
                None
            } else {
                body.owner_team_id.or(owner_team)
            };
            if visibility == "team" && owner_team_id.is_none() {
                return Err(ApiError::validation(vec![ValidationIssue::new(
                    "ownerTeamId",
                    "ownerTeamId is required for team visibility",
                )]));
            }
            state
                .store
                .set_resource_visibility(auth.org_id, &resource, id, &visibility, owner_team_id)
                .await?;
            Ok(())
        }
        .await;
        match result {
            Ok(()) => updated.push(id),
            Err(error) => failed.push(BulkShareFailure {
                id,
                message: error.message,
            }),
        }
    }
    audit::record(
        &state,
        &auth,
        "resource.bulk_shared",
        Some(&resource),
        None,
        serde_json::json!({
            "updated": updated.len(),
            "failed": failed.len(),
            "visibility": body.visibility,
            "ownerTeamId": body.owner_team_id,
            "clearOwnerTeam": body.clear_owner_team,
        }),
    )
    .await;
    Ok(Json(BulkShareResponse { updated, failed }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::AccessScope;
    use orbit_domain::UserRole;

    fn scope(role: UserRole, teams: &[Uuid]) -> AccessScope {
        AccessScope {
            user_id: Uuid::now_v7(),
            role,
            teams: teams.iter().map(|id| (*id, "member".to_string())).collect(),
            team_isolation: true,
        }
    }

    #[test]
    fn owner_team_must_be_one_the_caller_belongs_to_unless_privileged() {
        let mine = Uuid::now_v7();
        let other = Uuid::now_v7();
        let member = scope(UserRole::Viewer, &[mine]);
        assert!(may_target_team(&member, mine));
        assert!(!may_target_team(&member, other));
        assert!(may_target_team(&scope(UserRole::Owner, &[]), other));
        assert!(may_target_team(&scope(UserRole::Admin, &[]), other));
    }
}
