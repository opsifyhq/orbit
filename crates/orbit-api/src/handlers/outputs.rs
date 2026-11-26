//! Execution result outputs (Slack/Mattermost/webhook/Telegram) and delivery.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use orbit_contracts::{OutputEvent, OutputKind, TelegramTarget};
use orbit_domain::ValidationIssue;
use orbit_security::{new_secret_ref, AuthContext, Permission, SecretValue};
use orbit_store::{Output, OutputDelivery};

use crate::audit;
use crate::error::ApiError;
use crate::output_delivery::{deliver, guard_credential, redact_urls, with_note};
use crate::paging::{Page, PageQuery};
use crate::patch::{merge_patch, parse_patch};
use crate::state::AppState;

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OutputRequest {
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub agent_id: Option<Uuid>,
    /// Multiple agent targets. Takes precedence over the legacy `agentId`.
    #[serde(default)]
    pub agent_ids: Vec<Uuid>,
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
    /// Non-secret S3 settings: bucket, endpoint, region, keyPrefix, pathStyle, etc. On
    /// update, absent or `null` keeps the stored settings and an object replaces them.
    #[serde(default)]
    pub config: Option<serde_json::Value>,
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
    pub agent_ids: Vec<Uuid>,
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

impl From<Output> for OutputView {
    fn from(output: Output) -> Self {
        Self {
            id: output.id,
            name: output.name,
            kind: output.kind,
            agent_id: output.agent_id,
            agent_ids: output.agent_ids,
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
    let target_agents = if body.agent_ids.is_empty() {
        body.agent_id.into_iter().collect::<Vec<_>>()
    } else {
        body.agent_ids.clone()
    };
    if !target_agents.is_empty() && body.team_id.is_some() {
        issues.push(ValidationIssue::new(
            "target",
            "choose agents or a team, not both",
        ));
    }
    if name.is_empty() {
        issues.push(ValidationIssue::new("name", "name is required"));
    }
    issues.extend(orbit_domain::name_length_issue("name", &name));
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
    agent_ids: &[Uuid],
    team_id: Option<Uuid>,
) -> Result<(), ApiError> {
    for agent_id in agent_ids {
        let agent = state.store.get_agent(auth.org_id, *agent_id).await?;
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
    if agent_ids.is_empty() && team_id.is_none() && !access.is_privileged() {
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
    if (output.agent_ids.is_empty() && output.team_id.is_none())
        || !output_target_visible(state, auth, access, &output).await
    {
        return Err(ApiError::not_found("output"));
    }
    require_target_manage(state, auth, access, &output.agent_ids, output.team_id).await?;
    Ok(output)
}

async fn output_target_visible(
    state: &AppState,
    auth: &AuthContext,
    access: &crate::access::AccessScope,
    output: &Output,
) -> bool {
    for agent_id in &output.agent_ids {
        if let Ok(agent) = state.store.get_agent(auth.org_id, *agent_id).await {
            if access.can_view(agent.owner_user_id, agent.owner_team_id, &agent.visibility) {
                return true;
            }
        }
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

#[utoipa::path(get, path = "/api/v1/outputs", tag = "outputs", params(PageQuery), responses((status = 200, body = Page<OutputView>)))]
pub async fn list(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<PageQuery>,
) -> Result<Json<Page<OutputView>>, ApiError> {
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
    Ok(Json(Page::from_vec(visible, &query, 100)?))
}

#[utoipa::path(
    get,
    path = "/api/v1/outputs/{output_id}/deliveries",
    tag = "outputs",
    params(("output_id" = Uuid, Path), PageQuery),
    responses((status = 200, body = Page<OutputDeliveryView>), (status = 404))
)]
pub async fn deliveries(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(output_id): Path<Uuid>,
    Query(query): Query<PageQuery>,
) -> Result<Json<Page<OutputDeliveryView>>, ApiError> {
    auth.require(Permission::SettingsRead)?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    let output = state.store.get_output(auth.org_id, output_id).await?;
    if !access.is_privileged() && !output_target_visible(&state, &auth, &access, &output).await {
        return Err(ApiError::not_found("output"));
    }
    // An agent or team output only reports on runs of a target the caller can see. An
    // organisation-wide one reports on every agent's runs, so only an admin sees which.
    let with_run =
        access.is_privileged() || !output.agent_ids.is_empty() || output.team_id.is_some();
    let rows = state
        .store
        .list_output_deliveries(auth.org_id, output_id, crate::paging::MAX_PAGE_LIMIT)
        .await?;
    let items: Vec<OutputDeliveryView> = rows
        .into_iter()
        .map(|delivery| OutputDeliveryView::from_delivery(delivery, with_run))
        .collect();
    Ok(Json(Page::from_window(items, &query, 50)?))
}

/// The request's `config`: `None` when absent or `null`, an object as given, anything
/// else a validation error.
fn request_config(body: &OutputRequest) -> Result<Option<serde_json::Value>, ApiError> {
    match &body.config {
        None => Ok(None),
        Some(config) if config.is_object() => Ok(Some(config.clone())),
        Some(_) => Err(ApiError::validation(vec![ValidationIssue::new(
            "config",
            "config must be an object",
        )])),
    }
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
    let target_agents = if body.agent_ids.is_empty() {
        body.agent_id.into_iter().collect::<Vec<_>>()
    } else {
        body.agent_ids.clone()
    };
    require_target_manage(&state, &auth, &access, &target_agents, body.team_id).await?;
    let value = credential(&body, kind).ok_or_else(|| {
        ApiError::validation(vec![ValidationIssue::new(
            credential_field(kind),
            format!("{} is required", credential_field(kind)),
        )])
    })?;
    guard_credential(kind, value, &state.outbound).await?;
    let config = request_config(&body)?.unwrap_or_else(|| serde_json::json!({}));
    if kind == OutputKind::S3 {
        orbit_s3::S3Config::for_output(&config, Some(value)).map_err(|message| {
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
            &config,
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
    let output = state
        .store
        .set_output_agent_ids(auth.org_id, output.id, &target_agents)
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

/// A PATCH body is merged onto the stored output. Credentials are write-only, so they
/// are accepted as extra keys but never filled from the stored row (#443).
fn stored_output_request(output: &Output) -> serde_json::Value {
    serde_json::json!({
        "name": output.name,
        "kind": output.kind,
        "agentId": output.agent_id,
        "agentIds": output.agent_ids,
        "teamId": output.team_id,
        "events": output.events,
        "chatId": output.telegram_chat_id,
        "messageThreadId": output.telegram_message_thread_id,
        "config": output.config,
        "template": output.template,
        "enabled": output.enabled,
    })
}

#[utoipa::path(patch, path = "/api/v1/outputs/{output_id}", tag = "outputs", request_body = OutputRequest, responses((status = 200, body = OutputView)))]
pub async fn update(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(output_id): Path<Uuid>,
    crate::extract::ApiJson(patch): crate::extract::ApiJson<serde_json::Value>,
) -> Result<Json<OutputView>, ApiError> {
    auth.require(Permission::SettingsWrite)?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    let existing = load_managed_output(&state, &auth, &access, output_id).await?;
    // A target is either one agent (`agentId`) or several (`agentIds`). A partial patch
    // that sets one must clear the other, or the stored target would win.
    let patch_sets_agent_ids = patch.get("agentIds").is_some();
    let patch_sets_agent_id = patch.get("agentId").is_some();
    let mut merged = merge_patch(
        stored_output_request(&existing),
        patch,
        &["webhookUrl", "botToken", "s3Credential"],
    )?;
    if let Some(object) = merged.as_object_mut() {
        if patch_sets_agent_ids {
            object.insert("agentId".into(), serde_json::Value::Null);
        }
        if patch_sets_agent_id {
            object.insert("agentIds".into(), serde_json::json!([]));
        }
    }
    let body: OutputRequest = parse_patch(merged)?;
    let (name, events, kind, telegram) = validated(&body)?;
    let target_agents = if body.agent_ids.is_empty() {
        body.agent_id.into_iter().collect::<Vec<_>>()
    } else {
        body.agent_ids.clone()
    };
    require_target_manage(&state, &auth, &access, &target_agents, body.team_id).await?;
    // An update without `config` (the on/off toggle, a rename) keeps the stored settings;
    // only an object replaces them. Changing kind starts from `{}` (#390).
    let config = match request_config(&body)? {
        Some(config) => config,
        None if existing.kind == kind.as_str() => existing.config.clone(),
        None => serde_json::json!({}),
    };
    if kind == OutputKind::S3 {
        // Checked with or without a new credential: an S3 output that cannot write to its
        // own bucket is refused instead of failing every delivery (#386).
        orbit_s3::S3Config::parse_output(&config).map_err(|message| {
            ApiError::validation(vec![ValidationIssue::new("config", message)])
        })?;
    }
    let secret_ref = if let Some(value) = credential(&body, kind) {
        guard_credential(kind, value, &state.outbound).await?;
        if kind == OutputKind::S3 {
            orbit_s3::S3Config::for_output(&config, Some(value)).map_err(|message| {
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
            &config,
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
    let output = state
        .store
        .set_output_agent_ids(auth.org_id, output.id, &target_agents)
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

#[utoipa::path(post, path = "/api/v1/outputs/{output_id}/test", tag = "outputs", responses((status = 204, description = "The test notification was delivered")))]
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
