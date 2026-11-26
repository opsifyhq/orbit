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
use orbit_store::AgentTeam;

use crate::error::ApiError;
use crate::state::AppState;

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
    let definition = crate::handlers::teams::parse_definition(&team)?;
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
        crate::handlers::teams::validate_queue_repositories(
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

#[utoipa::path(post, path = "/api/v1/agent-teams/{team_id}/queue/{item_id}/run", tag = "agent-teams", responses((status = 201, body = crate::handlers::teams::TeamRunDetail)))]
pub async fn run_item(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path((team_id, item_id)): Path<(Uuid, Uuid)>,
) -> Result<(StatusCode, Json<crate::handlers::teams::TeamRunDetail>), ApiError> {
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

/// Claim a queue item, then create and start its TeamRun.
///
/// The claim (`pending|failed -> running`) is a single conditional UPDATE, so concurrent
/// callers (double click, manual run vs auto-run, two replicas) cannot both start a run.
/// Anything that fails after the claim cancels the run it created (and its queued
/// execution) and puts the item back into `failed`, which is retryable.
pub(crate) async fn start_queue_item(
    state: &AppState,
    auth: &AuthContext,
    team: &AgentTeam,
    item: &TeamQueueItem,
) -> Result<crate::handlers::teams::TeamRunDetail, ApiError> {
    let definition = crate::handlers::teams::parse_definition(team)?;
    let Some(item) = state
        .store
        .claim_team_queue_item(auth.org_id, item.id)
        .await?
    else {
        return Err(ApiError::conflict(
            "queue item is not startable (already running, succeeded or cancelled)",
        ));
    };
    let task = item
        .payload
        .get("task")
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| definition.instructions.clone());
    let mut payload = item.payload.clone();
    if !payload.is_object() {
        payload = serde_json::json!({});
    }
    if let Some(repository) = &item.repository {
        payload["repository"] = serde_json::json!(repository);
    }
    if payload
        .get("repository")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .is_none()
    {
        if let Some(repository) = &definition.repository {
            payload["repository"] = serde_json::json!(repository);
        }
    }
    if !definition.repositories.is_empty() && payload.get("repositories").is_none() {
        payload["repositories"] = serde_json::json!(definition.repositories);
    }
    // Items stored before enqueue validated their repository (#384), or written by hand,
    // are checked again before the team's credentials are used for them.
    if let Err(error) =
        crate::handlers::teams::validate_queue_repositories(&definition, &payload, true)
    {
        release_failed_claim(state, auth, &item, None, &error.message).await;
        return Err(error);
    }
    let input = serde_json::json!({
        "task": task,
        "instructions": definition.instructions,
        "payload": payload,
        "definition": definition,
        "autoApprove": false,
        "queueItemId": item.id,
        "externalKey": item.external_key,
    });
    // Shared execution preflight (#140) before any run row is created. A refusal ends the
    // claim with the reason instead of leaving the item `running` without a run (#383).
    let authorized = async {
        let access = crate::access::AccessScope::load(state, auth).await?;
        let step_agent_ids: Vec<Uuid> = definition.steps.iter().map(|step| step.agent_id).collect();
        crate::preflight::authorize_team_run(
            state,
            auth.org_id,
            &access,
            team,
            &step_agent_ids,
            &crate::handlers::teams::team_connection_ids(&definition),
        )
        .await
    }
    .await;
    if let Err(error) = authorized {
        release_failed_claim(state, auth, &item, None, &error.message).await;
        return Err(error);
    }
    let run = match state
        .store
        .create_team_run(auth.org_id, team.id, &input, Some(auth.user_id))
        .await
    {
        Ok(run) => run,
        Err(error) => {
            let error = ApiError::from(error);
            release_failed_claim(state, auth, &item, None, &error.message).await;
            return Err(error);
        }
    };
    match start_claimed_run(state, auth, &item, &run, &definition).await {
        Ok(steps) => Ok(crate::handlers::teams::TeamRunDetail { run, steps }),
        Err(error) => {
            release_failed_claim(state, auth, &item, Some(&run), &error.message).await;
            Err(error)
        }
    }
}

/// The payload a queue item runs with: its `repository` overrides `payload.repository`.
fn effective_payload(payload: &serde_json::Value, repository: Option<&str>) -> serde_json::Value {
    let mut effective = if payload.is_object() {
        payload.clone()
    } else {
        serde_json::json!({})
    };
    if let Some(repository) = repository.map(str::trim).filter(|value| !value.is_empty()) {
        effective["repository"] = serde_json::json!(repository);
    }
    effective
}

async fn start_claimed_run(
    state: &AppState,
    auth: &AuthContext,
    item: &TeamQueueItem,
    run: &orbit_store::TeamRun,
    definition: &crate::handlers::teams::TeamDefinition,
) -> Result<Vec<orbit_store::TeamStep>, ApiError> {
    if !state
        .store
        .attach_team_run_to_queue_item(auth.org_id, item.id, run.id)
        .await?
    {
        return Err(ApiError::conflict(
            "queue item was cancelled or claimed elsewhere while starting",
        ));
    }
    let first = crate::handlers::teams::start_first_steps(state, run, definition).await?;
    for step in &first.started {
        crate::handlers::teams::auto_launch_step(state, run, definition, step, Some(auth)).await?;
    }
    Ok(first.steps)
}

/// Undo a failed start: cancel the run and its not-yet-running execution so nothing
/// executes for an item that is reported as failed, then mark the item retryable.
async fn release_failed_claim(
    state: &AppState,
    auth: &AuthContext,
    item: &TeamQueueItem,
    run: Option<&orbit_store::TeamRun>,
    message: &str,
) {
    if let Some(run) = run {
        // A workflow run may have started several steps; stop them all.
        match state
            .store
            .request_team_run_cancel(run.org_id, run.id)
            .await
        {
            Ok(cancelled) => {
                crate::handlers::teams::announce_cancel(state, run.org_id, &cancelled).await
            }
            Err(error) => {
                tracing::warn!(%error, run_id = %run.id, "failed to cancel executions of aborted team run")
            }
        }
        let key = format!("team:{}:0", run.id);
        if let Ok(Some(execution)) = state
            .store
            .get_execution_by_idempotency_key(run.org_id, &key)
            .await
        {
            if let Err(error) = state
                .store
                .request_execution_cancel(run.org_id, execution.id)
                .await
            {
                tracing::warn!(%error, execution_id = %execution.id, "failed to cancel execution of aborted team run");
            }
        }
        if let Err(error) = state
            .store
            .finish_team_run(run.id, "cancelled", None, Some(message))
            .await
        {
            tracing::warn!(%error, run_id = %run.id, "failed to cancel aborted team run");
        }
    }
    if let Err(error) = state
        .store
        .fail_claimed_team_queue_item(auth.org_id, item.id, message)
        .await
    {
        tracing::warn!(%error, queue_item = %item.id, "failed to release queue item claim");
    }
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
    // A running item has claimed its source issue. Stop its run first, then close the
    // item through the outcome writer so the claim label is removed (#393).
    if let (true, Some(run_id)) = (item.status == "running", run_id) {
        let mut cancelled = state
            .store
            .request_team_run_cancel(auth.org_id, run_id)
            .await?;
        if let Some(run) = state
            .store
            .finish_team_run(run_id, "cancelled", None, Some("queue item cancelled"))
            .await?
        {
            // Second sweep: a step started meanwhile may have missed the first.
            cancelled.extend(
                state
                    .store
                    .request_team_run_cancel(auth.org_id, run_id)
                    .await?,
            );
            crate::handlers::teams::announce_cancel(&state, auth.org_id, &cancelled).await;
            crate::team_outcomes::close_cancelled_queue_loop(&state, &run, "cancelled by operator")
                .await;
            let updated = state
                .store
                .get_team_queue_item(auth.org_id, item_id)
                .await?;
            audit_item(
                &state,
                &auth,
                "team_queue.cancelled",
                &updated,
                Some(run_id),
            )
            .await;
            return Ok(Json(updated));
        }
        // The run had already ended; its outcome is pending. Cancel the item directly.
        crate::handlers::teams::announce_cancel(&state, auth.org_id, &cancelled).await;
    }
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
        crate::handlers::teams::announce_cancel(&state, auth.org_id, &cancelled).await;
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
