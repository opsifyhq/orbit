//! Agent Team work queue API.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use orbit_domain::ValidationIssue;
use orbit_security::{AuthContext, Permission};
use orbit_store::team_queue::TeamQueueItem;

use crate::error::ApiError;
use crate::state::AppState;
use crate::team_runs::{effective_payload, start_queue_item};

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct QueueItemInput {
    pub external_key: String,
    #[serde(default)]
    pub repository: Option<String>,
    #[serde(default)]
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EnqueueRequest {
    /// 1 to 500 items. The batch is stored all or nothing.
    pub items: Vec<QueueItemInput>,
}

/// Most items one enqueue request may carry.
const MAX_ENQUEUE_ITEMS: usize = 500;

/// A queue item as enqueued: `created` is false when an item with that `externalKey`
/// was already queued, which is then returned unchanged.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EnqueuedItem {
    #[serde(flatten)]
    pub item: TeamQueueItem,
    pub created: bool,
}

#[utoipa::path(
    post, path = "/api/v1/agent-teams/{team_id}/queue", tag = "agent-teams",
    request_body = EnqueueRequest,
    responses(
        (status = 201, description = "at least one item was created", body = [EnqueuedItem]),
        (status = 200, description = "every item was already queued", body = [EnqueuedItem]),
    )
)]
pub async fn enqueue(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(team_id): Path<Uuid>,
    crate::extract::ApiJson(body): crate::extract::ApiJson<EnqueueRequest>,
) -> Result<(StatusCode, Json<Vec<EnqueuedItem>>), ApiError> {
    auth.require(Permission::ExecutionsCreate)?;
    let team = crate::handlers::teams::require_team_use(&state, &auth, team_id).await?;
    let definition = crate::team_runs::parse_definition(&team)?;
    if body.items.is_empty() || body.items.len() > MAX_ENQUEUE_ITEMS {
        return Err(ApiError::validation(vec![ValidationIssue::new(
            "items",
            format!("send 1 to {MAX_ENQUEUE_ITEMS} queue items"),
        )]));
    }
    // Validate the whole batch before storing anything (#432).
    for (index, item) in body.items.iter().enumerate() {
        if item.external_key.trim().is_empty() {
            return Err(ApiError::validation(vec![ValidationIssue::new(
                format!("items[{index}].externalKey"),
                "externalKey is required",
            )]));
        }
        // The run uses the team's connection credentials, so an item may only target the
        // team's configured repositories, the same rule as `create_run` (#384).
        crate::team_runs::validate_queue_repositories(
            &definition,
            &effective_payload(&item.payload, item.repository.as_deref()),
            false,
        )?;
    }
    let new_items: Vec<orbit_store::team_queue::NewQueueItem<'_>> = body
        .items
        .iter()
        .map(|item| orbit_store::team_queue::NewQueueItem {
            external_key: item.external_key.trim(),
            repository: item
                .repository
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty()),
            payload: &item.payload,
        })
        .collect();
    let stored = state
        .store
        .enqueue_team_items(auth.org_id, team_id, &new_items)
        .await?;
    let created: Vec<Uuid> = stored
        .iter()
        .filter(|(_, created)| *created)
        .map(|(item, _)| item.id)
        .collect();
    let status = if created.is_empty() {
        StatusCode::OK
    } else {
        crate::audit::record(
            &state,
            &auth,
            "team_queue.enqueued",
            Some("agent_team"),
            Some(team_id),
            serde_json::json!({ "items": created }),
        )
        .await;
        StatusCode::CREATED
    };
    Ok((
        status,
        Json(
            stored
                .into_iter()
                .map(|(item, created)| EnqueuedItem { item, created })
                .collect(),
        ),
    ))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct QueueListQuery {
    /// Items per page, 1 to 500 (default 500).
    pub limit: Option<i64>,
    /// Return the items created before this item: pass the first (oldest) id of the
    /// previous page to page back.
    pub before: Option<Uuid>,
}

#[utoipa::path(get, path = "/api/v1/agent-teams/{team_id}/queue", tag = "agent-teams", params(QueueListQuery), responses((status = 200, description = "the newest items, oldest first", body = [orbit_store::team_queue::TeamQueueItem])))]
pub async fn list(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(team_id): Path<Uuid>,
    axum::extract::Query(query): axum::extract::Query<QueueListQuery>,
) -> Result<Json<Vec<TeamQueueItem>>, ApiError> {
    auth.require(Permission::HarnessesRead)?;
    crate::handlers::teams::require_team_view(&state, &auth, team_id).await?;
    let limit = query.limit.unwrap_or(500);
    if !(1..=500).contains(&limit) {
        return Err(ApiError::validation(vec![ValidationIssue::new(
            "limit",
            "limit must be 1 to 500",
        )]));
    }
    Ok(Json(
        state
            .store
            .list_team_queue(auth.org_id, team_id, limit, query.before)
            .await?,
    ))
}

#[utoipa::path(post, path = "/api/v1/agent-teams/{team_id}/queue/{item_id}/run", tag = "agent-teams", responses((status = 201, body = crate::team_runs::TeamRunDetail)))]
pub async fn run_item(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path((team_id, item_id)): Path<(Uuid, Uuid)>,
) -> Result<(StatusCode, Json<crate::team_runs::TeamRunDetail>), ApiError> {
    auth.require(Permission::ExecutionsCreate)?;
    // The limit is checked atomically in start_step, which also serves dispatcher auto-run.
    let team = crate::handlers::teams::require_team_use(&state, &auth, team_id).await?;
    let item = state
        .store
        .get_team_queue_item(auth.org_id, item_id)
        .await?;
    if item.team_id != team_id {
        return Err(ApiError::not_found("queue item"));
    }
    if item.status == "running" {
        return Err(ApiError::conflict("queue item is already running"));
    }
    let detail = start_queue_item(&state, &auth, &team, &item).await?;
    audit_item(
        &state,
        &auth,
        "team_queue.run_started",
        &item,
        Some(detail.run.id),
    )
    .await;
    Ok((StatusCode::CREATED, Json(detail)))
}

/// A cancelled item keeps its run link until it is retried or requeued, so a second run is
/// never started while the first one's execution is still stopping (#299).
async fn reject_while_previous_run_is_live(
    state: &AppState,
    item: &TeamQueueItem,
) -> Result<(), ApiError> {
    if let Some(run_id) = item.team_run_id {
        if state.store.team_run_has_live_executions(run_id).await? {
            return Err(ApiError::conflict(
                "the previous run is still stopping; try again once its execution has ended",
            ));
        }
    }
    Ok(())
}

async fn fetch_item(
    state: &AppState,
    auth: &AuthContext,
    team_id: Uuid,
    item_id: Uuid,
) -> Result<TeamQueueItem, ApiError> {
    crate::handlers::teams::require_team_use(state, auth, team_id).await?;
    let item = state
        .store
        .get_team_queue_item(auth.org_id, item_id)
        .await?;
    if item.team_id != team_id {
        return Err(ApiError::not_found("queue item"));
    }
    Ok(item)
}

#[utoipa::path(post, path = "/api/v1/agent-teams/{team_id}/queue/{item_id}/retry", tag = "agent-teams", responses((status = 200, body = orbit_store::team_queue::TeamQueueItem)))]
pub async fn retry(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path((team_id, item_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<TeamQueueItem>, ApiError> {
    auth.require(Permission::ExecutionsCreate)?;
    let item = fetch_item(&state, &auth, team_id, item_id).await?;
    if item.status == "running" {
        return Err(ApiError::conflict("running items must be cancelled first"));
    }
    reject_while_previous_run_is_live(&state, &item).await?;
    let updated = state
        .store
        .retry_team_queue_item(auth.org_id, item_id, None)
        .await?
        .ok_or_else(|| {
            ApiError::conflict(
                "item cannot be retried (only failed or cancelled items with attempts left; use requeue to reset)",
            )
        })?;
    audit_item(&state, &auth, "team_queue.retried", &updated, None).await;
    Ok(Json(updated))
}

#[utoipa::path(post, path = "/api/v1/agent-teams/{team_id}/queue/{item_id}/requeue", tag = "agent-teams", responses((status = 200, body = orbit_store::team_queue::TeamQueueItem)))]
pub async fn requeue(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path((team_id, item_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<TeamQueueItem>, ApiError> {
    auth.require(Permission::ExecutionsCreate)?;
    let item = fetch_item(&state, &auth, team_id, item_id).await?;
    if item.status == "running" {
        return Err(ApiError::conflict("running items must be cancelled first"));
    }
    reject_while_previous_run_is_live(&state, &item).await?;
    let updated = state
        .store
        .requeue_team_queue_item(auth.org_id, item_id)
        .await?
        .ok_or_else(|| {
            ApiError::conflict("only pending, failed or cancelled items can be requeued")
        })?;
    audit_item(&state, &auth, "team_queue.requeued", &updated, None).await;
    Ok(Json(updated))
}

#[utoipa::path(post, path = "/api/v1/agent-teams/{team_id}/queue/{item_id}/cancel", tag = "agent-teams", responses((status = 200, body = orbit_store::team_queue::TeamQueueItem)))]
pub async fn cancel(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path((team_id, item_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<TeamQueueItem>, ApiError> {
    auth.require(Permission::ExecutionsCreate)?;
    let item = fetch_item(&state, &auth, team_id, item_id).await?;
    let run_id = item.team_run_id;
    let updated = state
        .store
        .cancel_team_queue_item(auth.org_id, item_id, Some("cancelled by operator"))
        .await?
        .ok_or_else(|| ApiError::conflict("item is already succeeded or cancelled"))?;
    if let Some(run_id) = run_id {
        // Stop the work, not only the bookkeeping: the step execution would otherwise run
        // to completion while the item shows as cancelled (#299).
        let cancelled = state
            .store
            .request_team_run_cancel(auth.org_id, run_id)
            .await?;
        crate::team_runs::announce_cancel(&state, auth.org_id, &cancelled).await;
        let _ = state
            .store
            .finish_team_run(run_id, "cancelled", None, Some("queue item cancelled"))
            .await;
    }
    audit_item(&state, &auth, "team_queue.cancelled", &updated, run_id).await;
    Ok(Json(updated))
}

/// Audit a queue action on the team, naming the item (#432).
async fn audit_item(
    state: &AppState,
    auth: &AuthContext,
    action: &str,
    item: &TeamQueueItem,
    run_id: Option<Uuid>,
) {
    crate::audit::record(
        state,
        auth,
        action,
        Some("agent_team"),
        Some(item.team_id),
        serde_json::json!({
            "itemId": item.id,
            "externalKey": item.external_key,
            "runId": run_id,
        }),
    )
    .await;
}
