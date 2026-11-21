//! Access team management (ADR 0011).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use orbit_domain::{is_valid_slug, ValidationIssue};
use orbit_security::{AuthContext, Permission};
use orbit_store::{AccessTeam, TeamMember};

use crate::audit;
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
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    access.require_view("team", None, Some(team_id), "org")?;
    state.store.get_access_team(auth.org_id, team_id).await?;
    Ok(Json(state.store.list_team_members(team_id).await?))
}

#[utoipa::path(post, path = "/api/v1/teams/{team_id}/members", tag = "teams", request_body = TeamMemberRequest, responses((status = 200, body = TeamMember)))]
pub async fn add_member(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(team_id): Path<Uuid>,
    crate::extract::ApiJson(body): crate::extract::ApiJson<TeamMemberRequest>,
) -> Result<Json<TeamMember>, ApiError> {
    auth.require(Permission::SettingsWrite)?;
    state.require_feature(orbit_security::FEATURE_TEAMS)?;
    state.store.get_access_team(auth.org_id, team_id).await?;
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
    auth.require(Permission::SettingsWrite)?;
    state.require_feature(orbit_security::FEATURE_TEAMS)?;
    state.store.get_access_team(auth.org_id, team_id).await?;
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
