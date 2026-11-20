//! Launch/stop worker instances through the optional `orb-launcher` service.
//!
//! The API stores launch intent and status, but never receives Docker socket or
//! Kubernetes privileges. The launcher exposes a small internal HTTP surface.

use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use orbit_security::{AuthContext, Permission};
use orbit_store::WorkerLaunch;

use crate::error::ApiError;
use crate::state::AppState;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LauncherRequest {
    launch_id: Uuid,
    profile_id: Uuid,
    name: String,
    harness_kind: String,
    target: String,
    execution_driver: String,
    image: String,
    image_tag: String,
    auth_secret_name: Option<String>,
    env: serde_json::Value,
    resources: serde_json::Value,
    api_url: String,
    worker_token: String,
    worker_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    ca_secret_name: Option<String>,
    ca_secret_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    single_execution_id: Option<Uuid>,
    /// Execution timeout; the launcher derives the Job `activeDeadlineSeconds` from it.
    #[serde(skip_serializing_if = "Option::is_none")]
    timeout_seconds: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LauncherStatus {
    status: String,
    #[serde(default)]
    runtime_id: Option<String>,
    #[serde(default)]
    worker_id: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LaunchLogs {
    pub logs: String,
}

fn launcher(state: &AppState) -> Result<(&str, &str), ApiError> {
    match (&state.launcher_url, &state.launcher_token) {
        (Some(url), Some(token)) => Ok((url.trim_end_matches('/'), token)),
        _ => Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "launcher_unavailable",
            "no worker launcher is configured for this deployment",
        )),
    }
}

async fn call_launcher<T: for<'de> Deserialize<'de>>(
    state: &AppState,
    method: reqwest::Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> Result<T, ApiError> {
    let (url, token) = launcher(state)?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|error| ApiError::internal(error.to_string()))?;
    let mut request = client
        .request(method, format!("{url}{path}"))
        .bearer_auth(token);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await.map_err(|error| {
        ApiError::new(
            StatusCode::BAD_GATEWAY,
            "launcher_error",
            format!("worker launcher request failed: {error}"),
        )
    })?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            "launcher_error",
            format!("worker launcher returned {status}: {body}"),
        ));
    }
    response.json().await.map_err(|error| {
        ApiError::new(StatusCode::BAD_GATEWAY, "launcher_error", error.to_string())
    })
}

async fn refresh(state: &AppState, launch: WorkerLaunch) -> WorkerLaunch {
    if !matches!(launch.status.as_str(), "pending" | "starting" | "running") {
        return launch;
    }
    let (url, token) = match (&state.launcher_url, &state.launcher_token) {
        (Some(url), Some(token)) => (url.trim_end_matches('/'), token),
        _ => return launch,
    };
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(4))
        .build()
    {
        Ok(client) => client,
        Err(_) => return launch,
    };
    let response = client
        .get(format!(
            "{url}/launches/{}?target={}",
            launch.id, launch.target
        ))
        .bearer_auth(token)
        .send()
        .await;
    let Ok(response) = response else {
        return launch;
    };
    let Ok(status) = response.json::<LauncherStatus>().await else {
        return launch;
    };
    let stopped = matches!(status.status.as_str(), "stopped" | "failed");
    let updated = state
        .store
        .update_worker_launch(
            launch.org_id,
            launch.id,
            &status.status,
            status.runtime_id.as_deref(),
            status.worker_id.as_deref(),
            status.error.as_deref(),
            status.status == "running",
            stopped,
        )
        .await
        .unwrap_or(launch);
    if updated.status == "failed"
        && updated
            .error
            .as_deref()
            .is_some_and(|error| error.to_lowercase().contains("oom"))
    {
        fail_worker_executions(state, &updated).await;
    }
    updated
}

/// A worker OOM kill takes the process down before it can report the execution failure.
/// Fail any running executions on that worker with a clear error so they are not left
/// running until timeout.
async fn fail_worker_executions(state: &AppState, launch: &WorkerLaunch) {
    let Some(worker_id) = launch.worker_id.as_deref() else {
        return;
    };
    let executions = match state
        .store
        .running_executions_for_worker(launch.org_id, worker_id)
        .await
    {
        Ok(executions) => executions,
        Err(error) => {
            tracing::warn!(%error, launch_id = %launch.id, "could not load executions for OOM launch");
            return;
        }
    };
    for execution in executions {
        let message = launch
            .error
            .clone()
            .unwrap_or_else(|| "worker container was OOMKilled".into());
        if let Err(err) = state
            .store
            .append_execution_event(
                launch.org_id,
                execution.id,
                "worker_oom",
                orbit_domain::LogLevel::Error,
                &message,
                &serde_json::json!({
                    "code": "worker_oom",
                    "workerId": worker_id,
                    "profileId": launch.profile_id,
                    "launchId": launch.id,
                    "error": message,
                }),
            )
            .await
        {
            tracing::warn!(execution_id = %execution.id, error = %err, "failed to append execution event");
        }
        let _ = state
            .store
            .finish_execution(
                execution.id,
                orbit_domain::ExecutionStatus::Failed,
                None,
                Some(&serde_json::json!({ "code": "worker_oom", "message": message })),
                None,
                execution.session_ref.as_deref(),
            )
            .await;
    }
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
    let mut refreshed = Vec::with_capacity(launches.len());
    for launch in launches {
        refreshed.push(refresh(&state, launch).await);
    }
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
pub(crate) async fn launch_profile_internal(
    state: &AppState,
    auth: &AuthContext,
    profile_id: Uuid,
    single_execution_id: Option<Uuid>,
) -> Result<WorkerLaunch, ApiError> {
    launcher(state)?;
    let profile = state
        .store
        .get_worker_profile(auth.org_id, profile_id)
        .await?;
    let access = crate::access::AccessScope::load(state, auth).await?;
    access.require_view(
        "worker profile",
        profile.owner_user_id,
        profile.owner_team_id,
        &profile.visibility,
    )?;
    if !profile.enabled {
        return Err(ApiError::conflict(
            "enable the worker profile before launching it",
        ));
    }
    // Reuse an existing live launch for this profile instead of creating a new worker
    // for every TeamRun. Single-execution launches are always isolated.
    if single_execution_id.is_none() {
        if let Some(existing) = state
            .store
            .find_live_launch_for_profile(auth.org_id, profile.id)
            .await?
        {
            return Ok(existing);
        }
    }
    let issued =
        crate::handlers::worker_profiles::issue_profile_token(state, auth.org_id, &profile, false)
            .await?;
    let image_tag = if profile.image_tag.trim().is_empty() {
        crate::images::image_tag()
    } else {
        profile.image_tag.clone()
    };
    let image = format!("{}:{}", profile.image, image_tag);
    let launch = match state
        .store
        .create_worker_launch(
            auth.org_id,
            profile.id,
            &profile.name,
            &profile.target,
            &image,
            Some(issued.token_id),
            Some(auth.user_id),
            single_execution_id.is_some(),
        )
        .await
    {
        Ok(launch) => launch,
        Err(error) => {
            let _ = state
                .store
                .revoke_worker_token(auth.org_id, issued.token_id)
                .await;
            // Lost the race against a concurrent launch of the same profile: reuse the winner.
            if error.is_unique_violation() && single_execution_id.is_none() {
                if let Some(existing) = state
                    .store
                    .find_live_launch_for_profile(auth.org_id, profile.id)
                    .await?
                {
                    return Ok(existing);
                }
            }
            return Err(error.into());
        }
    };
    let worker_id = launch_worker_id(launch.id);
    let mut timeout_seconds = None;
    if let Some(execution_id) = single_execution_id {
        // Lets the background reaper revoke this launch's token when the execution ends.
        state
            .store
            .set_worker_launch_single_execution(launch.id, execution_id)
            .await?;
        // Claim and token binding are one statement: a failure between them would leave a
        // dispatched execution with a NULL token, which `ensure_execution` treats as a
        // legacy claim that any profile-scoped token can access.
        let claimed = state
            .store
            .claim_execution_for_launch(execution_id, &worker_id, issued.token_id)
            .await;
        let claimed = match claimed {
            Ok(Some(claimed)) => claimed,
            other => {
                // Do not leave a phantom pending launch or an active token behind.
                let _ = state
                    .store
                    .revoke_worker_token(auth.org_id, issued.token_id)
                    .await;
                let _ = state
                    .store
                    .update_worker_launch(
                        auth.org_id,
                        launch.id,
                        "failed",
                        None,
                        None,
                        Some("execution was no longer queued for this launch"),
                        false,
                        true,
                    )
                    .await;
                other?;
                return Err(ApiError::conflict(
                    "execution is no longer queued for this launch",
                ));
            }
        };
        timeout_seconds = Some(claimed.timeout_seconds.max(1) as u64);
    }
    let request = LauncherRequest {
        launch_id: launch.id,
        profile_id: profile.id,
        name: profile.name.clone(),
        harness_kind: profile.harness_kind.clone(),
        target: profile.target.clone(),
        execution_driver: profile.execution_driver.clone(),
        image: profile.image.clone(),
        image_tag,
        auth_secret_name: profile.auth_secret_name.clone(),
        env: profile.env.clone(),
        resources: profile.resources.clone(),
        api_url: state.worker_api_url.clone(),
        worker_token: issued.token,
        worker_id: worker_id.clone(),
        ca_secret_name: state.ca_secret_name.clone(),
        ca_secret_key: state.ca_secret_key.clone(),
        single_execution_id,
        timeout_seconds,
    };
    let result: LauncherStatus = match call_launcher(
        state,
        reqwest::Method::POST,
        "/launches",
        Some(serde_json::to_value(&request).unwrap_or_default()),
    )
    .await
    {
        Ok(result) => result,
        Err(error) => {
            let _ = state
                .store
                .revoke_worker_token(auth.org_id, issued.token_id)
                .await;
            let _ = state
                .store
                .update_worker_launch(
                    auth.org_id,
                    launch.id,
                    "failed",
                    None,
                    None,
                    Some(&error.message),
                    false,
                    true,
                )
                .await;
            if let Some(execution_id) = single_execution_id {
                let _ = state
                    .store
                    .finish_execution(
                        execution_id,
                        orbit_domain::ExecutionStatus::Failed,
                        None,
                        Some(&serde_json::json!({
                            "code": "launch_failed",
                            "message": error.message
                        })),
                        None,
                        None,
                    )
                    .await;
            }
            return Err(error);
        }
    };
    state
        .store
        .update_worker_launch(
            auth.org_id,
            launch.id,
            &result.status,
            result.runtime_id.as_deref(),
            Some(result.worker_id.as_deref().unwrap_or(&worker_id)),
            result.error.as_deref(),
            true,
            false,
        )
        .await
        .map_err(Into::into)
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
    let launch = state
        .store
        .get_worker_launch(auth.org_id, launch_id)
        .await?;
    Ok(Json(stop_launch_internal(&state, &auth, launch).await?))
}

pub(crate) async fn stop_launch_internal(
    state: &AppState,
    auth: &AuthContext,
    launch: WorkerLaunch,
) -> Result<WorkerLaunch, ApiError> {
    let _: serde_json::Value = call_launcher(
        state,
        reqwest::Method::POST,
        &format!("/launches/{}/stop?target={}", launch.id, launch.target),
        Some(serde_json::json!({})),
    )
    .await?;
    if let Some(token_id) = launch.worker_token_id {
        let _ = state.store.revoke_worker_token(auth.org_id, token_id).await;
    }
    Ok(state
        .store
        .update_worker_launch(
            auth.org_id,
            launch.id,
            "stopped",
            None,
            None,
            None,
            false,
            true,
        )
        .await?)
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
    Ok(Json(
        state
            .store
            .archive_worker_launch(auth.org_id, launch_id)
            .await?,
    ))
}

/// Worker id for a launch: `worker-` plus the full UUID without hyphens. The first 8 hex
/// chars of a UUIDv7 are timestamp bits shared by every launch within ~65 s (issue #106).
pub(crate) fn launch_worker_id(launch_id: uuid::Uuid) -> String {
    format!("worker-{}", launch_id.simple())
}

#[cfg(test)]
mod worker_id_tests {
    use super::launch_worker_id;

    #[test]
    fn back_to_back_launches_get_distinct_worker_ids() {
        let a = uuid::Uuid::now_v7();
        let b = uuid::Uuid::now_v7();
        assert_ne!(launch_worker_id(a), launch_worker_id(b));
        assert!(launch_worker_id(a).len() <= 63);
    }
}
