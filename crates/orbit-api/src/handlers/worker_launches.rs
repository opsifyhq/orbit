//! Launch/stop worker instances through the optional `orb-launcher` service.
//!
//! The API stores launch intent and status, but never receives Docker socket or
//! Kubernetes privileges. The launcher exposes a small internal HTTP surface.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use orbit_security::{AuthContext, Permission};
use orbit_store::WorkerLaunch;

use crate::error::ApiError;
use crate::launches::{call_launcher, launch_profile_internal, refresh, stop_launch_internal};
use crate::state::AppState;

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LaunchLogs {
    pub logs: String,
}

/// A launch is exactly as visible as the worker profile it was launched from. Launches
/// whose profile no longer exists are visible to owners/admins only.
fn launch_visible(
    access: &crate::access::AccessScope,
    profiles: &std::collections::HashMap<Uuid, orbit_store::WorkerProfile>,
    launch: &WorkerLaunch,
) -> bool {
    match profiles.get(&launch.profile_id) {
        Some(profile) => access.can_view(
            profile.owner_user_id,
            profile.owner_team_id,
            &profile.visibility,
        ),
        None => access.is_privileged(),
    }
}

async fn load_visible_launch(
    state: &AppState,
    auth: &AuthContext,
    launch_id: Uuid,
) -> Result<WorkerLaunch, ApiError> {
    let launch = state
        .store
        .get_worker_launch(auth.org_id, launch_id)
        .await?;
    let access = crate::access::AccessScope::load(state, auth).await?;
    let profiles = state
        .store
        .list_worker_profiles(auth.org_id, None)
        .await?
        .into_iter()
        .map(|profile| (profile.id, profile))
        .collect();
    if !launch_visible(&access, &profiles, &launch) {
        return Err(ApiError::not_found("worker launch"));
    }
    Ok(launch)
}

/// Like [`load_visible_launch`], but the caller must also be allowed to manage the
/// launch's worker profile (stop, delete, archive).
async fn load_managed_launch(
    state: &AppState,
    auth: &AuthContext,
    launch_id: Uuid,
) -> Result<WorkerLaunch, ApiError> {
    let launch = load_visible_launch(state, auth, launch_id).await?;
    let access = crate::access::AccessScope::load(state, auth).await?;
    if access.is_privileged() {
        return Ok(launch);
    }
    // `load_visible_launch` already hid launches whose profile is gone from non-privileged
    // callers, so the profile exists here.
    let profile = state
        .store
        .get_worker_profile(auth.org_id, launch.profile_id)
        .await?;
    access.require_manage(
        "worker launch",
        profile.owner_user_id,
        profile.owner_team_id,
        &profile.visibility,
    )?;
    Ok(launch)
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerLaunchesQuery {
    #[serde(default)]
    pub include_inactive: bool,
}

#[utoipa::path(get, path = "/api/v1/worker-launches", tag = "workers", responses((status = 200, body = [WorkerLaunch])))]
pub async fn list_launches(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<WorkerLaunchesQuery>,
) -> Result<Json<Vec<WorkerLaunch>>, ApiError> {
    auth.require(Permission::HarnessesRead)?;
    let launches = if query.include_inactive {
        state.store.list_worker_launches(auth.org_id, None).await?
    } else {
        state.store.list_active_worker_launches(auth.org_id).await?
    };
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    let profiles = state
        .store
        .list_worker_profiles(auth.org_id, None)
        .await?
        .into_iter()
        .map(|profile| (profile.id, profile))
        .collect();
    let launches: Vec<WorkerLaunch> = launches
        .into_iter()
        .filter(|launch| launch_visible(&access, &profiles, launch))
        .collect();
    // Each refresh is a launcher call with its own timeout; run them together so a slow or
    // unreachable launcher costs one timeout, not one per launch (#306).
    let refreshed =
        futures::future::join_all(launches.into_iter().map(|launch| refresh(&state, launch))).await;
    Ok(Json(refreshed))
}

#[utoipa::path(get, path = "/api/v1/worker-launches/{launch_id}", tag = "workers", responses((status = 200, body = WorkerLaunch)))]
pub async fn get_launch(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(launch_id): Path<Uuid>,
) -> Result<Json<WorkerLaunch>, ApiError> {
    auth.require(Permission::HarnessesRead)?;
    let launch = load_visible_launch(&state, &auth, launch_id).await?;
    Ok(Json(refresh(&state, launch).await))
}

#[utoipa::path(post, path = "/api/v1/worker-profiles/{profile_id}/launch", tag = "workers", responses((status = 201, body = WorkerLaunch)))]
pub async fn launch_profile(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(profile_id): Path<Uuid>,
) -> Result<(StatusCode, Json<WorkerLaunch>), ApiError> {
    auth.require(Permission::SettingsWrite)?;
    Ok((
        StatusCode::CREATED,
        Json(launch_profile_internal(&state, &auth, profile_id, None).await?),
    ))
}

#[utoipa::path(post, path = "/api/v1/worker-launches/{launch_id}/stop", tag = "workers", responses((status = 200, body = WorkerLaunch)))]
pub async fn stop_launch(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(launch_id): Path<Uuid>,
) -> Result<Json<WorkerLaunch>, ApiError> {
    auth.require(Permission::SettingsWrite)?;
    let launch = load_managed_launch(&state, &auth, launch_id).await?;
    Ok(Json(stop_launch_internal(&state, &auth, launch).await?))
}

#[utoipa::path(get, path = "/api/v1/worker-launches/{launch_id}/logs", tag = "workers", responses((status = 200, body = LaunchLogs)))]
pub async fn launch_logs(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(launch_id): Path<Uuid>,
) -> Result<Json<LaunchLogs>, ApiError> {
    auth.require(Permission::HarnessesRead)?;
    let launch = load_visible_launch(&state, &auth, launch_id).await?;
    let logs: LaunchLogs = call_launcher(
        &state,
        reqwest::Method::GET,
        &format!("/launches/{launch_id}/logs?target={}", launch.target),
        None,
    )
    .await?;
    Ok(Json(logs))
}

#[utoipa::path(delete, path = "/api/v1/worker-launches/{launch_id}", tag = "workers", responses((status = 204)))]
pub async fn delete_launch(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(launch_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::SettingsWrite)?;
    load_managed_launch(&state, &auth, launch_id).await?;
    state
        .store
        .delete_worker_launch(auth.org_id, launch_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/api/v1/worker-launches/{launch_id}/archive", tag = "workers", responses((status = 200, body = WorkerLaunch)))]
pub async fn archive_launch(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(launch_id): Path<Uuid>,
) -> Result<Json<WorkerLaunch>, ApiError> {
    auth.require(Permission::SettingsWrite)?;
    load_managed_launch(&state, &auth, launch_id).await?;
    Ok(Json(
        state
            .store
            .archive_worker_launch(auth.org_id, launch_id)
            .await?,
    ))
}
