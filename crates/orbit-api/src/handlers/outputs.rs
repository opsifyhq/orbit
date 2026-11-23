//! Execution result outputs (Slack/Mattermost/webhook/Telegram) and delivery.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use orbit_contracts::{
    redact_secret, telegram_error, telegram_message_parts, OutputEvent, OutputKind, TelegramTarget,
};
use orbit_domain::ValidationIssue;
use orbit_security::{new_secret_ref, AuthContext, OutboundPolicy, Permission, SecretValue};
use orbit_store::outputs::DeliveryOutcome;
use orbit_store::{
    ExecutionRecord, Output, OutputDelivery, OutputDeliveryJob, Store, StoreError, TeamRun,
};

use crate::audit;
use crate::error::ApiError;
use crate::state::AppState;

/// Default Bot API host. Override with `ORBIT_TELEGRAM_API_BASE_URL` only for an explicit
/// admin-controlled proxy or test server; the override is still passed through the outbound
/// URL guard on every request.
const TELEGRAM_API_BASE_URL: &str = "https://api.telegram.org";

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
    /// Incoming webhook / target URL for webhook kinds. Write-only; never returned.
    #[serde(default)]
    pub webhook_url: Option<String>,
    /// Telegram bot token. Write-only; never returned, logged or published in events.
    #[serde(default)]
    pub bot_token: Option<String>,
    /// S3 credential as JSON: `{ accessKeyId, secretAccessKey, sessionToken? }`.
    #[serde(default)]
    pub s3_credential: Option<String>,
    /// Non-secret S3 settings: bucket, endpoint, region, keyPrefix, pathStyle, etc.
    #[serde(default)]
    pub config: serde_json::Value,
    /// Numeric Telegram user/group/channel id (for example `-100…`).
    #[serde(default)]
    pub chat_id: Option<String>,
    /// Optional numeric Telegram forum topic id.
    #[serde(default)]
    pub message_thread_id: Option<String>,
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
    /// True when a webhook URL or Telegram bot token is stored. The value itself is never
    /// returned.
    pub has_webhook: bool,
    pub chat_id: Option<String>,
    pub message_thread_id: Option<String>,
    /// Non-secret configuration for S3 (empty for other kinds).
    pub config: serde_json::Value,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// The newest delivery, without run details. Only in list responses, and only once the
    /// output has delivered (or tried to).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_delivery: Option<OutputDeliveryView>,
}

/// One delivery of an output: whether the message arrived, and why not.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OutputDeliveryView {
    pub id: Uuid,
    /// `pending` (waiting, or retrying after an error), `sending`, `succeeded`, `failed`
    /// (refused, not retried), `dead` (retries used up) or `skipped` (output off or deleted).
    pub status: String,
    pub attempts: i32,
    /// HTTP status of the last attempt, when the destination answered.
    pub response_status: Option<i32>,
    /// Why the last attempt failed. Never contains the webhook URL.
    pub error: Option<String>,
    /// When a pending delivery is tried next.
    pub next_attempt_at: Option<chrono::DateTime<chrono::Utc>>,
    /// The run it reports on, only when the caller may see it.
    pub execution_id: Option<Uuid>,
    pub team_run_id: Option<Uuid>,
    pub task: Option<String>,
    pub agent_name: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl OutputDeliveryView {
    fn from_delivery(delivery: OutputDelivery, with_run: bool) -> Self {
        Self {
            id: delivery.id,
            next_attempt_at: (delivery.status == "pending").then_some(delivery.next_attempt_at),
            status: delivery.status,
            attempts: delivery.attempts,
            response_status: delivery.response_status,
            error: delivery.last_error.as_deref().map(redact_urls),
            execution_id: delivery.execution_id.filter(|_| with_run),
            team_run_id: delivery.team_run_id.filter(|_| with_run),
            task: delivery.task.filter(|_| with_run),
            agent_name: delivery.agent_name.filter(|_| with_run),
            created_at: delivery.created_at,
            updated_at: delivery.updated_at,
        }
    }
}

/// Replace every http(s) URL in an error with `<url>`. Send errors already drop the
/// webhook URL; this also covers rows recorded before that, since the URL is a secret.
fn redact_urls(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = ["http://", "https://"]
        .iter()
        .filter_map(|scheme| rest.find(scheme))
        .min()
    {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let end = tail
            .find(|c: char| c.is_whitespace() || matches!(c, ')' | '"' | '\'' | '>' | ','))
            .unwrap_or(tail.len());
        out.push_str("<url>");
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
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
            chat_id: output.telegram_chat_id,
            message_thread_id: output.telegram_message_thread_id,
            config: output.config,
            created_at: output.created_at,
            updated_at: output.updated_at,
            last_delivery: None,
        }
    }
}

fn credential_field(kind: OutputKind) -> &'static str {
    match kind {
        OutputKind::Telegram => "botToken",
        OutputKind::S3 => "s3Credential",
        _ => "webhookUrl",
    }
}

fn credential(body: &OutputRequest, kind: OutputKind) -> Option<&str> {
    match kind {
        OutputKind::Telegram => body.bot_token.as_deref(),
        OutputKind::S3 => body.s3_credential.as_deref(),
        _ => body.webhook_url.as_deref(),
    }
    .map(str::trim)
    .filter(|value| !value.is_empty())
}

async fn guard_credential(kind: OutputKind, value: &str) -> Result<(), ApiError> {
    match kind {
        OutputKind::Telegram => {
            // Telegram tokens are `<numeric id>:<secret>`; rejecting URL metacharacters here
            // prevents the token from altering the assembled Bot API request path.
            let mut parts = value.split(':');
            let id = parts.next().unwrap_or_default();
            let secret = parts.next().unwrap_or_default();
            let valid = !id.is_empty()
                && id.bytes().all(|byte| byte.is_ascii_digit())
                && !secret.is_empty()
                && secret
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
                && parts.next().is_none();
            if !valid {
                return Err(ApiError::validation(vec![ValidationIssue::new(
                    "botToken",
                    "botToken must be a Telegram bot token (numeric id:secret)",
                )]));
            }
            Ok(())
        }
        OutputKind::S3 => {
            orbit_s3::S3Credentials::from_secret(value).map_err(|message| {
                ApiError::validation(vec![ValidationIssue::new("s3Credential", message)])
            })?;
            Ok(())
        }
        _ => guard_webhook_url(value).await,
    }
}

async fn store_credential(
    state: &AppState,
    org_id: Uuid,
    name: &str,
    value: &str,
) -> Result<String, ApiError> {
    let secret_ref = new_secret_ref("output");
    state
        .store
        .put_secret(
            Some(org_id),
            &secret_ref,
            &SecretValue::new(value),
            &format!("output:{name}"),
        )
        .await?;
    Ok(secret_ref)
}

fn validated(
    body: &OutputRequest,
) -> Result<(String, Vec<String>, OutputKind, Option<TelegramTarget>), ApiError> {
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
    let Some(kind) = OutputKind::parse(&body.kind) else {
        issues.push(ValidationIssue::new(
            "kind",
            "kind must be slack, mattermost, webhook, telegram or s3",
        ));
        return Err(ApiError::validation(issues));
    };
    let mut events = if body.events.is_empty() {
        OutputEvent::ALL
            .iter()
            .map(|event| (*event).to_string())
            .collect()
    } else {
        body.events.clone()
    };
    events.sort();
    events.dedup();
    for event in &events {
        if OutputEvent::parse(event).is_none() {
            issues.push(ValidationIssue::new(
                "events",
                format!("unknown event '{event}'"),
            ));
        }
    }
    let telegram = if kind == OutputKind::Telegram {
        match TelegramTarget::parse(
            body.chat_id.as_deref().unwrap_or(""),
            body.message_thread_id.as_deref(),
        ) {
            Ok(target) => Some(target),
            Err(message) => {
                let field = if message.contains("messageThreadId") {
                    "messageThreadId"
                } else {
                    "chatId"
                };
                issues.push(ValidationIssue::new(field, message));
                None
            }
        }
    } else {
        None
    };
    if !issues.is_empty() {
        return Err(ApiError::validation(issues));
    }
    Ok((name, events, kind, telegram))
}

/// An output forwards each run's task and result to an external URL, so attaching one
/// to an agent or agent team requires manage access to it (the same bar as attaching a
/// trigger). An output with neither target applies to every execution in the
/// organisation and may only be created or changed by an owner/admin.
async fn require_target_manage(
    state: &AppState,
    auth: &AuthContext,
    access: &crate::access::AccessScope,
    agent_id: Option<Uuid>,
    team_id: Option<Uuid>,
) -> Result<(), ApiError> {
    if let Some(agent_id) = agent_id {
        let agent = state.store.get_agent(auth.org_id, agent_id).await?;
        access.require_manage(
            "agent",
            agent.owner_user_id,
            agent.owner_team_id,
            &agent.visibility,
        )?;
    }
    if let Some(team_id) = team_id {
        let team = state.store.get_agent_team(auth.org_id, team_id).await?;
        access.require_manage(
            "agent team",
            team.owner_user_id,
            team.owner_team_id,
            &team.visibility,
        )?;
    }
    if agent_id.is_none() && team_id.is_none() && !access.is_privileged() {
        return Err(ApiError::forbidden(
            "only an owner or admin can manage an organisation-wide output",
        ));
    }
    Ok(())
}

/// Loads an existing output and requires manage access to the target it has today.
async fn load_managed_output(
    state: &AppState,
    auth: &AuthContext,
    access: &crate::access::AccessScope,
    output_id: Uuid,
) -> Result<Output, ApiError> {
    let output = state.store.get_output(auth.org_id, output_id).await?;
    if access.is_privileged() {
        return Ok(output);
    }
    // A caller who cannot even see the target gets the same 404 as for a missing output.
    if (output.agent_id.is_none() && output.team_id.is_none())
        || !output_target_visible(state, auth, access, &output).await
    {
        return Err(ApiError::not_found("output"));
    }
    require_target_manage(state, auth, access, output.agent_id, output.team_id).await?;
    Ok(output)
}

async fn output_target_visible(
    state: &AppState,
    auth: &AuthContext,
    access: &crate::access::AccessScope,
    output: &Output,
) -> bool {
    if let Some(agent_id) = output.agent_id {
        return match state.store.get_agent(auth.org_id, agent_id).await {
            Ok(agent) => {
                access.can_view(agent.owner_user_id, agent.owner_team_id, &agent.visibility)
            }
            Err(_) => false,
        };
    }
    if let Some(team_id) = output.team_id {
        return match state.store.get_agent_team(auth.org_id, team_id).await {
            Ok(team) => access.can_view(team.owner_user_id, team.owner_team_id, &team.visibility),
            Err(_) => false,
        };
    }
    // Org-wide outputs carry no secrets in the view and describe a data flow every
    // member's runs are subject to, so they stay listed for everyone with settings:read.
    true
}

#[utoipa::path(get, path = "/api/v1/outputs", tag = "outputs", responses((status = 200, body = [OutputView])))]
pub async fn list(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> Result<Json<Vec<OutputView>>, ApiError> {
    auth.require(Permission::SettingsRead)?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    // One visibility query for the whole list instead of one lookup per output.
    let visible_ids: std::collections::HashSet<Uuid> = state
        .store
        .list_visible_output_ids(auth.org_id, Some(&access.store_access()))
        .await?
        .into_iter()
        .collect();
    let mut latest: std::collections::HashMap<Uuid, OutputDelivery> = state
        .store
        .latest_output_deliveries(auth.org_id)
        .await?
        .into_iter()
        .map(|delivery| (delivery.output_id, delivery))
        .collect();
    let mut visible = Vec::new();
    for output in state.store.list_outputs(auth.org_id).await? {
        if !visible_ids.contains(&output.id) {
            continue;
        }
        let last = latest.remove(&output.id);
        let mut view = OutputView::from(output);
        view.last_delivery =
            last.map(|delivery| OutputDeliveryView::from_delivery(delivery, false));
        visible.push(view);
    }
    Ok(Json(visible))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct DeliveriesQuery {
    /// How many deliveries to return, newest first (1–200, default 50).
    pub limit: Option<i64>,
}

#[utoipa::path(
    get,
    path = "/api/v1/outputs/{output_id}/deliveries",
    tag = "outputs",
    params(("output_id" = Uuid, Path), DeliveriesQuery),
    responses((status = 200, body = [OutputDeliveryView]), (status = 404))
)]
pub async fn deliveries(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(output_id): Path<Uuid>,
    Query(query): Query<DeliveriesQuery>,
) -> Result<Json<Vec<OutputDeliveryView>>, ApiError> {
    auth.require(Permission::SettingsRead)?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    let output = state.store.get_output(auth.org_id, output_id).await?;
    if !access.is_privileged() && !output_target_visible(&state, &auth, &access, &output).await {
        return Err(ApiError::not_found("output"));
    }
    // An agent or team output only reports on runs of a target the caller can see. An
    // organisation-wide one reports on every agent's runs, so only an admin sees which.
    let with_run = access.is_privileged() || output.agent_id.is_some() || output.team_id.is_some();
    let rows = state
        .store
        .list_output_deliveries(auth.org_id, output_id, query.limit.unwrap_or(50))
        .await?;
    Ok(Json(
        rows.into_iter()
            .map(|delivery| OutputDeliveryView::from_delivery(delivery, with_run))
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
    let (name, events, kind, telegram) = validated(&body)?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    require_target_manage(&state, &auth, &access, body.agent_id, body.team_id).await?;
    let value = credential(&body, kind).ok_or_else(|| {
        ApiError::validation(vec![ValidationIssue::new(
            credential_field(kind),
            format!("{} is required", credential_field(kind)),
        )])
    })?;
    guard_credential(kind, value).await?;
    if kind == OutputKind::S3 {
        orbit_s3::S3Config::from_connection(&body.config, Some(value)).map_err(|message| {
            ApiError::validation(vec![ValidationIssue::new("config", message)])
        })?;
    }
    let secret_ref = store_credential(&state, auth.org_id, &name, value).await?;
    let output = match state
        .store
        .create_output(
            auth.org_id,
            &name,
            kind.as_str(),
            body.agent_id,
            body.team_id,
            &events,
            &secret_ref,
            body.template.as_deref(),
            telegram.as_ref().map(|target| target.chat_id.as_str()),
            telegram
                .as_ref()
                .and_then(|target| target.message_thread_id.as_deref()),
            body.enabled,
        )
        .await
    {
        Ok(output) => output,
        Err(error) => {
            // The secret was written first; do not leave it behind when the row is refused.
            let _ = state.store.delete_secret(&secret_ref).await;
            return Err(error.into());
        }
    };
    let config = if body.config.is_object() {
        body.config.clone()
    } else {
        serde_json::json!({})
    };
    let output = state
        .store
        .set_output_config(auth.org_id, output.id, &config)
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
    let (name, events, kind, telegram) = validated(&body)?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    let existing = load_managed_output(&state, &auth, &access, output_id).await?;
    require_target_manage(&state, &auth, &access, body.agent_id, body.team_id).await?;
    let secret_ref = if let Some(value) = credential(&body, kind) {
        guard_credential(kind, value).await?;
        if kind == OutputKind::S3 {
            orbit_s3::S3Config::from_connection(&body.config, Some(value)).map_err(|message| {
                ApiError::validation(vec![ValidationIssue::new("config", message)])
            })?;
        }
        Some(store_credential(&state, auth.org_id, &name, value).await?)
    } else if existing.kind != kind.as_str() {
        // Switching kinds must not reuse a stored webhook URL as a bot token (or vice versa).
        return Err(ApiError::validation(vec![ValidationIssue::new(
            credential_field(kind),
            format!(
                "{} is required when changing the output kind",
                credential_field(kind)
            ),
        )]));
    } else {
        None
    };
    let output = match state
        .store
        .update_output(
            auth.org_id,
            output_id,
            &name,
            kind.as_str(),
            body.agent_id,
            body.team_id,
            &events,
            secret_ref.as_deref(),
            body.template.as_deref(),
            telegram.as_ref().map(|target| target.chat_id.as_str()),
            telegram
                .as_ref()
                .and_then(|target| target.message_thread_id.as_deref()),
            body.enabled,
        )
        .await
    {
        Ok(output) => output,
        Err(error) => {
            if let Some(reference) = secret_ref.as_deref() {
                let _ = state.store.delete_secret(reference).await;
            }
            return Err(error.into());
        }
    };
    let config = if body.config.is_object() {
        body.config.clone()
    } else {
        serde_json::json!({})
    };
    let output = state
        .store
        .set_output_config(auth.org_id, output.id, &config)
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
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    load_managed_output(&state, &auth, &access, output_id).await?;
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
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    let output = load_managed_output(&state, &auth, &access, output_id).await?;
    let text = with_note(
        "ORBIT test notification — your output is configured correctly.".into(),
        output.template.as_deref(),
    );
    deliver(&state, &output, &text, None, None).await?;
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
    send(state, output, text, execution, team_run, None, None)
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
/// de-duplicate the at-least-once delivery. Telegram/Slack/Mattermost ignore the headers.
pub async fn send(
    state: &AppState,
    output: &Output,
    text: &str,
    execution: Option<&ExecutionRecord>,
    team_run: Option<&TeamRun>,
    delivery_id: Option<Uuid>,
    progress: Option<TelegramProgress<'_>>,
) -> Result<(), SendError> {
    let kind = OutputKind::parse(&output.kind).ok_or_else(|| SendError {
        status: None,
        message: format!("unsupported output kind '{}'", output.kind),
        retryable: false,
    })?;
    let secret = match state.store.get_secret_value(&output.secret_ref).await {
        Ok(Some(value)) => value.expose().to_string(),
        Ok(None) => {
            return Err(SendError {
                status: None,
                message: "output credential is missing".into(),
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
    if kind == OutputKind::S3 {
        let (config, credentials) =
            orbit_s3::S3Config::from_connection(&output.config, Some(&secret)).map_err(
                |message| SendError {
                    status: None,
                    message,
                    retryable: false,
                },
            )?;
        let client = orbit_s3::S3Client::new(config, credentials).map_err(|message| SendError {
            status: None,
            message,
            retryable: false,
        })?;
        let key = format!(
            "{}{}.txt",
            client.config().key_prefix,
            delivery_id.unwrap_or_else(Uuid::now_v7)
        );
        client
            .put_object(None, &key, text.as_bytes(), "text/plain; charset=utf-8")
            .await
            .map_err(|message| SendError {
                status: None,
                message,
                retryable: true,
            })?;
        return Ok(());
    }
    if kind == OutputKind::Telegram {
        let target = TelegramTarget::parse(
            output.telegram_chat_id.as_deref().unwrap_or(""),
            output.telegram_message_thread_id.as_deref(),
        )
        .map_err(|message| SendError {
            status: None,
            message: message.to_string(),
            retryable: false,
        })?;
        return send_telegram_with_progress(
            &TelegramEndpoint::from_env(),
            &secret,
            &target,
            text,
            delivery_id,
            progress,
        )
        .await;
    }
    guard_webhook_url(&secret)
        .await
        .map_err(|error| SendError {
            status: None,
            message: error.message,
            retryable: false,
        })?;
    let client = orbit_security::guarded_client(std::time::Duration::from_secs(15));
    let payload = if kind == OutputKind::Webhook {
        serde_json::json!({
            "event": execution
                .map(|e| format!("execution.{}", e.status))
                .or_else(|| team_run.map(|run| format!("team_run.{}", run.status)))
                .unwrap_or_else(|| "test".into()),
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
    let mut request = client.post(&secret).json(&payload);
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

/// Operator-controlled Telegram endpoint. The base URL defaults to the official Bot API;
/// an admin may set `ORBIT_TELEGRAM_API_BASE_URL` (for a proxy or mock server), but the
/// fully assembled URL is always checked against the outbound policy before any request.
#[derive(Debug, Clone)]
pub struct TelegramEndpoint {
    pub base_url: String,
    policy: OutboundPolicy,
}

impl TelegramEndpoint {
    pub fn from_env() -> Self {
        let base_url = std::env::var("ORBIT_TELEGRAM_API_BASE_URL")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| TELEGRAM_API_BASE_URL.to_string());
        Self::new(base_url, OutboundPolicy::from_env())
    }

    pub fn new(base_url: impl Into<String>, policy: OutboundPolicy) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            policy,
        }
    }
}

/// Resume state for a claimed multi-part Telegram delivery. `store` is `None` for direct
/// sends (tests and the test endpoint), which then always start at part zero.
#[derive(Clone, Copy)]
pub struct TelegramProgress<'a> {
    pub store: Option<&'a Store>,
    pub delivery_id: Uuid,
    pub claim_token: Uuid,
    pub parts_sent: i32,
}

/// Send one Telegram notification, splitting long text into multiple `sendMessage` calls.
/// The bot token is only ever used in the request URL, never in errors or diagnostics.
pub async fn send_telegram(
    endpoint: &TelegramEndpoint,
    bot_token: &str,
    target: &TelegramTarget,
    text: &str,
    delivery_id: Option<Uuid>,
) -> Result<(), SendError> {
    send_telegram_with_progress(endpoint, bot_token, target, text, delivery_id, None).await
}

/// Send a Telegram notification, resuming after the parts already accepted on a previous
/// attempt and durably counting each newly accepted part as it goes.
pub async fn send_telegram_with_progress(
    endpoint: &TelegramEndpoint,
    bot_token: &str,
    target: &TelegramTarget,
    text: &str,
    delivery_id: Option<Uuid>,
    progress: Option<TelegramProgress<'_>>,
) -> Result<(), SendError> {
    let url = format!("{}/bot{}/sendMessage", endpoint.base_url, bot_token);
    orbit_security::check_outbound_url(&url, &endpoint.policy)
        .await
        .map_err(|error| SendError {
            status: None,
            message: format!(
                "telegram endpoint refused by outbound policy ({})",
                error.category()
            ),
            retryable: false,
        })?;
    let client = orbit_security::guarded_client_with_policy(
        std::time::Duration::from_secs(15),
        endpoint.policy.clone(),
    );
    let parts = telegram_message_parts(text, delivery_id);
    let resume_after = progress
        .as_ref()
        .map_or(0, |progress| progress.parts_sent.max(0) as usize);
    for (index, part) in parts.iter().enumerate() {
        if index < resume_after {
            continue;
        }
        let mut payload = serde_json::json!({
            "chat_id": target.chat_id,
            "text": part,
            "disable_web_page_preview": true,
        });
        if let Some(thread_id) = &target.message_thread_id {
            payload["message_thread_id"] = serde_json::Value::String(thread_id.clone());
        }
        let mut request = client.post(&url).json(&payload);
        if let Some(id) = delivery_id {
            request = request
                .header("Idempotency-Key", format!("{id}-{index}"))
                .header("X-Orbit-Delivery-Id", id.to_string());
        }
        let response = request.send().await.map_err(|error| SendError {
            status: None,
            message: redact_secret(&orbit_security::describe_request_error(&error), bot_token),
            retryable: true,
        })?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let description = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|value| {
                    value
                        .get("description")
                        .and_then(|description| description.as_str())
                        .map(str::to_string)
                });
            let mapped = telegram_error(status.as_u16(), description.as_deref(), bot_token);
            return Err(SendError {
                status: Some(status.as_u16()),
                message: mapped.message,
                retryable: mapped.retryable,
            });
        }
        if let Some(progress) = &progress {
            let Some(store) = progress.store else {
                continue;
            };
            match store
                .record_output_delivery_part(progress.delivery_id, progress.claim_token)
                .await
            {
                Ok(true) => {}
                Ok(false) => {
                    return Err(SendError {
                        status: None,
                        message: "delivery claim lost while sending Telegram part".into(),
                        retryable: false,
                    })
                }
                Err(error) => {
                    return Err(SendError {
                        status: None,
                        message: format!("could not record Telegram part: {error}"),
                        retryable: true,
                    })
                }
            }
        }
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
    let text = with_note(text, output.template.as_deref());
    let progress = TelegramProgress {
        store: Some(&state.store),
        delivery_id: job.id,
        claim_token: job.claim_token,
        parts_sent: job.parts_sent,
    };
    match send(
        state,
        &output,
        &text,
        execution.as_ref(),
        team_run.as_ref(),
        Some(job.id),
        Some(progress),
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

/// Append the output's note (stored as `template`) after the run summary, as the UI
/// promises. A blank note adds nothing.
fn with_note(text: String, note: Option<&str>) -> String {
    match note.map(str::trim).filter(|note| !note.is_empty()) {
        Some(note) => format!("{text}\n\n{note}"),
        None => text,
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
        "{} {} — {}\n{}",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_is_appended_after_the_summary() {
        assert_eq!(
            with_note(
                "Agent succeeded — task\nDone".into(),
                Some("  Ping #release  ")
            ),
            "Agent succeeded — task\nDone\n\nPing #release"
        );
    }

    #[test]
    fn urls_are_redacted_from_errors() {
        assert_eq!(
            super::redact_urls(
                "error sending request for url (https://hooks.slack.com/services/T/B/secret): timed out"
            ),
            "error sending request for url (<url>): timed out"
        );
        assert_eq!(
            super::redact_urls("a http://x/1 and https://y/2"),
            "a <url> and <url>"
        );
        assert_eq!(
            super::redact_urls("webhook returned HTTP 404"),
            "webhook returned HTTP 404"
        );
    }

    #[test]
    fn blank_note_adds_nothing() {
        assert_eq!(with_note("summary".into(), None), "summary");
        assert_eq!(with_note("summary".into(), Some("   ")), "summary");
    }

    #[test]
    fn telegram_endpoint_defaults_to_the_official_host() {
        let endpoint = TelegramEndpoint::new("https://proxy.example/", OutboundPolicy::default());
        assert_eq!(endpoint.base_url, "https://proxy.example");
    }

    #[tokio::test]
    async fn telegram_tokens_cannot_alter_the_request_path() {
        assert!(guard_credential(OutputKind::Telegram, "123456:ABC_def-1")
            .await
            .is_ok());
        for token in [
            "123:abc/../../evil",
            "123:abc?x=1",
            "abc:secret",
            "123:",
            ":secret",
        ] {
            assert!(
                guard_credential(OutputKind::Telegram, token).await.is_err(),
                "{token} must be rejected"
            );
        }
    }
}
