//! Execution result outputs (Slack/Mattermost/webhook) and delivery.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use orbit_domain::ValidationIssue;
use orbit_security::{new_secret_ref, AuthContext, Permission, SecretValue};
use orbit_store::outputs::DeliveryOutcome;
use orbit_store::{ExecutionRecord, Output, OutputDeliveryJob, StoreError, TeamRun};

use crate::audit;
use crate::error::ApiError;
use crate::state::AppState;

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OutputRequest {
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub agent_id: Option<Uuid>,
    #[serde(default)]
    pub team_id: Option<Uuid>,
    #[serde(default)]
    pub events: Vec<String>,
    /// Incoming webhook / target URL. Write-only; never returned.
    #[serde(default)]
    pub webhook_url: Option<String>,
    #[serde(default)]
    pub template: Option<String>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OutputView {
    pub id: Uuid,
    pub name: String,
    pub kind: String,
    pub agent_id: Option<Uuid>,
    pub team_id: Option<Uuid>,
    pub events: Vec<String>,
    pub template: Option<String>,
    pub enabled: bool,
    pub has_webhook: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<Output> for OutputView {
    fn from(output: Output) -> Self {
        Self {
            id: output.id,
            name: output.name,
            kind: output.kind,
            agent_id: output.agent_id,
            team_id: output.team_id,
            events: output.events,
            template: output.template,
            enabled: output.enabled,
            has_webhook: !output.secret_ref.is_empty(),
            created_at: output.created_at,
            updated_at: output.updated_at,
        }
    }
}

fn validated(body: &OutputRequest) -> Result<(String, Vec<String>), ApiError> {
    let mut issues = Vec::new();
    let name = body.name.trim().to_string();
    if body.agent_id.is_some() && body.team_id.is_some() {
        issues.push(ValidationIssue::new(
            "target",
            "choose an agent or a team, not both",
        ));
    }
    if name.is_empty() {
        issues.push(ValidationIssue::new("name", "name is required"));
    }
    if !matches!(body.kind.as_str(), "slack" | "mattermost" | "webhook") {
        issues.push(ValidationIssue::new(
            "kind",
            "kind must be slack, mattermost or webhook",
        ));
    }
    let mut events = if body.events.is_empty() {
        vec!["succeeded".into(), "failed".into(), "cancelled".into()]
    } else {
        body.events.clone()
    };
    events.sort();
    events.dedup();
    for event in &events {
        if !matches!(event.as_str(), "succeeded" | "failed" | "cancelled") {
            issues.push(ValidationIssue::new(
                "events",
                format!("unknown event '{event}'"),
            ));
        }
    }
    if !issues.is_empty() {
        return Err(ApiError::validation(issues));
    }
    Ok((name, events))
}

#[utoipa::path(get, path = "/api/v1/outputs", tag = "outputs", responses((status = 200, body = [OutputView])))]
pub async fn list(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> Result<Json<Vec<OutputView>>, ApiError> {
    auth.require(Permission::SettingsRead)?;
    Ok(Json(
        state
            .store
            .list_outputs(auth.org_id)
            .await?
            .into_iter()
            .map(OutputView::from)
            .collect(),
    ))
}

#[utoipa::path(post, path = "/api/v1/outputs", tag = "outputs", request_body = OutputRequest, responses((status = 201, body = OutputView)))]
pub async fn create(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    crate::extract::ApiJson(body): crate::extract::ApiJson<OutputRequest>,
) -> Result<(StatusCode, Json<OutputView>), ApiError> {
    auth.require(Permission::SettingsWrite)?;
    let (name, events) = validated(&body)?;
    let url = body
        .webhook_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ApiError::validation(vec![orbit_domain::ValidationIssue::new(
                "webhookUrl",
                "webhookUrl is required",
            )])
        })?;
    guard_webhook_url(url).await?;
    let secret_ref = new_secret_ref("output");
    state
        .store
        .put_secret(
            Some(auth.org_id),
            &secret_ref,
            &SecretValue::new(url),
            &format!("output:{name}"),
        )
        .await?;
    let output = state
        .store
        .create_output(
            auth.org_id,
            &name,
            &body.kind,
            body.agent_id,
            body.team_id,
            &events,
            &secret_ref,
            body.template.as_deref(),
        )
        .await?;
    audit::record(
        &state,
        &auth,
        "output.created",
        Some("output"),
        Some(output.id),
        serde_json::json!({ "name": output.name, "kind": output.kind }),
    )
    .await;
    Ok((StatusCode::CREATED, Json(OutputView::from(output))))
}

#[utoipa::path(patch, path = "/api/v1/outputs/{output_id}", tag = "outputs", request_body = OutputRequest, responses((status = 200, body = OutputView)))]
pub async fn update(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(output_id): Path<Uuid>,
    crate::extract::ApiJson(body): crate::extract::ApiJson<OutputRequest>,
) -> Result<Json<OutputView>, ApiError> {
    auth.require(Permission::SettingsWrite)?;
    let (name, events) = validated(&body)?;
    let secret_ref = match body.webhook_url.as_deref().map(str::trim) {
        Some(url) if !url.is_empty() => {
            guard_webhook_url(url).await?;
            let reference = new_secret_ref("output");
            state
                .store
                .put_secret(
                    Some(auth.org_id),
                    &reference,
                    &SecretValue::new(url),
                    &format!("output:{name}"),
                )
                .await?;
            Some(reference)
        }
        _ => None,
    };
    let output = state
        .store
        .update_output(
            auth.org_id,
            output_id,
            &name,
            &body.kind,
            body.agent_id,
            body.team_id,
            &events,
            secret_ref.as_deref(),
            body.template.as_deref(),
            body.enabled,
        )
        .await?;
    audit::record(
        &state,
        &auth,
        "output.updated",
        Some("output"),
        Some(output.id),
        serde_json::json!({ "name": output.name, "kind": output.kind }),
    )
    .await;
    Ok(Json(OutputView::from(output)))
}

#[utoipa::path(delete, path = "/api/v1/outputs/{output_id}", tag = "outputs", responses((status = 204)))]
pub async fn delete(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(output_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::SettingsWrite)?;
    state.store.delete_output(auth.org_id, output_id).await?;
    audit::record(
        &state,
        &auth,
        "output.deleted",
        Some("output"),
        Some(output_id),
        serde_json::json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/api/v1/outputs/{output_id}/test", tag = "outputs", responses((status = 200)))]
pub async fn test_output(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(output_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::SettingsWrite)?;
    let output = state.store.get_output(auth.org_id, output_id).await?;
    let text = "ORBIT test notification — your output is configured correctly.";
    deliver(&state, &output, text, None, None).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Why a delivery attempt failed, and whether retrying can help.
#[derive(Debug)]
pub struct SendError {
    pub status: Option<u16>,
    pub message: String,
    pub retryable: bool,
}

/// HTTP statuses worth retrying: server errors, timeouts and rate limits. Other 4xx
/// (bad URL, revoked webhook, rejected payload) will not succeed on retry.
fn status_is_retryable(status: u16) -> bool {
    status >= 500 || status == 408 || status == 429
}

/// Delivery used by the test endpoint.
pub async fn deliver(
    state: &AppState,
    output: &Output,
    text: &str,
    execution: Option<&ExecutionRecord>,
    team_run: Option<&TeamRun>,
) -> Result<(), ApiError> {
    send(state, output, text, execution, team_run, None)
        .await
        .map_err(|error| {
            ApiError::new(
                StatusCode::BAD_GATEWAY,
                "output_delivery_failed",
                error.message,
            )
        })
}

/// POST one notification. When `delivery_id` is set it is sent as `Idempotency-Key` and
/// `X-Orbit-Delivery-Id` (and `deliveryId` in generic webhook bodies) so receivers can
/// de-duplicate the at-least-once delivery. Slack/Mattermost ignore unknown headers.
pub async fn send(
    state: &AppState,
    output: &Output,
    text: &str,
    execution: Option<&ExecutionRecord>,
    team_run: Option<&TeamRun>,
    delivery_id: Option<Uuid>,
) -> Result<(), SendError> {
    let url = match state.store.get_secret_value(&output.secret_ref).await {
        Ok(Some(value)) => value.expose().to_string(),
        Ok(None) => {
            return Err(SendError {
                status: None,
                message: "output webhook secret is missing".into(),
                retryable: false,
            })
        }
        Err(error) => {
            return Err(SendError {
                status: None,
                message: error.to_string(),
                retryable: true,
            })
        }
    };
    guard_webhook_url(&url).await.map_err(|error| SendError {
        status: None,
        message: error.message,
        retryable: false,
    })?;
    let client = orbit_security::guarded_client(std::time::Duration::from_secs(15));
    let payload = if output.kind == "webhook" {
        serde_json::json!({
            "event": execution.map(|e| format!("execution.{}", e.status)).unwrap_or_else(|| "test".into()),
            "deliveryId": delivery_id,
            "text": text,
            "execution": execution.map(|e| serde_json::json!({
                "id": e.id, "agentName": e.agent_name, "status": e.status,
                "output": e.output, "error": e.error,
            })),
            "teamRun": team_run.map(|run| serde_json::json!({
                "id": run.id, "teamId": run.team_id, "status": run.status,
                "output": run.output, "error": run.error,
            })),
        })
    } else {
        serde_json::json!({ "text": text })
    };
    let mut request = client.post(&url).json(&payload);
    if let Some(id) = delivery_id {
        request = request
            .header("Idempotency-Key", id.to_string())
            .header("X-Orbit-Delivery-Id", id.to_string());
    }
    let response = request.send().await.map_err(|error| SendError {
        status: None,
        // reqwest errors embed the (secret) webhook URL; strip it.
        message: error.without_url().to_string(),
        retryable: true,
    })?;
    let status = response.status();
    if !status.is_success() {
        return Err(SendError {
            status: Some(status.as_u16()),
            message: format!("webhook returned HTTP {status}"),
            retryable: status_is_retryable(status.as_u16()),
        });
    }
    Ok(())
}

const DELIVERY_LEASE: chrono::Duration = chrono::Duration::seconds(120);
const MAX_DELIVERIES_PER_TICK: usize = 25;

/// Background outbox loop: enqueue finished runs, then claim and deliver due rows.
///
/// Safe to run on every replica and concurrently: rows are claimed one at a time with a
/// lease (`FOR UPDATE SKIP LOCKED`), so each delivery is sent by one worker at a time and a
/// crashed sender's rows are reclaimed after the lease expires (at-least-once).
pub async fn deliver_pending(state: &AppState) {
    if let Err(error) = state.store.enqueue_output_deliveries().await {
        tracing::error!(%error, "output delivery enqueue failed");
    }
    if let Err(error) = state.store.enqueue_team_output_deliveries().await {
        tracing::error!(%error, "team output delivery enqueue failed");
    }
    for _ in 0..MAX_DELIVERIES_PER_TICK {
        let job = match state.store.claim_output_deliveries(1, DELIVERY_LEASE).await {
            Ok(mut jobs) => match jobs.pop() {
                Some(job) => job,
                None => break,
            },
            Err(error) => {
                tracing::error!(%error, "output delivery claim failed");
                break;
            }
        };
        process_delivery(state, job).await;
    }
}

async fn process_delivery(state: &AppState, job: OutputDeliveryJob) {
    let (outcome, status, error) = attempt_delivery(state, &job).await;
    match state
        .store
        .finish_output_delivery(
            job.id,
            job.claim_token,
            outcome,
            status,
            error.as_deref(),
            job.attempts,
        )
        .await
    {
        Ok(true) => {}
        Ok(false) => {
            tracing::warn!(delivery_id = %job.id, "output delivery claim lost before finish");
        }
        Err(error) => {
            // The lease will expire and the row is reclaimed.
            tracing::error!(delivery_id = %job.id, %error, "recording output delivery result failed");
        }
    }
}

type AttemptResult = (DeliveryOutcome, Option<i32>, Option<String>);

async fn attempt_delivery(state: &AppState, job: &OutputDeliveryJob) -> AttemptResult {
    let skipped = |reason: &str| (DeliveryOutcome::Skipped, None, Some(reason.to_string()));
    let retry = |error: StoreError| (DeliveryOutcome::Retry, None, Some(error.to_string()));
    let output = match state.store.get_output(job.org_id, job.output_id).await {
        Ok(output) if output.enabled => output,
        Ok(_) => return skipped("output is disabled"),
        Err(StoreError::NotFound(_)) => return skipped("output was deleted"),
        Err(error) => return retry(error),
    };
    let execution = match job.execution_id {
        Some(id) => match state.store.get_execution_by_id_any_org(id).await {
            Ok(execution) => Some(execution),
            Err(StoreError::NotFound(_)) => return skipped("execution was deleted"),
            Err(error) => return retry(error),
        },
        None => None,
    };
    let team_run = match job.team_run_id {
        Some(id) => match state.store.get_team_run_any_org(id).await {
            Ok(run) => Some(run),
            Err(StoreError::NotFound(_)) => return skipped("team run was deleted"),
            Err(error) => return retry(error),
        },
        None => None,
    };
    let text = execution
        .as_ref()
        .map(summary)
        .or_else(|| team_run.as_ref().map(team_summary))
        .unwrap_or_else(|| "ORBIT run finished".into());
    match send(
        state,
        &output,
        &text,
        execution.as_ref(),
        team_run.as_ref(),
        Some(job.id),
    )
    .await
    {
        Ok(()) => (DeliveryOutcome::Succeeded, None, None),
        Err(error) => {
            let outcome = if error.retryable {
                DeliveryOutcome::Retry
            } else {
                DeliveryOutcome::Failed
            };
            (outcome, error.status.map(i32::from), Some(error.message))
        }
    }
}

fn team_summary(run: &TeamRun) -> String {
    let output = run
        .output
        .as_ref()
        .map(|value| {
            value
                .get("text")
                .or_else(|| value.get("summary"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .unwrap_or_else(|| value.to_string())
        })
        .or_else(|| run.error.clone())
        .unwrap_or_else(|| "completed".into());
    let output: String = output.chars().take(3000).collect();
    format!(
        "Agent team run {} — {}\n{}",
        run.status,
        run.input
            .get("task")
            .and_then(|v| v.as_str())
            .unwrap_or("task"),
        output
    )
}

fn summary(execution: &ExecutionRecord) -> String {
    let output = execution
        .output
        .as_ref()
        .map(|value| {
            value
                .get("text")
                .or_else(|| value.get("summary"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .unwrap_or_else(|| value.to_string())
        })
        .or_else(|| execution.error.as_ref().map(|error| error.to_string()))
        .unwrap_or_else(|| "completed".into());
    let output: String = output.chars().take(3000).collect();
    format!(
        "{} {} — {}\\n{}",
        execution.agent_name,
        execution.status,
        execution
            .input
            .get("task")
            .and_then(|v| v.as_str())
            .unwrap_or("task"),
        output
    )
}

/// Refuse webhook targets in loopback, link-local/metadata or other blocked ranges
/// (see `ORBIT_OUTBOUND_*`). Private ranges are allowed by default.
async fn guard_webhook_url(url: &str) -> Result<(), ApiError> {
    orbit_security::check_outbound_url(url, &orbit_security::OutboundPolicy::from_env())
        .await
        .map_err(|error| {
            ApiError::validation(vec![ValidationIssue::new("webhookUrl", error.to_string())])
        })
}
