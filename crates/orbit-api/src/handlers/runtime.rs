//! Continuous agent runtime control (24/7 loop).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use orbit_security::{AuthContext, Permission};
use orbit_store::agent_runtimes::AgentRuntime;

use crate::audit;
use crate::error::ApiError;
use crate::state::AppState;

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StartRuntimeRequest {
    #[serde(default)]
    pub harness: Option<String>,
    #[serde(default)]
    pub task: Option<String>,
    /// Delay between monitor runs. Default 60 seconds, minimum 5.
    #[serde(default)]
    pub interval_seconds: Option<u32>,
}

async fn managed_agent(
    state: &AppState,
    auth: &AuthContext,
    agent_id: Uuid,
) -> Result<
    (
        orbit_store::Agent,
        orbit_domain::AgentSpec,
        orbit_store::AgentVersion,
    ),
    ApiError,
> {
    let agent = state.store.get_agent(auth.org_id, agent_id).await?;
    let access = crate::access::AccessScope::load(state, auth).await?;
    access.require_manage(
        "agent",
        agent.owner_user_id,
        agent.owner_team_id,
        &agent.visibility,
    )?;
    let version = state.store.get_current_version(auth.org_id, &agent).await?;
    let spec: orbit_domain::AgentSpec = serde_json::from_value(version.spec.clone())
        .map_err(|error| ApiError::internal(format!("stored agent spec is invalid: {error}")))?;
    Ok((agent, spec, version))
}

#[utoipa::path(post, path = "/api/v1/agents/{agent_id}/runtime", tag = "runtime", request_body = StartRuntimeRequest, responses(
    (status = 201, body = AgentRuntime, description = "A new continuous runtime was started"),
    (status = 200, body = AgentRuntime, description = "A continuous runtime was already running; the existing runtime is returned")
))]
pub async fn start(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(agent_id): Path<Uuid>,
    crate::extract::ApiJson(body): crate::extract::ApiJson<StartRuntimeRequest>,
) -> Result<(StatusCode, Json<AgentRuntime>), ApiError> {
    auth.require(Permission::ExecutionsCreate)?;
    let (agent, spec, _) = managed_agent(&state, &auth, agent_id).await?;
    if agent.status != "active" {
        return Err(ApiError::conflict(
            "only active agents can run continuously",
        ));
    }
    if let Some(existing) = state.store.get_agent_runtime(auth.org_id, agent_id).await? {
        return Ok((StatusCode::OK, Json(existing)));
    }
    let harness = match body.harness.as_deref() {
        Some(value) => value.parse::<orbit_domain::HarnessKind>().map_err(|_| {
            ApiError::validation(vec![orbit_domain::ValidationIssue::new(
                "harness",
                "unknown harness",
            )])
        })?,
        None => match spec.harness.as_ref().map(|harness| harness.kind) {
            Some(kind) => kind,
            None => state.default_harness().await.ok_or_else(|| {
                ApiError::validation(vec![orbit_domain::ValidationIssue::new(
                    "harness",
                    "no harness and no organization default is configured",
                )])
            })?,
        },
    };
    // Shared execution preflight (#140): the same checks every run of this runtime will
    // face, so a runtime that could never start is rejected up front.
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    crate::preflight::Preflight {
        state: &state,
        org_id: auth.org_id,
        access: &access,
        agent: &agent,
        spec: &spec,
        harness,
        extra_connection_ids: &[],
    }
    .run()
    .await?;
    let task = body
        .task
        .as_deref()
        .map(str::trim)
        .filter(|task| !task.is_empty())
        .map(str::to_string)
        .or_else(|| spec.run_defaults.task.clone())
        .unwrap_or_else(|| "Continue with your configured task.".into());
    let interval_seconds = body.interval_seconds.unwrap_or(60).clamp(5, 86_400);
    let runtime = state
        .store
        .create_agent_runtime(
            auth.org_id,
            agent_id,
            &task,
            harness.as_str(),
            interval_seconds as i32,
            Some(auth.user_id),
        )
        .await?;
    audit::record(
        &state,
        &auth,
        "agent.runtime_started",
        Some("agent"),
        Some(agent_id),
        serde_json::json!({ "runtimeId": runtime.id, "harness": harness.as_str() }),
    )
    .await;
    Ok((StatusCode::CREATED, Json(runtime)))
}

#[utoipa::path(get, path = "/api/v1/agents/{agent_id}/runtime", tag = "runtime", responses((status = 200, body = AgentRuntime)))]
pub async fn get(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(agent_id): Path<Uuid>,
) -> Result<Json<AgentRuntime>, ApiError> {
    auth.require(Permission::ExecutionsRead)?;
    let _ = managed_agent(&state, &auth, agent_id).await?;
    Ok(Json(
        state
            .store
            .get_agent_runtime(auth.org_id, agent_id)
            .await?
            .ok_or_else(|| ApiError::not_found("agent runtime"))?,
    ))
}

#[utoipa::path(post, path = "/api/v1/agents/{agent_id}/runtime/stop", tag = "runtime", responses((status = 200, body = AgentRuntime)))]
pub async fn stop(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(agent_id): Path<Uuid>,
) -> Result<Json<AgentRuntime>, ApiError> {
    auth.require(Permission::ExecutionsCancel)?;
    let _ = managed_agent(&state, &auth, agent_id).await?;
    let runtime = state
        .store
        .get_agent_runtime(auth.org_id, agent_id)
        .await?
        .ok_or_else(|| ApiError::not_found("agent runtime"))?;
    if let Some(execution_id) = runtime.last_execution_id {
        let _ = state
            .store
            .request_execution_cancel(auth.org_id, execution_id)
            .await;
    }
    let runtime = state
        .store
        .request_agent_runtime_stop(runtime.id)
        .await?
        .ok_or_else(|| ApiError::not_found("agent runtime"))?;
    audit::record(
        &state,
        &auth,
        "agent.runtime_stopping",
        Some("agent"),
        Some(agent_id),
        serde_json::json!({ "runtimeId": runtime.id }),
    )
    .await;
    Ok(Json(runtime))
}
