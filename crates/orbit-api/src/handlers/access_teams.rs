//! Access team management (ADR 0011).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use orbit_domain::{is_valid_slug, ValidationIssue};
use orbit_security::{AuthContext, Permission};
use orbit_store::{AccessTeam, TeamMember};

use crate::access::AccessScope;
use crate::audit;
use crate::dto::{
    TeamAccessSummary, TeamResourceCounts as TeamResourceCountsDto, TeamResourceItem,
    TeamResourceQuery, TeamResourcesPage, TeamRoleCounts,
};
use crate::error::ApiError;
use crate::state::AppState;

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TeamRequest {
    pub name: String,
    pub slug: String,
}

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TeamMemberRequest {
    pub user_id: Uuid,
    #[serde(default = "default_member_role")]
    pub role: String,
}

fn default_member_role() -> String {
    "member".into()
}

/// Load a team only when the caller is an org owner/admin or a member of that team.
/// Everyone else gets the same 404 as a missing team: membership is not public.
async fn load_visible_team(
    state: &AppState,
    auth: &AuthContext,
    team_id: Uuid,
) -> Result<(AccessTeam, AccessScope), ApiError> {
    let team = state.store.get_access_team(auth.org_id, team_id).await?;
    let access = AccessScope::load(state, auth).await?;
    if !access.is_privileged() && !access.teams.contains_key(&team_id) {
        return Err(ApiError::not_found("team"));
    }
    Ok((team, access))
}

/// Team membership and access changes are reserved for leads and org owners/admins.
fn require_team_manage(access: &AccessScope, team_id: Uuid) -> Result<(), ApiError> {
    if access.is_privileged()
        || access
            .teams
            .get(&team_id)
            .is_some_and(|role| role == "lead")
    {
        Ok(())
    } else {
        Err(ApiError::forbidden(
            "only team leads and organization owners/admins can change team access",
        ))
    }
}

fn validated(body: &TeamRequest) -> Result<(String, String), ApiError> {
    let name = body.name.trim().to_string();
    let slug = body.slug.trim().to_lowercase();
    let mut issues = Vec::new();
    if name.is_empty() {
        issues.push(ValidationIssue::new("name", "name is required"));
    }
    if !is_valid_slug(&slug) {
        issues.push(ValidationIssue::new(
            "slug",
            "lowercase letters, digits, - or _",
        ));
    }
    if !issues.is_empty() {
        return Err(ApiError::validation(issues));
    }
    Ok((name, slug))
}

#[utoipa::path(get, path = "/api/v1/teams", tag = "teams", responses((status = 200, body = [AccessTeam])))]
pub async fn list(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> Result<Json<Vec<AccessTeam>>, ApiError> {
    auth.require(Permission::SettingsRead)?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    let teams = state
        .store
        .list_access_teams(auth.org_id)
        .await?
        .into_iter()
        .filter(|team| {
            access.can_view(None, Some(team.id), "org") || access.teams.contains_key(&team.id)
        })
        .collect();
    Ok(Json(teams))
}

#[utoipa::path(post, path = "/api/v1/teams", tag = "teams", request_body = TeamRequest, responses((status = 201, body = AccessTeam)))]
pub async fn create(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    crate::extract::ApiJson(body): crate::extract::ApiJson<TeamRequest>,
) -> Result<(StatusCode, Json<AccessTeam>), ApiError> {
    auth.require(Permission::SettingsWrite)?;
    state.require_feature(orbit_security::FEATURE_TEAMS)?;
    let (name, slug) = validated(&body)?;
    let team = state
        .store
        .create_access_team(auth.org_id, &name, &slug, Some(auth.user_id))
        .await?;
    // The creator leads the team.
    let _ = state
        .store
        .upsert_team_member(team.id, auth.user_id, "lead")
        .await;
    audit::record(
        &state,
        &auth,
        "team.created",
        Some("team"),
        Some(team.id),
        serde_json::json!({ "name": team.name, "slug": team.slug }),
    )
    .await;
    Ok((StatusCode::CREATED, Json(team)))
}

#[utoipa::path(patch, path = "/api/v1/teams/{team_id}", tag = "teams", request_body = TeamRequest, responses((status = 200, body = AccessTeam)))]
pub async fn update(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(team_id): Path<Uuid>,
    crate::extract::ApiJson(body): crate::extract::ApiJson<TeamRequest>,
) -> Result<Json<AccessTeam>, ApiError> {
    auth.require(Permission::SettingsWrite)?;
    state.require_feature(orbit_security::FEATURE_TEAMS)?;
    state.store.get_access_team(auth.org_id, team_id).await?;
    let (name, slug) = validated(&body)?;
    let team = state
        .store
        .update_access_team(auth.org_id, team_id, &name, &slug)
        .await?;
    audit::record(
        &state,
        &auth,
        "team.updated",
        Some("team"),
        Some(team.id),
        serde_json::json!({ "name": team.name, "slug": team.slug }),
    )
    .await;
    Ok(Json(team))
}

#[utoipa::path(delete, path = "/api/v1/teams/{team_id}", tag = "teams", responses((status = 204)))]
pub async fn delete(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(team_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::SettingsWrite)?;
    state.require_feature(orbit_security::FEATURE_TEAMS)?;
    let team = state.store.get_access_team(auth.org_id, team_id).await?;
    if team.slug == "default" {
        return Err(ApiError::conflict("the Default team cannot be deleted"));
    }
    state.store.delete_access_team(auth.org_id, team_id).await?;
    audit::record(
        &state,
        &auth,
        "team.deleted",
        Some("team"),
        Some(team_id),
        serde_json::json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(get, path = "/api/v1/teams/{team_id}/members", tag = "teams", responses((status = 200, body = [TeamMember])))]
pub async fn list_members(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(team_id): Path<Uuid>,
) -> Result<Json<Vec<TeamMember>>, ApiError> {
    auth.require(Permission::MembersRead)?;
    load_visible_team(&state, &auth, team_id).await?;
    Ok(Json(state.store.list_team_members(team_id).await?))
}

#[utoipa::path(post, path = "/api/v1/teams/{team_id}/members", tag = "teams", request_body = TeamMemberRequest, responses((status = 200, body = TeamMember)))]
pub async fn add_member(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(team_id): Path<Uuid>,
    crate::extract::ApiJson(body): crate::extract::ApiJson<TeamMemberRequest>,
) -> Result<Json<TeamMember>, ApiError> {
    auth.require(Permission::MembersRead)?;
    let (_, access) = load_visible_team(&state, &auth, team_id).await?;
    require_team_manage(&access, team_id)?;
    state.require_feature(orbit_security::FEATURE_TEAMS)?;
    // `load_visible_team` already checked the team; main also validates the target user.
    state.store.get_user(auth.org_id, body.user_id).await?;
    let member = state
        .store
        .upsert_team_member(team_id, body.user_id, &body.role)
        .await?;
    audit::record(
        &state,
        &auth,
        "team.member_changed",
        Some("team"),
        Some(team_id),
        serde_json::json!({ "userId": body.user_id, "role": member.role }),
    )
    .await;
    Ok(Json(member))
}

#[utoipa::path(delete, path = "/api/v1/teams/{team_id}/members/{user_id}", tag = "teams", responses((status = 204)))]
pub async fn remove_member(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path((team_id, user_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::MembersRead)?;
    let (_, access) = load_visible_team(&state, &auth, team_id).await?;
    require_team_manage(&access, team_id)?;
    state.require_feature(orbit_security::FEATURE_TEAMS)?;
    state.store.remove_team_member(team_id, user_id).await?;
    audit::record(
        &state,
        &auth,
        "team.member_changed",
        Some("team"),
        Some(team_id),
        serde_json::json!({ "userId": user_id, "removed": true }),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

/// Access overview for one access team. The whole page (summary + first page of resources)
/// is one request; every resource is filtered by the caller's `AccessScope` before it is
/// returned, so a team member never sees another team's resources through this endpoint.
#[utoipa::path(
    get,
    path = "/api/v1/teams/{team_id}/resources",
    tag = "teams",
    params(TeamResourceQuery),
    responses((status = 200, body = TeamResourcesPage))
)]
pub async fn resources(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(team_id): Path<Uuid>,
    Query(query): Query<TeamResourceQuery>,
) -> Result<Json<TeamResourcesPage>, ApiError> {
    auth.require(Permission::MembersRead)?;
    if let Some(kind) = &query.resource_type {
        if !orbit_store::sharing::SHAREABLE_RESOURCES.contains(&kind.as_str()) {
            return Err(ApiError::validation(vec![ValidationIssue::new(
                "type",
                format!("unknown resource type '{kind}'"),
            )]));
        }
    }
    let (_, access) = load_visible_team(&state, &auth, team_id).await?;
    let store_access = access.store_access();
    let limit = crate::handlers::limit(query.limit);
    let (rows, next_cursor, total) = state
        .store
        .list_team_resources(
            auth.org_id,
            team_id,
            query.resource_type.as_deref(),
            limit,
            query.cursor.as_deref(),
            Some(&store_access),
        )
        .await?;
    let counts = state
        .store
        .count_team_resources(auth.org_id, team_id, Some(&store_access))
        .await?;

    let mut roles = TeamRoleCounts::default();
    for member in state.store.list_team_members(team_id).await? {
        match member.role.as_str() {
            "lead" => roles.lead += 1,
            "member" => roles.member += 1,
            "viewer" => roles.viewer += 1,
            _ => {}
        }
    }

    let items = rows
        .into_iter()
        .map(|row| {
            let owner_user = row.owner_user_id;
            let owner_team = row.owner_team_id;
            let visibility = row.visibility.as_str();
            let mut effective_access = Vec::with_capacity(3);
            if access.can_view(owner_user, owner_team, visibility) {
                effective_access.push("view".to_string());
            }
            if access.can_use(owner_user, owner_team, visibility) {
                effective_access.push("use".to_string());
            }
            if access.can_manage(owner_user, owner_team) {
                effective_access.push("manage".to_string());
            }
            TeamResourceItem {
                id: row.id,
                resource_type: row.resource_type,
                name: row.name,
                owner_name: row.owner_name,
                owner_team_id: owner_team,
                is_owner_team: owner_team == Some(team_id),
                visibility: row.visibility,
                effective_access,
                updated_at: row.updated_at,
            }
        })
        .collect();

    Ok(Json(TeamResourcesPage {
        items,
        next_cursor,
        total,
        summary: TeamAccessSummary {
            roles,
            resources: TeamResourceCountsDto {
                total: counts.total,
                agents: counts.agents,
                skills: counts.skills,
                connections: counts.connections,
                providers: counts.providers,
                worker_profiles: counts.worker_profiles,
                agent_teams: counts.agent_teams,
                triggers: counts.triggers,
                tool_policies: counts.tool_policies,
                env_mappings: counts.env_mappings,
            },
            org_visible_count: counts.org_visible,
            can_manage_team: require_team_manage(&access, team_id).is_ok(),
        },
    }))
}
