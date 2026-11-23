use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::Extension;
use axum::Json;
use futures::Stream;
use uuid::Uuid;

use orbit_contracts::{subjects, OrbitEvent};
use orbit_domain::{ExecutionStatus, TriggerSource};
use orbit_security::{AuthContext, Permission};
use orbit_store::executions::EventFilter;
use orbit_store::ExecutionRecord;

use crate::audit;
use crate::dto::{
    CreateExecutionRequest, EventListQuery, ExecutionEventPage, ExecutionListQuery, ExecutionPage,
};
use crate::error::ApiError;
use crate::handlers::{limit, parse_spec};
use crate::state::AppState;

async fn require_execution_view(
    state: &AppState,
    auth: &AuthContext,
    execution_id: Uuid,
) -> Result<ExecutionRecord, ApiError> {
    let execution = state.store.get_execution(auth.org_id, execution_id).await?;
    let agent = state
        .store
        .get_agent(auth.org_id, execution.agent_id)
        .await?;
    let access = crate::access::AccessScope::load(state, auth).await?;
    access.require_view(
        "execution",
        agent.owner_user_id,
        agent.owner_team_id,
        &agent.visibility,
    )?;
    Ok(execution)
}

/// Controlling an execution (cancel) needs use rights on its agent, not just view.
async fn require_execution_use(
    state: &AppState,
    auth: &AuthContext,
    execution_id: Uuid,
) -> Result<ExecutionRecord, ApiError> {
    let execution = state.store.get_execution(auth.org_id, execution_id).await?;
    let agent = state
        .store
        .get_agent(auth.org_id, execution.agent_id)
        .await?;
    let access = crate::access::AccessScope::load(state, auth).await?;
    access.require_use(
        "execution",
        agent.owner_user_id,
        agent.owner_team_id,
        &agent.visibility,
    )?;
    Ok(execution)
}

const EVENT_KINDS: &[&str] = &["status", "log", "tool_call", "output", "error", "metric"];

fn bounded_event_term(value: Option<&str>, field: &str) -> Result<Option<String>, ApiError> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if value.chars().count() > 200 {
        return Err(ApiError::bad_request(format!(
            "{field} must be <= 200 characters"
        )));
    }
    Ok(Some(value.to_string()))
}

/// Query parameters are validated here so bad filters fail loudly instead of
/// quietly returning an empty timeline. The store applies them before `limit`.
fn event_filter(query: &EventListQuery) -> Result<EventFilter, ApiError> {
    let mut kinds = Vec::new();
    if let Some(raw) = &query.kind {
        for kind in raw
            .split(',')
            .map(str::trim)
            .filter(|kind| !kind.is_empty())
        {
            if !EVENT_KINDS.contains(&kind) {
                return Err(ApiError::bad_request(format!(
                    "unknown event kind '{kind}'"
                )));
            }
            let kind = kind.to_string();
            if !kinds.contains(&kind) {
                kinds.push(kind);
            }
        }
    }
    Ok(EventFilter {
        kinds,
        tool: bounded_event_term(query.tool.as_deref(), "tool")?,
        q: bounded_event_term(query.q.as_deref(), "q")?,
    })
}

#[utoipa::path(get, path = "/api/v1/executions", tag = "executions", params(ExecutionListQuery), responses((status = 200, body = ExecutionPage)))]
pub async fn list_executions(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<ExecutionListQuery>,
) -> Result<Json<ExecutionPage>, ApiError> {
    auth.require(Permission::ExecutionsRead)?;
    // One status, or several separated by commas ("failed,timed_out"): the groups people
    // think in (in flight, failed) span more than one status.
    let status = match query.status.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(raw) => {
            let mut statuses = Vec::new();
            for part in raw
                .split(',')
                .map(str::trim)
                .filter(|part| !part.is_empty())
            {
                let parsed = ExecutionStatus::parse(part)
                    .ok_or_else(|| ApiError::bad_request(format!("unknown status '{part}'")))?;
                statuses.push(parsed.as_str());
            }
            Some(statuses.join(","))
        }
    };
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    let store_access = access.store_access();
    let (items, next_cursor, total) = state
        .store
        .list_executions(
            auth.org_id,
            limit(query.limit),
            query.cursor.as_deref(),
            status.as_deref(),
            query.agent_id,
            query.archived,
            Some(&store_access),
        )
        .await?;
    Ok(Json(ExecutionPage {
        items,
        next_cursor,
        total,
    }))
}

#[utoipa::path(get, path = "/api/v1/executions/{execution_id}", tag = "executions", responses((status = 200, body = ExecutionRecord)))]
pub async fn get_execution(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(execution_id): Path<Uuid>,
) -> Result<Json<ExecutionRecord>, ApiError> {
    auth.require(Permission::ExecutionsRead)?;
    Ok(Json(
        require_execution_view(&state, &auth, execution_id).await?,
    ))
}

#[utoipa::path(get, path = "/api/v1/executions/{execution_id}/events", tag = "executions", params(EventListQuery), responses((status = 200, body = ExecutionEventPage)))]
pub async fn list_execution_events(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(execution_id): Path<Uuid>,
    Query(query): Query<EventListQuery>,
) -> Result<Json<ExecutionEventPage>, ApiError> {
    auth.require(Permission::ExecutionsRead)?;
    require_execution_view(&state, &auth, execution_id).await?;
    let items = state
        .store
        .list_execution_events_filtered(
            auth.org_id,
            execution_id,
            query.after_seq.unwrap_or(0),
            query.limit.unwrap_or(200).clamp(1, 1000) as i64,
            &event_filter(&query)?,
        )
        .await?;
    Ok(Json(ExecutionEventPage { items }))
}

#[utoipa::path(get, path = "/api/v1/agents/{agent_id}/executions", tag = "executions", params(ExecutionListQuery), responses((status = 200, body = ExecutionPage)))]
pub async fn list_agent_executions(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(agent_id): Path<Uuid>,
    Query(query): Query<ExecutionListQuery>,
) -> Result<Json<ExecutionPage>, ApiError> {
    auth.require(Permission::ExecutionsRead)?;
    super::load_visible_agent(&state, &auth, agent_id).await?;
    let (items, next_cursor, total) = state
        .store
        .list_executions(
            auth.org_id,
            limit(query.limit),
            query.cursor.as_deref(),
            query.status.as_deref(),
            Some(agent_id),
            query.archived,
            None,
        )
        .await?;
    Ok(Json(ExecutionPage {
        items,
        next_cursor,
        total,
    }))
}

/// Manually start an execution. Returns 202 with the queued execution.
#[utoipa::path(post, path = "/api/v1/agents/{agent_id}/executions", tag = "executions", request_body = CreateExecutionRequest, responses((status = 202, body = ExecutionRecord), (status = 429, description = "Concurrency or rate limit reached")))]
pub async fn execute_agent(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(agent_id): Path<Uuid>,
    headers: HeaderMap,
    crate::extract::ApiJson(body): crate::extract::ApiJson<CreateExecutionRequest>,
) -> Result<(StatusCode, Json<ExecutionRecord>), ApiError> {
    auth.require(Permission::ExecutionsCreate)?;
    // Held until create_execution below: the check and insert cannot race with replicas.
    let _execution_limit = state.execution_limit_guard().await?;

    let agent = state.store.get_agent(auth.org_id, agent_id).await?;
    if agent.status == "archived" {
        return Err(ApiError::bad_request("archived agents cannot be executed"));
    }
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    access.require_use(
        "agent",
        agent.owner_user_id,
        agent.owner_team_id,
        &agent.visibility,
    )?;
    let version = state.store.get_current_version(auth.org_id, &agent).await?;
    let spec = parse_spec(&version)?;
    let task = if body.task.trim().is_empty() {
        spec.run_defaults
            .task
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("")
            .to_string()
    } else {
        body.task.trim().to_string()
    };
    if task.is_empty() {
        return Err(ApiError::validation(vec![
            orbit_domain::ValidationIssue::new("task", "task must not be empty"),
        ]));
    }
    if task.chars().count() > 200_000 {
        return Err(ApiError::validation(vec![
            orbit_domain::ValidationIssue::new("task", "task must be <= 200000 characters"),
        ]));
    }
    let mut skills = body.skills.clone();
    skills.sort();
    skills.dedup();
    crate::preflight::require_skills_use(&state, auth.org_id, &access, &skills, "skills", false)
        .await?;

    // Harness resolution: explicit request -> legacy agent version -> org default.
    let requested_harness = body.harness.as_deref().or(body.harness_override.as_deref());
    let harness_kind = match requested_harness {
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
                    "agent has no legacy harness and no default harness is configured",
                )])
            })?,
        },
    };
    // Shared preflight (#140): agent + connection/provider use and harness availability.
    // The per-hour quota is checked below, after the idempotent replay.
    let preflight = crate::preflight::Preflight {
        state: &state,
        org_id: auth.org_id,
        access: &access,
        agent: &agent,
        spec: &spec,
        harness: harness_kind,
        extra_connection_ids: &[],
    };
    preflight.authorize_access().await?;
    preflight.check_harness().await?;
    // Placement: the run's own choice, else the agent's, else any matching worker.
    let placement = match body.placement.as_deref().map(str::trim) {
        None | Some("") => spec
            .harness
            .as_ref()
            .map(|harness| harness.placement)
            .unwrap_or_default(),
        Some(value) => orbit_domain::PlacementTarget::parse(value).ok_or_else(|| {
            ApiError::validation(vec![orbit_domain::ValidationIssue::new(
                "placement",
                "unknown placement; expected auto, local, docker or kubernetes",
            )])
        })?,
    };
    let placement = placement.as_str();

    let idempotency_key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .or(body.idempotency_key.clone())
        .filter(|key| !key.trim().is_empty() && key.len() <= 200);

    // Idempotent replay returns the existing execution before any quota/concurrency
    // checks so retried client requests never observe a spurious 429.
    if let Some(key) = &idempotency_key {
        if let Some(existing) = state
            .store
            .get_execution_by_idempotency_key(auth.org_id, key)
            .await?
        {
            return Ok((StatusCode::ACCEPTED, Json(existing)));
        }
    }

    let active = state
        .store
        .count_active_executions_for_agent(auth.org_id, agent_id)
        .await?;
    if active >= spec.resources.max_concurrent_executions as i64 {
        return Err(ApiError::rate_limited(format!(
            "agent already has {active} active execution(s); maxConcurrentExecutions is {}",
            spec.resources.max_concurrent_executions
        )));
    }
    preflight.check_quota().await?;

    let input = serde_json::json!({
        "task": task,
        "name": body.name,
        "payload": body.payload,
        "provenance": {
            "source": format!("user:{}", auth.email),
            "trusted": true,
            "subject": null,
            "receivedAt": chrono::Utc::now(),
        },
        "agentVersion": version.version,
    });

    let execution = state
        .store
        .create_execution(orbit_store::executions::NewExecution {
            org_id: auth.org_id,
            agent_id: agent.id,
            agent_version_id: version.id,
            agent_name: &agent.name,
            harness_kind,
            trigger_source: TriggerSource::Manual,
            placement,
            trigger_id: None,
            input,
            session_ref: None,
            skills: &skills,
            timeout_seconds: spec.resources.timeout_seconds,
            requested_by: Some(auth.user_id),
            idempotency_key: idempotency_key.as_deref(),
            team_run_id: None,
            team_step_index: None,
        })
        .await?;

    // Publish only for newly created executions; a replayed idempotent request returns
    // the existing row and must not enqueue a second run.
    if execution.status == "queued" {
        let event = OrbitEvent::new(
            subjects::EXECUTION_REQUESTED,
            "orbit-api",
            serde_json::json!({
                "executionId": execution.id,
                "agentId": agent.id,
                "harness": harness_kind.as_str(),
            }),
        )
        .with_org(auth.org_id)
        .with_idempotency(format!("exec:{}", execution.id));
        if let Err(err) = state.bus.publish(event).await {
            tracing::warn!(error = %err, execution_id = %execution.id, "event publish failed; worker polling will pick up the execution");
        }
    }

    // Make the queued state observable even before a worker claims the execution.
    if let Err(err) = state
        .store
        .append_execution_event(
            auth.org_id,
            execution.id,
            "status",
            orbit_domain::LogLevel::Info,
            &format!(
                "execution queued; waiting for a {} worker (placement: {})",
                harness_kind, placement
            ),
            &serde_json::json!({
                "harness": harness_kind.as_str(),
                "placement": placement,
            }),
        )
        .await
    {
        tracing::warn!(execution_id = %execution.id, error = %err, "failed to append execution event");
    }

    audit::record(
        &state,
        &auth,
        "execution.requested",
        Some("execution"),
        Some(execution.id),
        serde_json::json!({
            "agentId": agent.id,
            "agentVersion": version.version,
            "idempotencyKey": idempotency_key,
        }),
    )
    .await;

    Ok((StatusCode::ACCEPTED, Json(execution)))
}

#[utoipa::path(post, path = "/api/v1/executions/{execution_id}/cancel", tag = "executions", responses((status = 200, body = ExecutionRecord)))]
pub async fn cancel_execution(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(execution_id): Path<Uuid>,
) -> Result<Json<ExecutionRecord>, ApiError> {
    auth.require(Permission::ExecutionsCancel)?;
    require_execution_use(&state, &auth, execution_id).await?;
    let execution = state
        .store
        .request_execution_cancel(auth.org_id, execution_id)
        .await?;

    let event = OrbitEvent::new(
        subjects::EXECUTION_CANCEL,
        "orbit-api",
        serde_json::json!({ "executionId": execution.id }),
    )
    .with_org(auth.org_id);
    if let Err(err) = state.bus.publish(event).await {
        tracing::warn!(error = %err, "cancel event publish failed; worker polls the cancel flag");
    }
    audit::record(
        &state,
        &auth,
        "execution.cancel_requested",
        Some("execution"),
        Some(execution.id),
        serde_json::json!({}),
    )
    .await;
    Ok(Json(execution))
}

/// Server-Sent Events stream: snapshot, replay of persisted events, then tail.
#[utoipa::path(get, path = "/api/v1/executions/{execution_id}/events/stream", tag = "executions", responses((status = 200, description = "SSE stream of execution events")))]
pub async fn stream_execution_events(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(execution_id): Path<Uuid>,
    headers: HeaderMap,
    Query(query): Query<EventListQuery>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    auth.require(Permission::ExecutionsRead)?;
    require_execution_view(&state, &auth, execution_id).await?;

    let last_event_id = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<i64>().ok());
    let after_seq = query.after_seq.or(last_event_id).unwrap_or(0);

    let org_id = auth.org_id;
    let store = state.store.clone();
    let shutdown = state.shutdown.clone();
    let stream = async_stream::stream! {
        let mut last = after_seq;
        let mut snapshot_sent = false;
        loop {
            if shutdown.is_cancelled() {
                yield Ok(Event::default().event("shutdown").data("{}"));
                break;
            }
            let execution = match store.get_execution(org_id, execution_id).await {
                Ok(execution) => execution,
                Err(err) => {
                    yield Ok(Event::default().event("error").data(
                        serde_json::json!({ "message": err.to_string() }).to_string(),
                    ));
                    break;
                }
            };
            if !snapshot_sent {
                yield Ok(Event::default()
                    .event("snapshot")
                    .data(serde_json::to_string(&execution).unwrap_or_default()));
                snapshot_sent = true;
            }

            match store
                .list_execution_events(org_id, execution_id, last, 100)
                .await
            {
                Ok(events) if !events.is_empty() => {
                    for event in events {
                        last = event.seq;
                        yield Ok(Event::default()
                            .event("execution.event")
                            .id(event.seq.to_string())
                            .data(serde_json::to_string(&event).unwrap_or_default()));
                    }
                    continue;
                }
                Ok(_) => {}
                Err(err) => {
                    yield Ok(Event::default().event("error").data(
                        serde_json::json!({ "message": err.to_string() }).to_string(),
                    ));
                    break;
                }
            }

            let terminal = ExecutionStatus::parse(&execution.status)
                .map(|status| status.is_terminal())
                .unwrap_or(false);
            if terminal {
                yield Ok(Event::default()
                    .event("done")
                    .data(serde_json::to_string(&execution).unwrap_or_default()));
                break;
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(700)) => {}
                _ = shutdown.cancelled() => {}
            }
        }
    };

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    ))
}

#[utoipa::path(post, path = "/api/v1/executions/{execution_id}/rerun", tag = "executions", responses((status = 202, body = ExecutionRecord)))]
pub async fn rerun_execution(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(execution_id): Path<Uuid>,
) -> Result<(StatusCode, Json<ExecutionRecord>), ApiError> {
    auth.require(Permission::ExecutionsCreate)?;
    let original = require_execution_view(&state, &auth, execution_id).await?;
    if !matches!(
        ExecutionStatus::parse(&original.status),
        Some(status) if status.is_terminal()
    ) {
        return Err(ApiError::conflict("only terminal executions can be rerun"));
    }
    let agent = state
        .store
        .get_agent(auth.org_id, original.agent_id)
        .await?;
    // A team step carries the team's connections, env mappings and subagents in its input.
    // Replaying that without re-running the team authorization would hand them to whoever
    // can merely view the execution, so team steps can only be re-run through the team.
    if original.team_run_id.is_some() || original.input.get("team").is_some() {
        return Err(ApiError::conflict(
            "this execution is a team step and cannot be rerun on its own; re-run the team instead",
        ));
    }
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    crate::preflight::require_skills_use(
        &state,
        auth.org_id,
        &access,
        &original.skills,
        "skills",
        false,
    )
    .await?;
    let version = state
        .store
        .get_agent_version_by_id(auth.org_id, original.agent_version_id)
        .await?;
    let spec: orbit_domain::AgentSpec = serde_json::from_value(version.spec)
        .map_err(|error| ApiError::internal(format!("stored agent spec is invalid: {error}")))?;
    let harness_kind = original
        .harness_kind
        .parse::<orbit_domain::HarnessKind>()
        .map_err(|_| ApiError::internal("stored execution has an unknown harness"))?;
    // Same preflight as a fresh execution (#140): the rerun replays the original version's
    // spec, so its connections/provider must still be usable by the caller.
    crate::preflight::Preflight {
        state: &state,
        org_id: auth.org_id,
        access: &access,
        agent: &agent,
        spec: &spec,
        harness: harness_kind,
        extra_connection_ids: &[],
    }
    .run()
    .await?;
    let active = state
        .store
        .count_active_executions_for_agent(auth.org_id, original.agent_id)
        .await?;
    if active >= spec.resources.max_concurrent_executions as i64 {
        return Err(ApiError::conflict(
            "agent concurrency limit reached; retry after a running execution finishes",
        ));
    }
    // Held until the rerun execution is inserted below.
    let _execution_limit = state.execution_limit_guard().await?;
    let mut input = original.input.clone();
    // #194: an approval override only survives a rerun by someone who may decide approvals.
    if !auth.can(Permission::ApprovalsDecide) {
        orbit_domain::strip_approval_overrides(&mut input);
    } else if let Some(object) = input.as_object_mut() {
        object.remove(orbit_domain::LEGACY_AUTONOMY_OVERRIDE_KEY);
    }
    if let Some(object) = input.as_object_mut() {
        object.insert("rerunOf".into(), serde_json::json!(original.id));
    }
    let idempotency_key = format!("rerun:{}:{}", original.id, Uuid::now_v7());
    let execution = state
        .store
        .create_execution(orbit_store::executions::NewExecution {
            org_id: auth.org_id,
            agent_id: original.agent_id,
            agent_version_id: original.agent_version_id,
            agent_name: &original.agent_name,
            harness_kind,
            trigger_source: TriggerSource::Manual,
            placement: &original.placement,
            trigger_id: None,
            input,
            session_ref: None,
            skills: &original.skills,
            timeout_seconds: original.timeout_seconds.max(1) as u64,
            requested_by: Some(auth.user_id),
            idempotency_key: Some(&idempotency_key),
            team_run_id: None,
            team_step_index: None,
        })
        .await?;
    let event = OrbitEvent::new(
        subjects::EXECUTION_REQUESTED,
        "orbit-api",
        serde_json::json!({
            "executionId": execution.id,
            "agentId": execution.agent_id,
            "harness": execution.harness_kind,
            "rerunOf": original.id,
        }),
    )
    .with_org(auth.org_id)
    .with_idempotency(format!("exec:{}", execution.id));
    if let Err(err) = state.bus.publish(event).await {
        tracing::warn!(error = %err, "rerun event publish failed; worker polling will pick up the execution");
    }
    if let Err(err) = state
        .store
        .append_execution_event(
            auth.org_id,
            execution.id,
            "status",
            orbit_domain::LogLevel::Info,
            &format!(
                "execution queued as rerun of {}; waiting for a {} worker",
                original.id, execution.harness_kind
            ),
            &serde_json::json!({ "rerunOf": original.id, "harness": execution.harness_kind }),
        )
        .await
    {
        tracing::warn!(execution_id = %execution.id, error = %err, "failed to append execution event");
    }
    audit::record(
        &state,
        &auth,
        "execution.rerun",
        Some("execution"),
        Some(execution.id),
        serde_json::json!({ "rerunOf": original.id }),
    )
    .await;
    Ok((StatusCode::ACCEPTED, Json(execution)))
}
