//! Worker launch lifecycle: calls to the optional `orb-launcher` service, launch status
//! refresh and the executions a terminated launch leaves behind.
//!
//! The API stores launch intent and status, but never receives Docker socket or
//! Kubernetes privileges. The launcher exposes a small internal HTTP surface.

use crate::error::ApiError;
use crate::state::AppState;
use axum::http::StatusCode;
use orbit_security::AuthContext;
use orbit_store::WorkerLaunch;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use uuid::Uuid;

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
    #[serde(default)]
    termination: Option<orbit_contracts::WorkerTermination>,
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

pub(crate) async fn call_launcher<T: for<'de> Deserialize<'de>>(
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

pub(crate) async fn refresh(state: &AppState, launch: WorkerLaunch) -> WorkerLaunch {
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
    if updated.status == "failed" {
        if let Some(termination) = launch_termination(&status, &updated) {
            fail_worker_executions(state, &updated, &termination).await;
        }
    }
    updated
}

/// Structured termination from the launcher, with a legacy OOM-message fallback for older
/// launchers that only report a human-readable error.
fn launch_termination(
    status: &LauncherStatus,
    launch: &WorkerLaunch,
) -> Option<orbit_contracts::WorkerTermination> {
    if let Some(termination) = status.termination.clone() {
        return Some(termination);
    }
    let oom = status
        .error
        .as_deref()
        .is_some_and(|error| error.to_lowercase().contains("oom"));
    if !oom {
        return None;
    }
    let name = status
        .runtime_id
        .clone()
        .or_else(|| launch.runtime_id.clone())
        .or_else(|| launch.worker_id.clone())
        .unwrap_or_else(|| "unknown-worker".into());
    Some(orbit_contracts::WorkerTermination::new(
        orbit_contracts::WorkerTerminationReason::WorkerOom,
        name,
    ))
}

/// A worker terminated by the runtime (OOM, eviction, exit or node loss) cannot report its
/// own execution failure. Fail every in-flight execution on that worker with the structured
/// reason, terminate the team step/run/queue without auto-retry, and audit the cause.
async fn fail_worker_executions(
    state: &AppState,
    launch: &WorkerLaunch,
    termination: &orbit_contracts::WorkerTermination,
) {
    let Some(worker_id) = launch.worker_id.as_deref() else {
        return;
    };
    let executions = match state
        .store
        .active_executions_for_worker(launch.org_id, worker_id)
        .await
    {
        Ok(executions) => executions,
        Err(error) => {
            tracing::warn!(%error, launch_id = %launch.id, "could not load executions for terminated launch");
            return;
        }
    };
    let message = launch
        .error
        .clone()
        .unwrap_or_else(|| termination.message());
    let mut data = serde_json::to_value(termination).unwrap_or_else(|_| serde_json::json!({}));
    if let Some(object) = data.as_object_mut() {
        object.insert(
            "code".into(),
            serde_json::json!(termination.reason.as_str()),
        );
        object.insert("message".into(), serde_json::json!(message));
        object.insert("workerId".into(), serde_json::json!(worker_id));
        object.insert("profileId".into(), serde_json::json!(launch.profile_id));
        object.insert("launchId".into(), serde_json::json!(launch.id));
        // Side effects may already have happened; an operator must decide to rerun.
        object.insert("retryable".into(), serde_json::json!(false));
    }
    for execution in executions {
        let finished = state
            .store
            .finish_execution(
                execution.id,
                orbit_domain::ExecutionStatus::Failed,
                None,
                Some(&data),
                None,
                execution.session_ref.as_deref(),
            )
            .await;
        let ended = match finished {
            Ok(Some(ended)) => ended,
            Ok(None) => {
                // Another path already ended it; still make sure the team run is closed.
                let mut ended = execution.clone();
                ended.status = orbit_domain::ExecutionStatus::Failed.as_str().into();
                ended.error = Some(data.clone());
                ended
            }
            Err(error) => {
                tracing::warn!(execution_id = %execution.id, %error, "could not fail execution for terminated worker");
                continue;
            }
        };
        crate::background::report_ended_execution(state, &ended, &message, &data).await;
        if let Err(error) = state
            .store
            .append_audit_log(
                launch.org_id,
                "system",
                None,
                None,
                termination.reason.audit_action(),
                Some("execution"),
                Some(execution.id),
                data.clone(),
                None,
                None,
            )
            .await
        {
            tracing::warn!(execution_id = %execution.id, %error, "failed to audit worker termination");
        }
    }
}

/// Background termination poller: refresh live launches so an OOM/eviction/exit is surfaced
/// even when nobody has the Workers page open. Returns false only when the store read fails.
pub async fn refresh_active_launches(state: &AppState) -> bool {
    if state.launcher_url.is_none() {
        return true;
    }
    let launches = match state
        .store
        .list_active_worker_launches_installation(200)
        .await
    {
        Ok(launches) => launches,
        Err(error) => {
            tracing::warn!(%error, "could not list live worker launches for termination polling");
            return false;
        }
    };
    let _ =
        futures::future::join_all(launches.into_iter().map(|launch| refresh(state, launch))).await;
    true
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
    // Count running workers installation-wide. Held through the launch insert so two
    // concurrent launches (manual, team run or dispatcher) cannot exceed the seat count.
    let worker_limit = state.worker_limit_guard().await?;
    let issued =
        crate::worker_tokens::issue_profile_token(state, auth.org_id, &profile, false).await?;
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
    drop(worker_limit);
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
    let launch_started = std::time::Instant::now();
    let launched = call_launcher(
        state,
        reqwest::Method::POST,
        "/launches",
        Some(serde_json::to_value(&request).unwrap_or_default()),
    )
    .await;
    state
        .metrics
        .worker_launch_request(launched.is_ok(), launch_started.elapsed());
    let result: LauncherStatus = match launched {
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
