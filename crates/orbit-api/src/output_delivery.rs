//! Output delivery: sending execution and team-run results to Slack, Mattermost,
//! webhooks and Telegram, and the background loop that drains the delivery queue.

use crate::error::ApiError;
use crate::state::AppState;
use axum::http::StatusCode;
use orbit_contracts::{
    redact_secret, telegram_error, telegram_message_parts, OutputKind, TelegramTarget,
};
use orbit_domain::ValidationIssue;
use orbit_security::OutboundPolicy;
use orbit_store::outputs::DeliveryOutcome;
use orbit_store::{ExecutionRecord, Output, OutputDeliveryJob, Store, StoreError, TeamRun};
use uuid::Uuid;

/// Default Bot API host. Override with `ORBIT_TELEGRAM_API_BASE_URL` only for an explicit
/// admin-controlled proxy or test server; the override is still passed through the outbound
/// URL guard on every request.
const TELEGRAM_API_BASE_URL: &str = "https://api.telegram.org";

/// Replace every http(s) URL in an error with `<url>`. Send errors already drop the
/// webhook URL; this also covers rows recorded before that, since the URL is a secret.
pub(crate) fn redact_urls(text: &str) -> String {
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
        let (config, credentials) = orbit_s3::S3Config::for_output(&output.config, Some(&secret))
            .map_err(|message| SendError {
            status: None,
            message,
            retryable: false,
        })?;
        let client = orbit_s3::S3Client::with_policy(config, credentials, state.outbound.clone())
            .map_err(|message| SendError {
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
            .map_err(|error| SendError {
                status: error.status,
                message: error.message,
                retryable: error.retryable,
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
    guard_webhook_url(&secret, &state.outbound)
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
pub(crate) fn with_note(text: String, note: Option<&str>) -> String {
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
pub(crate) async fn guard_webhook_url(url: &str, policy: &OutboundPolicy) -> Result<(), ApiError> {
    orbit_security::check_outbound_url(url, policy)
        .await
        .map_err(|error| {
            ApiError::validation(vec![ValidationIssue::new("webhookUrl", error.to_string())])
        })
}

pub(crate) async fn guard_credential(
    kind: OutputKind,
    value: &str,
    policy: &OutboundPolicy,
) -> Result<(), ApiError> {
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
        _ => guard_webhook_url(value, policy).await,
    }
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
        assert!(guard_credential(
            OutputKind::Telegram,
            "123456:ABC_def-1",
            &OutboundPolicy::default()
        )
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
                guard_credential(OutputKind::Telegram, token, &OutboundPolicy::default())
                    .await
                    .is_err(),
                "{token} must be rejected"
            );
        }
    }
}
