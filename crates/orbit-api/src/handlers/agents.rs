use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Extension;
use axum::Json;
use uuid::Uuid;

use orbit_domain::{AgentDefinition, AgentSpec, AgentStatus};
use orbit_security::{AuthContext, Permission};

use crate::audit;
use crate::dto::{
    AgentDetail, AgentInstancePage, AgentListQuery, AgentPage, AgentSummary, AgentVersionView,
    UpdateAgentRequest,
};
use crate::error::ApiError;
use crate::handlers::{limit, load_agent_detail, load_visible_agent};
use crate::state::AppState;

async fn require_agent_manage(
    state: &AppState,
    auth: &AuthContext,
    agent_id: Uuid,
) -> Result<(), ApiError> {
    let agent = state.store.get_agent(auth.org_id, agent_id).await?;
    let access = crate::access::AccessScope::load(state, auth).await?;
    access.require_manage(
        "agent",
        agent.owner_user_id,
        agent.owner_team_id,
        &agent.visibility,
    )
}

#[utoipa::path(get, path = "/api/v1/agents", tag = "agents", params(AgentListQuery), responses((status = 200, body = AgentPage)))]
pub async fn list_agents(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<AgentListQuery>,
) -> Result<Json<AgentPage>, ApiError> {
    auth.require(Permission::AgentsRead)?;
    if let Some(status) = &query.status {
        if status != "current" && AgentStatus::parse(status).is_none() {
            return Err(ApiError::bad_request(format!(
                "unknown status '{status}'; expected draft, active, paused, archived, or current (everything not archived)"
            )));
        }
    }
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    let (rows, next_cursor, total) = state
        .store
        .list_agents(
            auth.org_id,
            limit(query.limit),
            query.cursor.as_deref(),
            query.status.as_deref(),
            query.q.as_deref(),
            Some(&access.store_access()),
        )
        .await?;

    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let spec = row
            .spec
            .as_ref()
            .and_then(|value| serde_json::from_value::<AgentSpec>(value.clone()).ok());
        items.push(AgentSummary::from_parts(&row.agent, spec.as_ref()));
    }
    Ok(Json(AgentPage {
        items,
        next_cursor,
        total,
    }))
}

#[utoipa::path(post, path = "/api/v1/agents", tag = "agents", request_body = AgentDefinition, responses((status = 201, body = AgentDetail)))]
pub async fn create_agent(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    crate::extract::ApiJson(definition): crate::extract::ApiJson<AgentDefinition>,
) -> Result<(StatusCode, Json<AgentDetail>), ApiError> {
    auth.require(Permission::AgentsWrite)?;
    state.enforce_limit("agents", state.store.count_agents(auth.org_id).await?)?;
    let report = state
        .store
        .validate_agent_references(auth.org_id, &definition)
        .await?;
    if !report.is_valid() {
        return Err(ApiError::validation(report.issues));
    }
    // The author must be able to use every connection/provider the spec binds (#140).
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    crate::preflight::require_spec_use(&state, auth.org_id, &access, &definition.spec).await?;

    let (agent, _version) = state
        .store
        .create_agent(auth.org_id, Some(auth.user_id), &definition)
        .await?;
    state
        .store
        .assign_resource_owner(auth.org_id, "agents", agent.id, auth.user_id)
        .await?;
    state.metrics.inc_agents();
    audit::record(
        &state,
        &auth,
        "agent.created",
        Some("agent"),
        Some(agent.id),
        serde_json::json!({ "name": agent.name, "harness": definition.spec.harness.as_ref().map(|harness| harness.kind.as_str()).unwrap_or("execution-selected") }),
    )
    .await;

    let detail = load_agent_detail(&state, &auth, agent.id).await?;
    Ok((StatusCode::CREATED, Json(detail)))
}

#[utoipa::path(get, path = "/api/v1/agents/{agent_id}", tag = "agents", responses((status = 200, body = AgentDetail)))]
pub async fn get_agent(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(agent_id): Path<Uuid>,
) -> Result<Json<AgentDetail>, ApiError> {
    auth.require(Permission::AgentsRead)?;
    Ok(Json(load_agent_detail(&state, &auth, agent_id).await?))
}

#[utoipa::path(patch, path = "/api/v1/agents/{agent_id}", tag = "agents", request_body = UpdateAgentRequest, responses((status = 200, body = AgentDetail)))]
pub async fn update_agent(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(agent_id): Path<Uuid>,
    crate::extract::ApiJson(body): crate::extract::ApiJson<UpdateAgentRequest>,
) -> Result<Json<AgentDetail>, ApiError> {
    auth.require(Permission::AgentsWrite)?;
    require_agent_manage(&state, &auth, agent_id).await?;
    if let Some(definition) = &body.definition {
        let report = state
            .store
            .validate_agent_references(auth.org_id, definition)
            .await?;
        if !report.is_valid() {
            return Err(ApiError::validation(report.issues));
        }
        let access = crate::access::AccessScope::load(&state, &auth).await?;
        crate::preflight::require_spec_use(&state, auth.org_id, &access, &definition.spec).await?;
        let agent = state.store.get_agent(auth.org_id, agent_id).await?;
        // Someone else saved a newer version since this edit was opened: refuse rather
        // than silently overwrite their work (the version write re-checks under a lock).
        if let Some(expected) = body.expected_version {
            if expected != agent.current_version {
                return Err(ApiError::conflict(
                    orbit_store::agents::stale_version_message(expected, agent.current_version),
                ));
            }
        }
        let current = state.store.get_current_version(auth.org_id, &agent).await?;
        let candidate_spec = serde_json::to_value(&definition.spec)
            .map_err(|error| ApiError::internal(error.to_string()))?;
        if current.spec == candidate_spec {
            // Metadata-only edit (name/description/labels) must not create a duplicate
            // spec version.
            state
                .store
                .update_agent_metadata(
                    auth.org_id,
                    agent_id,
                    Some(definition.metadata.name.trim()),
                    Some(definition.metadata.description.as_str()),
                    Some(&serde_json::to_value(&definition.metadata.labels).unwrap_or_default()),
                )
                .await?;
            audit::record(
                &state,
                &auth,
                "agent.updated",
                Some("agent"),
                Some(agent_id),
                serde_json::json!({ "fields": ["metadata"] }),
            )
            .await;
        } else {
            let (_, version) = state
                .store
                .add_agent_version_expecting(
                    auth.org_id,
                    agent_id,
                    Some(auth.user_id),
                    definition,
                    body.expected_version,
                )
                .await?;
            audit::record(
                &state,
                &auth,
                "agent.version_created",
                Some("agent"),
                Some(agent_id),
                serde_json::json!({ "version": version.version }),
            )
            .await;
        }
    } else {
        let name = body
            .name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        if body.name.is_some() && name.is_none() {
            return Err(ApiError::validation(vec![
                orbit_domain::ValidationIssue::new("name", "name must not be empty"),
            ]));
        }
        state
            .store
            .update_agent_metadata(
                auth.org_id,
                agent_id,
                name,
                body.description.as_deref(),
                body.labels.as_ref(),
            )
            .await?;
        audit::record(
            &state,
            &auth,
            "agent.updated",
            Some("agent"),
            Some(agent_id),
            serde_json::json!({ "fields": ["metadata"] }),
        )
        .await;
    }
    Ok(Json(load_agent_detail(&state, &auth, agent_id).await?))
}

#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
pub struct DeleteAgentQuery {
    /// Permanently delete the agent and its execution history (default: archive).
    pub hard: Option<bool>,
}

#[utoipa::path(delete, path = "/api/v1/agents/{agent_id}", tag = "agents", params(DeleteAgentQuery), responses((status = 200, body = AgentDetail), (status = 204)))]
pub async fn delete_agent(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(agent_id): Path<Uuid>,
    Query(query): Query<DeleteAgentQuery>,
) -> Result<Response, ApiError> {
    auth.require(Permission::AgentsWrite)?;
    require_agent_manage(&state, &auth, agent_id).await?;
    if query.hard.unwrap_or(false) {
        state.store.delete_agent(auth.org_id, agent_id).await?;
        audit::record(
            &state,
            &auth,
            "agent.deleted",
            Some("agent"),
            Some(agent_id),
            serde_json::json!({ "hard": true }),
        )
        .await;
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    let agent = state
        .store
        .set_agent_status(auth.org_id, agent_id, AgentStatus::Archived)
        .await?;
    audit::record(
        &state,
        &auth,
        "agent.archived",
        Some("agent"),
        Some(agent_id),
        serde_json::json!({ "name": agent.name }),
    )
    .await;
    Ok(Json(load_agent_detail(&state, &auth, agent_id).await?).into_response())
}

#[utoipa::path(post, path = "/api/v1/agents/{agent_id}/activate", tag = "agents", responses((status = 200, body = AgentDetail)))]
pub async fn activate_agent(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(agent_id): Path<Uuid>,
) -> Result<Json<AgentDetail>, ApiError> {
    auth.require(Permission::AgentsWrite)?;
    require_agent_manage(&state, &auth, agent_id).await?;
    state
        .store
        .set_agent_status(auth.org_id, agent_id, AgentStatus::Active)
        .await?;
    audit::record(
        &state,
        &auth,
        "agent.activated",
        Some("agent"),
        Some(agent_id),
        serde_json::json!({}),
    )
    .await;
    Ok(Json(load_agent_detail(&state, &auth, agent_id).await?))
}

#[utoipa::path(post, path = "/api/v1/agents/{agent_id}/pause", tag = "agents", responses((status = 200, body = AgentDetail)))]
pub async fn pause_agent(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(agent_id): Path<Uuid>,
) -> Result<Json<AgentDetail>, ApiError> {
    auth.require(Permission::AgentsWrite)?;
    require_agent_manage(&state, &auth, agent_id).await?;
    state
        .store
        .set_agent_status(auth.org_id, agent_id, AgentStatus::Paused)
        .await?;
    audit::record(
        &state,
        &auth,
        "agent.paused",
        Some("agent"),
        Some(agent_id),
        serde_json::json!({}),
    )
    .await;
    Ok(Json(load_agent_detail(&state, &auth, agent_id).await?))
}

#[utoipa::path(get, path = "/api/v1/agents/{agent_id}/versions", tag = "agents", responses((status = 200, body = [AgentVersionView])))]
pub async fn list_versions(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(agent_id): Path<Uuid>,
) -> Result<Json<Vec<AgentVersionView>>, ApiError> {
    auth.require(Permission::AgentsRead)?;
    load_visible_agent(&state, &auth, agent_id).await?;
    let versions = state
        .store
        .list_agent_versions(auth.org_id, agent_id)
        .await?;
    Ok(Json(versions.into_iter().map(Into::into).collect()))
}

#[utoipa::path(get, path = "/api/v1/agents/{agent_id}/export", tag = "agents", responses((status = 200, description = "Agent definition YAML")))]
pub async fn export_agent(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(agent_id): Path<Uuid>,
) -> Result<Response, ApiError> {
    auth.require(Permission::AgentsRead)?;
    let agent = state.store.get_agent(auth.org_id, agent_id).await?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    access.require_view(
        "agent",
        agent.owner_user_id,
        agent.owner_team_id,
        &agent.visibility,
    )?;
    // Same secret-free, name-referenced document as `/export/agents/{id}?format=yaml`.
    let yaml = crate::handlers::export::build_document(
        &state,
        auth.org_id,
        orbit_contracts::portable::PortableKind::Agent,
        agent_id,
    )
    .await?
    .to_yaml()
    .map_err(ApiError::internal)?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/yaml; charset=utf-8"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"agent.yaml\"",
            ),
        ],
        yaml,
    )
        .into_response())
}

#[utoipa::path(get, path = "/api/v1/agent-instances", tag = "agents", responses((status = 200, body = AgentInstancePage)))]
pub async fn list_agent_instances(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> Result<Json<AgentInstancePage>, ApiError> {
    auth.require(Permission::AgentsRead)?;
    let instances = state.store.list_agent_instances(auth.org_id).await?;
    let total = instances.len() as i64;
    Ok(Json(AgentInstancePage {
        items: instances
            .into_iter()
            .map(|instance| crate::dto::AgentInstanceView { instance })
            .collect(),
        next_cursor: None,
        total,
    }))
}
