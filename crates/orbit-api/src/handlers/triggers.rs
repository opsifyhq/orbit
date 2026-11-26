use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use orbit_security::{AuthContext, Permission};
use orbit_store::Trigger;

use crate::audit;
use crate::dto::{CreateTriggerRequest, UpdateTriggerRequest};
use crate::error::ApiError;
use crate::paging::{Page, PageQuery};
use crate::state::AppState;

#[derive(Debug, Clone, Deserialize, utoipa::IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct TriggerListQuery {
    pub agent_id: Option<Uuid>,
    pub limit: Option<i64>,
    pub cursor: Option<String>,
}

impl TriggerListQuery {
    fn page(&self) -> PageQuery {
        PageQuery {
            limit: self.limit,
            cursor: self.cursor.clone(),
        }
    }
}

#[utoipa::path(get, path = "/api/v1/triggers", tag = "triggers", params(TriggerListQuery), responses((status = 200, body = Page<Trigger>)))]
pub async fn list_triggers(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<TriggerListQuery>,
) -> Result<Json<Page<Trigger>>, ApiError> {
    auth.require(Permission::AgentsRead)?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    let triggers = state
        .store
        .list_triggers(auth.org_id, query.agent_id, Some(&access.store_access()))
        .await?;
    Ok(Json(Page::from_vec(triggers, &query.page(), 100)?))
}

#[utoipa::path(post, path = "/api/v1/triggers", tag = "triggers", request_body = CreateTriggerRequest, responses((status = 201, body = Trigger)))]
pub async fn create_trigger(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    crate::extract::ApiJson(body): crate::extract::ApiJson<CreateTriggerRequest>,
) -> Result<(StatusCode, Json<Trigger>), ApiError> {
    auth.require(Permission::TriggersWrite)?;
    if let Some(harness) = &body.harness {
        if harness.parse::<orbit_domain::HarnessKind>().is_err() {
            return Err(ApiError::validation(vec![
                orbit_domain::ValidationIssue::new("harness", "unknown harness"),
            ]));
        }
    }
    let mut issues = Vec::new();
    match body.kind.as_str() {
        "schedule" => {
            let cron = body
                .config
                .get("cron")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let timezone = body.config.get("timezone").and_then(|v| v.as_str());
            if cron.parse::<croner::Cron>().is_err() {
                issues.push(orbit_domain::ValidationIssue::new(
                    "config.cron",
                    "invalid cron expression (standard 5-field cron)",
                ));
            }
            if let Err(error) = orbit_domain::parse_timezone(timezone) {
                issues.push(orbit_domain::ValidationIssue::new(
                    "config.timezone",
                    error.to_string(),
                ));
            }
        }
        "event" => {
            if body
                .config
                .get("subject")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .is_empty()
            {
                issues.push(orbit_domain::ValidationIssue::new(
                    "config.subject",
                    "event subject filter is required",
                ));
            }
        }
        "webhook" | "manual" => {}
        other => issues.push(orbit_domain::ValidationIssue::new(
            "kind",
            format!("unknown trigger kind '{other}'"),
        )),
    }
    if !issues.is_empty() {
        return Err(ApiError::validation(issues));
    }

    // A trigger makes its owner run the target agent, so creation needs use rights on the
    // agent and on every connection/provider its current spec references. Without this an
    // Operator could attach a trigger to a private agent and execute it via the scheduler.
    let agent = state.store.get_agent(auth.org_id, body.agent_id).await?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    access.require_use(
        "agent",
        agent.owner_user_id,
        agent.owner_team_id,
        &agent.visibility,
    )?;
    // An archived agent cannot run, so a trigger on it could only fail (#438).
    if agent.status == "archived" {
        return Err(ApiError::conflict(
            "the agent is archived; restore it before adding a trigger",
        ));
    }
    let version = state.store.get_current_version(auth.org_id, &agent).await?;
    let spec = crate::handlers::parse_spec(&version)?;
    crate::preflight::require_spec_references_use(&state, auth.org_id, &access, &spec).await?;

    let trigger = state
        .store
        .create_trigger(
            auth.org_id,
            body.agent_id,
            &body.kind,
            body.harness.as_deref(),
            body.config,
            body.enabled.unwrap_or(true),
        )
        .await?;
    state
        .store
        .assign_resource_owner(auth.org_id, "triggers", trigger.id, auth.user_id)
        .await?;
    audit::record(
        &state,
        &auth,
        "trigger.created",
        Some("trigger"),
        Some(trigger.id),
        serde_json::json!({ "agentId": body.agent_id, "kind": trigger.kind }),
    )
    .await;
    Ok((StatusCode::CREATED, Json(trigger)))
}

#[utoipa::path(patch, path = "/api/v1/triggers/{trigger_id}", tag = "triggers", request_body = UpdateTriggerRequest, responses((status = 200, body = Trigger)))]
pub async fn update_trigger(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(trigger_id): Path<Uuid>,
    crate::extract::ApiJson(body): crate::extract::ApiJson<UpdateTriggerRequest>,
) -> Result<Json<Trigger>, ApiError> {
    auth.require(Permission::TriggersWrite)?;
    let trigger = state.store.get_trigger(auth.org_id, trigger_id).await?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    access.require_manage(
        "trigger",
        trigger.owner_user_id,
        trigger.owner_team_id,
        &trigger.visibility,
    )?;
    let trigger = match body.enabled {
        Some(enabled) => {
            state
                .store
                .set_trigger_enabled(auth.org_id, trigger_id, enabled)
                .await?
        }
        None => state.store.get_trigger(auth.org_id, trigger_id).await?,
    };
    audit::record(
        &state,
        &auth,
        "trigger.updated",
        Some("trigger"),
        Some(trigger.id),
        serde_json::json!({ "enabled": trigger.enabled }),
    )
    .await;
    Ok(Json(trigger))
}

#[utoipa::path(delete, path = "/api/v1/triggers/{trigger_id}", tag = "triggers", responses((status = 204)))]
pub async fn delete_trigger(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(trigger_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::TriggersWrite)?;
    let trigger = state.store.get_trigger(auth.org_id, trigger_id).await?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    access.require_manage(
        "trigger",
        trigger.owner_user_id,
        trigger.owner_team_id,
        &trigger.visibility,
    )?;
    state.store.delete_trigger(auth.org_id, trigger_id).await?;
    audit::record(
        &state,
        &auth,
        "trigger.deleted",
        Some("trigger"),
        Some(trigger_id),
        serde_json::json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}
