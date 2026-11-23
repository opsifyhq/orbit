//! Control-plane maintenance loops: scheduled triggers and orphaned-execution reaping.
//!
//! These used to run inside the worker, which required database access. They are
//! control-plane responsibilities, so they now run in `orbit-api` and keep workers
//! token-only.

use std::time::Duration;

use chrono::Utc;
use orbit_contracts::{subjects, OrbitEvent};
use orbit_domain::{AgentStatus, ExecutionStatus, LogLevel, TriggerSource};
use orbit_store::Trigger;
use tokio::task::JoinHandle;

use crate::state::AppState;
use crate::supervisor::{supervise, supervise_graceful};

pub fn spawn(state: AppState) -> Vec<JoinHandle<()>> {
    let mut handles = Vec::new();
    // Offline license: re-read the mounted file so airgapped renewals apply without a
    // restart (Secret volume updates propagate within ~60s).
    handles.push(supervise("license", &state, |license_state| async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(60));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut previous = license_phase(&license_state.license_status());
        loop {
            ticker.tick().await;
            let tick = Tick::start(&license_state, "license");
            license_state.license.reload();
            tick.done(true);
            let current = license_phase(&license_state.license_status());
            if current != previous {
                audit_license_transition(&license_state, previous, current).await;
                previous = current;
            }
        }
    }));

    handles.push(supervise("outputs", &state, |output_state| async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(5));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let tick = Tick::start(&output_state, "outputs");
            crate::handlers::outputs::deliver_pending(&output_state).await;
            tick.done(true);
        }
    }));

    handles.push(supervise_graceful(
        "scheduler",
        &state,
        SCHEDULER_SHUTDOWN_GRACE,
        |scheduler_state| async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(15));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                // A shutdown request stops new work, but the tick already started (and any
                // triggers it claimed) is allowed to finish under the supervisor's grace.
                if scheduler_state.shutdown.is_cancelled() {
                    break;
                }
                let tick = Tick::start(&scheduler_state, "scheduler");
                let result = scheduler_tick(&scheduler_state).await;
                tick.done(result.is_ok());
                if let Err(error) = result {
                    tracing::error!(%error, "scheduler tick failed");
                }
            }
        },
    ));

    handles.push(supervise("reaper", &state, |reaper_state| async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(15));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let tick = Tick::start(&reaper_state, "reaper");
            let reaped = reap(&reaper_state).await;
            revoke_finished_launch_tokens(&reaper_state).await;
            tick.done(reaped);
        }
    }));

    handles.push(supervise("team_runs", &state, |team_state| async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(2));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let tick = Tick::start(&team_state, "team_runs");
            let mut ok = true;
            // Page through every running run (keyset on id) so a backlog of long-lived
            // runs cannot starve newer ones.
            let mut after = None;
            loop {
                match team_state.store.list_running_team_runs(after, 50).await {
                    Ok(runs) if runs.is_empty() => break,
                    Ok(runs) => {
                        after = runs.last().map(|run| run.id);
                        for run in runs {
                            match crate::handlers::teams::advance(&team_state, run).await {
                                Ok(_) => team_state.metrics.team_run_advanced(),
                                Err(error) => {
                                    ok = false;
                                    tracing::warn!(%error, "team run advance failed");
                                }
                            }
                        }
                    }
                    Err(error) => {
                        ok = false;
                        tracing::warn!(%error, "team run polling failed");
                        break;
                    }
                }
            }
            if let Err(error) = crate::handlers::teams::reconcile_team_outcomes(&team_state).await {
                ok = false;
                tracing::warn!(%error, "team outcome reconciliation failed");
            }
            tick.done(ok);
        }
    }));

    handles.push(supervise(
        "dispatcher",
        &state,
        |dispatcher_state| async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(15));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                let tick = Tick::start(&dispatcher_state, "dispatcher");
                let result = crate::dispatcher::dispatch_tick(&dispatcher_state).await;
                tick.done(result.is_ok());
                if let Err(error) = result {
                    tracing::error!(%error, "work source dispatcher tick failed");
                }
            }
        },
    ));

    handles.push(supervise("worker_tags", &state, |tag_state| async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(60));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let tick = Tick::start(&tag_state, "worker_tags");
            let result = reconcile_worker_tags(&tag_state).await;
            tick.done(result.is_ok());
            if let Err(error) = result {
                tracing::warn!(%error, "worker tag reconciliation failed");
            }
        }
    }));

    handles.push(supervise("runtime", &state, |runtime_state| async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(5));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let tick = Tick::start(&runtime_state, "runtime");
            let result = runtime_tick(&runtime_state).await;
            tick.done(result.is_ok());
            if let Err(error) = result {
                tracing::warn!(%error, "agent runtime tick failed");
            }
        }
    }));

    handles.push(supervise(
        "retention",
        &state,
        |retention_state| async move {
            // Run shortly after start, then daily.
            let mut ticker = tokio::time::interval(Duration::from_secs(6 * 3600));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                archive_old_runs(&retention_state).await;
            }
        },
    ));

    handles.push(supervise(
        "memory_pruner",
        &state,
        |pruner_state| async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(3600));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                let tick = Tick::start(&pruner_state, "memory_pruner");
                let result = pruner_state.store.prune_expired_memory().await;
                tick.done(result.is_ok());
                match result {
                    Ok(n) if n > 0 => tracing::info!(count = n, "pruned expired memory entries"),
                    Ok(_) => {}
                    Err(error) => tracing::warn!(%error, "memory pruning failed"),
                }
            }
        },
    ));
    handles.extend(spawn_audit_retention(&state));
    handles
}

/// Times one pass of a background loop for `orbit_background_tick_duration_seconds`.
/// A pass that is dropped (shutdown, panic) is not recorded.
struct Tick {
    metrics: std::sync::Arc<crate::metrics::Metrics>,
    name: &'static str,
    started: std::time::Instant,
}

impl Tick {
    fn start(state: &AppState, name: &'static str) -> Self {
        Self {
            metrics: state.metrics.clone(),
            name,
            started: std::time::Instant::now(),
        }
    }

    fn done(self, ok: bool) {
        self.metrics
            .background_tick(self.name, self.started.elapsed(), ok);
    }
}

/// Lifecycle phase derived from the verified status. `expired` is distinct from an
/// unusable license (`invalid`) and from no license at all (`community`).
pub(crate) fn license_phase(status: &orbit_security::LicenseStatus) -> &'static str {
    if !status.valid {
        return match status.error.as_deref() {
            Some(error) if error.contains("expired") => "expired",
            Some(_) => "invalid",
            None => "community",
        };
    }
    if status.grace {
        "grace"
    } else {
        "active"
    }
}

/// Record active → grace → expired (and renew/install) transitions in the audit log.
/// The license itself is installation-wide, so it is logged against the default org with
/// a system actor. No credential or signed token is written.
pub async fn audit_license_transition(state: &AppState, from: &str, to: &str) {
    let license = state.license_status();
    tracing::info!(from, to, "license phase changed");
    if let Err(error) = state
        .store
        .append_audit_log(
            state.default_org_id,
            "system",
            None,
            None,
            "license.phase_changed",
            Some("license"),
            None,
            serde_json::json!({
                "from": from,
                "to": to,
                "licenseId": license.license_id,
            }),
            None,
            None,
        )
        .await
    {
        tracing::error!(%error, from, to, "failed to audit license phase change");
    }
}

/// Parse `ORBIT_AUDIT_RETENTION_DAYS`. Unset, empty, `0` or invalid -> keep audit logs
/// forever (`None`). Deleting audit history is opt-in so an upgrade never prunes it.
fn audit_retention_days(raw: Option<&str>) -> Option<u32> {
    match raw.map(str::trim).filter(|value| !value.is_empty()) {
        None => None,
        Some(value) => match value.parse::<u32>() {
            Ok(0) => None,
            Ok(days) => Some(days),
            Err(_) => {
                tracing::warn!(
                    value,
                    "invalid ORBIT_AUDIT_RETENTION_DAYS; keeping audit logs"
                );
                None
            }
        },
    }
}

/// Delete audit log rows older than `ORBIT_AUDIT_RETENTION_DAYS` (unset or `0` keeps
/// everything). Runs daily under `supervise` (restarted after a panic, stopped on
/// shutdown). The store deletes in bounded batches and holds an advisory lock, so only
/// one API replica prunes at a time.
fn spawn_audit_retention(state: &AppState) -> Option<JoinHandle<()>> {
    let days = audit_retention_days(std::env::var("ORBIT_AUDIT_RETENTION_DAYS").ok().as_deref())?;
    Some(supervise(
        "audit_retention",
        state,
        move |retention_state| async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(24 * 3600));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                let cutoff = Utc::now() - chrono::Duration::days(days as i64);
                match retention_state.store.prune_audit_logs(cutoff).await {
                    Ok(0) => {}
                    Ok(count) => tracing::info!(count, days, "pruned old audit log entries"),
                    Err(error) => tracing::warn!(%error, "audit log pruning failed"),
                }
            }
        },
    ))
}

/// Archive terminal executions, team runs and worker launches older than the configured
/// retention window. History is kept; the default list simply stops showing old runs.
async fn archive_old_runs(state: &AppState) {
    let Some(days) = state.archive_after_days else {
        return;
    };
    let cutoff = Utc::now() - chrono::Duration::days(days as i64);
    match state.store.archive_executions_before(cutoff).await {
        Ok(0) => {}
        Ok(count) => tracing::info!(count, days, "archived old executions"),
        Err(error) => tracing::warn!(%error, "execution archive failed"),
    }
    match state.store.archive_team_runs_before(cutoff).await {
        Ok(0) => {}
        Ok(count) => tracing::info!(count, days, "archived old team runs"),
        Err(error) => tracing::warn!(%error, "team run archive failed"),
    }
    match state.store.archive_worker_launches_before(cutoff).await {
        Ok(0) => {}
        Ok(count) => tracing::info!(count, days, "archived old worker launches"),
        Err(error) => tracing::warn!(%error, "worker launch archive failed"),
    }
}

/// A claimed trigger must be fired before shutdown interrupts the loop. The supervisor
/// allows this long for the in-flight tick; it fits inside the API's shutdown deadline.
const SCHEDULER_SHUTDOWN_GRACE: Duration = Duration::from_secs(2);
/// Triggers claimed per store call (keeps lock/transaction time bounded).
const TRIGGER_CLAIM_SIZE: i64 = 10;
/// Repeat claims per tick so a backlog of more than ten due triggers clears promptly.
/// Each batch advances `next_run_at` before it fires, so nothing is fired twice.
const MAX_TRIGGER_BATCHES_PER_TICK: usize = 10;

async fn scheduler_tick(state: &AppState) -> anyhow::Result<()> {
    match state.store.repair_stuck_schedule_triggers().await {
        Ok(0) => {}
        Ok(count) => tracing::warn!(count, "repaired schedule triggers with no next run"),
        Err(error) => tracing::warn!(%error, "schedule trigger repair failed"),
    }
    for _ in 0..MAX_TRIGGER_BATCHES_PER_TICK {
        let batch = state.store.claim_due_triggers(TRIGGER_CLAIM_SIZE).await?;
        if batch.is_empty() {
            break;
        }
        for trigger in batch {
            if let Err(error) = fire_trigger(state, trigger).await {
                tracing::warn!(%error, "scheduled trigger failed");
            }
        }
        // Finish the claimed batch, then stop before claiming another one on shutdown.
        if state.shutdown.is_cancelled() {
            break;
        }
    }
    Ok(())
}

/// Fire one due trigger and record success/failure on it. Public for integration tests.
pub async fn fire_trigger(state: &AppState, trigger: Trigger) -> anyhow::Result<()> {
    let result = fire_trigger_inner(state, trigger.clone()).await;
    match &result {
        Ok(()) => {
            state.metrics.trigger_fired();
            let _ = state.store.record_trigger_success(trigger.id).await;
        }
        Err(error) => {
            tracing::warn!(%error, trigger_id = %trigger.id, "scheduled trigger failed");
            if let Ok(disabled) = state.store.record_trigger_failure(trigger.id).await {
                if disabled {
                    tracing::warn!(trigger_id = %trigger.id, "trigger auto-disabled after consecutive failures");
                }
            }
        }
    }
    result
}

async fn fire_trigger_inner(state: &AppState, trigger: Trigger) -> anyhow::Result<()> {
    // `claim_due_triggers` already stored the next occurrence atomically with the claim.
    let agent = state
        .store
        .get_agent(trigger.org_id, trigger.agent_id)
        .await?;
    if AgentStatus::parse(&agent.status) != Some(AgentStatus::Active) {
        tracing::debug!(agent_id = %agent.id, "scheduled trigger skipped: agent is not active");
        return Ok(());
    }
    let version = state
        .store
        .get_current_version(trigger.org_id, &agent)
        .await?;
    let spec: orbit_domain::AgentSpec = serde_json::from_value(version.spec.clone())?;

    let active = state
        .store
        .count_active_executions_for_agent(trigger.org_id, agent.id)
        .await?;
    if active >= spec.resources.max_concurrent_executions as i64 {
        tracing::info!(agent_id = %agent.id, "scheduled trigger skipped: concurrency limit");
        return Ok(());
    }

    let fired_at = trigger.last_fired_at.unwrap_or_else(Utc::now);
    // A schedule may carry its own task; otherwise the agent gets a generic prompt.
    let task = trigger
        .config
        .get("task")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|task| !task.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            format!(
                "Scheduled run for agent '{}'. Perform your configured monitoring task.",
                agent.name
            )
        });
    let input = serde_json::json!({
        "task": task,
        "payload": { "triggerId": trigger.id, "firedAt": fired_at },
        "provenance": {
            "source": "schedule",
            "trusted": false,
            "subject": trigger.id,
            "receivedAt": fired_at,
        },
        "agentVersion": version.version,
    });

    let harness_kind = match trigger
        .harness_kind
        .as_deref()
        .and_then(|value| value.parse::<orbit_domain::HarnessKind>().ok())
        .or_else(|| spec.harness.as_ref().map(|harness| harness.kind))
    {
        Some(kind) => kind,
        None => match state.default_harness().await {
            Some(kind) => kind,
            None => {
                tracing::warn!(trigger_id = %trigger.id, "scheduled trigger skipped: no harness configured");
                return Ok(());
            }
        },
    };
    let placement = spec
        .harness
        .as_ref()
        .map(|harness| harness.placement.as_str())
        .unwrap_or("auto");
    // Shared execution preflight (#140), evaluated at fire time for the trigger owner (or,
    // for spec-derived triggers without an owner, the agent owner). If that user lost
    // access to the agent, a connection or the provider, the run fails with a recorded
    // error and counts towards the trigger's consecutive-failure auto-disable. Harness
    // unavailability and the hourly quota skip this tick instead of failing the trigger.
    let identity_user = trigger.owner_user_id.or(agent.owner_user_id);
    let identity = crate::preflight::background_access(
        state,
        trigger.org_id,
        &[trigger.owner_user_id, agent.owner_user_id],
    )
    .await;
    let access = match identity {
        Ok(access) => access,
        Err(error) => {
            crate::preflight::audit_denied_run(
                state,
                trigger.org_id,
                "trigger.fire_denied",
                "trigger",
                trigger.id,
                &error,
                identity_user,
            )
            .await;
            anyhow::bail!("scheduled run refused: {}", error.message);
        }
    };
    if let Some(access) = &access {
        let preflight = crate::preflight::Preflight {
            state,
            org_id: trigger.org_id,
            access,
            agent: &agent,
            spec: &spec,
            harness: harness_kind,
            extra_connection_ids: &[],
        };
        if let Err(error) = preflight.authorize_access().await {
            crate::preflight::audit_denied_run(
                state,
                trigger.org_id,
                "trigger.fire_denied",
                "trigger",
                trigger.id,
                &error,
                identity_user,
            )
            .await;
            anyhow::bail!("scheduled run refused: {}", error.message);
        }
    }
    if let Err(error) =
        crate::preflight::require_harness_available(state, trigger.org_id, harness_kind).await
    {
        tracing::warn!(trigger_id = %trigger.id, error = %error.message, "scheduled trigger skipped: harness unavailable");
        return Ok(());
    }
    if let Err(error) =
        crate::preflight::require_hourly_quota(state, trigger.org_id, agent.id, &spec).await
    {
        tracing::info!(trigger_id = %trigger.id, error = %error.message, "scheduled trigger skipped: hourly quota reached");
        return Ok(());
    }
    let idempotency_key = format!("trigger:{}:{}", trigger.id, fired_at.timestamp_millis());
    let execution = state
        .store
        .create_execution(orbit_store::executions::NewExecution {
            org_id: trigger.org_id,
            agent_id: agent.id,
            agent_version_id: version.id,
            agent_name: &agent.name,
            harness_kind,
            trigger_source: TriggerSource::Schedule,
            placement,
            trigger_id: Some(trigger.id),
            input,
            session_ref: None,
            skills: &[],
            timeout_seconds: spec.resources.timeout_seconds,
            requested_by: None,
            idempotency_key: Some(idempotency_key.as_str()),
            team_run_id: None,
            team_step_index: None,
        })
        .await?;

    let event = OrbitEvent::new(
        subjects::EXECUTION_REQUESTED,
        "orbit-api-scheduler",
        serde_json::json!({
            "executionId": execution.id,
            "agentId": agent.id,
            "triggerId": trigger.id,
            "harness": harness_kind.as_str(),
        }),
    )
    .with_org(trigger.org_id)
    .with_idempotency(format!("exec:{}", execution.id));
    if let Err(error) = state.bus.publish(event).await {
        tracing::warn!(%error, "scheduled execution publish failed; polling will pick it up");
    }
    tracing::info!(
        execution_id = %execution.id,
        agent_id = %agent.id,
        status = ExecutionStatus::Queued.as_str(),
        "scheduled execution queued"
    );
    Ok(())
}

/// A worker whose heartbeat stops for this long is considered gone.
pub(crate) const HEARTBEAT_STALE_SECONDS: i64 = 90;
/// A claimed run with no activity for this long was never started by its worker. Long
/// enough for a large repository checkout, which reports only when it finishes.
const UNSTARTED_SILENT_SECONDS: i64 = 600;
/// How long a claimed, never-started run may ignore a cancel request.
const UNSTARTED_CANCEL_SECONDS: i64 = 60;
/// Rows pruned per bounded delete; the reaper repeats until the batch is short or the
/// per-tick cap is reached so a large backlog cannot hold a long transaction open.
const OUTPUT_DELIVERY_PRUNE_BATCH: i64 = 1000;
const MAX_OUTPUT_DELIVERY_PRUNE_BATCHES: usize = 10;
/// Default slack added to `timeout_seconds` before a still-`running` run is ended by the
/// platform. Override with `ORBIT_EXECUTION_TIMEOUT_GRACE_SECONDS`.
const DEFAULT_TIMEOUT_GRACE_SECONDS: i64 = 300;

fn timeout_grace_seconds() -> i64 {
    std::env::var("ORBIT_EXECUTION_TIMEOUT_GRACE_SECONDS")
        .ok()
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| *value >= 0)
        .unwrap_or(DEFAULT_TIMEOUT_GRACE_SECONDS)
}

/// A single-execution launch's worker token has no use once its execution is terminal;
/// revoke it so a leaked token cannot be replayed.
async fn revoke_finished_launch_tokens(state: &AppState) {
    match state.store.revoke_finished_single_execution_tokens().await {
        Ok(ids) if !ids.is_empty() => {
            tracing::info!(
                count = ids.len(),
                "revoked worker tokens of finished single-execution launches"
            );
        }
        Ok(_) => {}
        Err(error) => tracing::warn!(%error, "single-execution token revocation failed"),
    }
}

/// Returns `false` when any of the reaper's steps failed (each is logged).
async fn reap(state: &AppState) -> bool {
    let mut ok = true;
    // Workers killed by the cluster (SIGKILL, eviction, node loss) cannot call `offline`.
    // Keep their row long enough to explain the gap, then prune it so the UI does not
    // accumulate ghost workers forever.
    let heartbeat_retention_hours = std::env::var("ORBIT_WORKER_STALE_RETENTION_HOURS")
        .ok()
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| *value >= 1)
        .unwrap_or(24);
    match state
        .store
        .delete_stale_worker_heartbeats(heartbeat_retention_hours)
        .await
    {
        Ok(count) if count > 0 => {
            tracing::info!(count, "pruned stale worker heartbeats");
        }
        Ok(_) => {}
        Err(error) => {
            ok = false;
            tracing::warn!(%error, "pruning stale worker heartbeats failed");
        }
    }

    if let Some(days) = state.output_delivery_retention_days {
        let cutoff = Utc::now() - chrono::Duration::days(days as i64);
        let mut pruned = 0u64;
        for _ in 0..MAX_OUTPUT_DELIVERY_PRUNE_BATCHES {
            match state
                .store
                .prune_output_deliveries_before(cutoff, OUTPUT_DELIVERY_PRUNE_BATCH)
                .await
            {
                Ok(count) => {
                    pruned += count;
                    if count < OUTPUT_DELIVERY_PRUNE_BATCH as u64 {
                        break;
                    }
                }
                Err(error) => {
                    ok = false;
                    tracing::warn!(%error, "output delivery pruning failed");
                    break;
                }
            }
        }
        if pruned > 0 {
            tracing::info!(count = pruned, days, "pruned old output deliveries");
        }
    }

    match state
        .store
        .reap_orphaned_executions(HEARTBEAT_STALE_SECONDS)
        .await
    {
        Ok(rows) => {
            for execution in rows {
                tracing::warn!(
                    execution_id = %execution.id,
                    worker_id = ?execution.worker_id,
                    "reaped orphaned execution"
                );
                report_reaped(
                    state,
                    &execution,
                    "worker heartbeat expired; execution marked failed",
                    &orbit_store::workers::worker_lost_error(),
                )
                .await;
            }
        }
        Err(error) => {
            ok = false;
            tracing::warn!(%error, "execution reaper failed");
        }
    }

    match state
        .store
        .reap_unstarted_executions(None, UNSTARTED_SILENT_SECONDS, UNSTARTED_CANCEL_SECONDS)
        .await
    {
        Ok(rows) => {
            for execution in rows {
                let cancelled = execution.status == ExecutionStatus::Cancelled.as_str();
                tracing::warn!(
                    execution_id = %execution.id,
                    worker_id = ?execution.worker_id,
                    status = %execution.status,
                    "ended execution its worker never started"
                );
                let message = if cancelled {
                    "cancelled; the worker that took this execution never started it"
                } else {
                    "a worker took this execution but never started it; execution marked failed"
                };
                let data = execution.error.clone().unwrap_or_else(
                    || serde_json::json!({ "code": "worker_did_not_start", "retryable": true }),
                );
                report_reaped(state, &execution, message, &data).await;
            }
        }
        Err(error) => {
            ok = false;
            tracing::warn!(%error, "unstarted execution reaper failed");
        }
    }

    match state
        .store
        .reap_timed_out_executions(None, timeout_grace_seconds())
        .await
    {
        Ok(rows) => {
            for execution in rows {
                tracing::warn!(
                    execution_id = %execution.id,
                    worker_id = ?execution.worker_id,
                    "ended execution that outlived its timeout"
                );
                let message = format!(
                    "execution outlived its {}s timeout (plus grace) without being reported finished; marked timed out",
                    execution.timeout_seconds
                );
                let data = execution.error.clone().unwrap_or_else(
                    || serde_json::json!({ "code": "execution_timeout", "retryable": true }),
                );
                report_reaped(state, &execution, &message, &data).await;
            }
        }
        Err(error) => {
            ok = false;
            tracing::warn!(%error, "timed-out execution reaper failed");
        }
    }
    ok
}

/// Record why the platform ended an execution and tell subscribers.
async fn report_reaped(
    state: &AppState,
    execution: &orbit_store::ExecutionRecord,
    message: &str,
    data: &serde_json::Value,
) {
    if let Err(err) = state
        .store
        .append_execution_event(
            execution.org_id,
            execution.id,
            "error",
            LogLevel::Error,
            message,
            data,
        )
        .await
    {
        tracing::warn!(execution_id = %execution.id, error = %err, "failed to append execution event");
    }
    let event = OrbitEvent::new(
        subjects::EXECUTION_UPDATED,
        "orbit-api-reaper",
        serde_json::json!({
            "executionId": execution.id,
            "agentId": execution.agent_id,
            "status": execution.status,
        }),
    )
    .with_org(execution.org_id)
    .with_idempotency(format!("reap:{}", execution.id));
    let _ = state.bus.publish(event).await;
}

/// Relaunch platform-follow worker profiles when ORBIT_IMAGE_TAG changes. The old token
/// is revoked first (no new claims), active executions drain, then the launcher stops and
/// replaces the worker.
async fn reconcile_worker_tags(state: &AppState) -> anyhow::Result<()> {
    if state.launcher_url.is_none() {
        return Ok(());
    }
    let tag = crate::images::image_tag();
    let profiles = state.store.list_platform_follow_profiles().await?;
    for profile in profiles {
        let Some(user_id) = profile.owner_user_id else {
            continue;
        };
        let desired = format!("{}:{}", profile.image, tag);
        let launches = state
            .store
            .active_launches_for_profile(profile.org_id, profile.id)
            .await?;
        for launch in launches
            .into_iter()
            .filter(|launch| launch.image != desired)
        {
            let auth = orbit_security::AuthContext {
                user_id,
                org_id: profile.org_id,
                email: "system@orbit.local".into(),
                display_name: "ORBIT auto-update".into(),
                role: orbit_domain::UserRole::Owner,
                method: orbit_security::AuthMethod::ApiToken,
                scopes: Vec::new(),
                token_id: None,
            };
            if let Some(token_id) = launch.worker_token_id {
                let _ = state
                    .store
                    .revoke_worker_token(profile.org_id, token_id)
                    .await;
            }
            if let Some(worker_id) = &launch.worker_id {
                if state
                    .store
                    .has_active_executions_for_worker(profile.org_id, worker_id)
                    .await?
                {
                    tracing::info!(profile = %profile.name, %worker_id, "worker update waiting for drain");
                    continue;
                }
            }
            let old_image = launch.image.clone();
            match crate::handlers::worker_launches::stop_launch_internal(state, &auth, launch).await
            {
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(profile = %profile.name, %error, "could not stop old worker");
                    continue;
                }
            }
            match crate::handlers::worker_launches::launch_profile_internal(
                state, &auth, profile.id, None,
            )
            .await
            {
                Ok(new_launch) => {
                    tracing::info!(
                        profile = %profile.name,
                        old = %old_image,
                        new = %new_launch.image,
                        "worker auto-relaunched for platform tag"
                    );
                    crate::audit::record(
                        state,
                        &auth,
                        "worker.auto_relaunched",
                        Some("worker_profile"),
                        Some(profile.id),
                        serde_json::json!({
                            "oldImage": old_image,
                            "newImage": new_launch.image,
                            "launchId": new_launch.id,
                        }),
                    )
                    .await;
                }
                Err(error) => {
                    tracing::error!(profile = %profile.name, %error, "worker auto-relaunch failed");
                }
            }
        }
    }
    Ok(())
}

enum RuntimeTickError {
    /// Retrying cannot help; the runtime is stopped with a clear `last_error`.
    Permanent(String),
    /// Possibly recoverable (store hiccup, agent edited later); retried with backoff.
    Transient(anyhow::Error),
}

impl From<orbit_store::StoreError> for RuntimeTickError {
    fn from(error: orbit_store::StoreError) -> Self {
        Self::Transient(error.into())
    }
}

const RUNTIME_MIN_BACKOFF_SECONDS: i64 = 30;
const RUNTIME_MAX_BACKOFF_SECONDS: i64 = 900;
/// A runtime stops after this many failed executions in a row.
#[doc(hidden)]
pub const RUNTIME_MAX_CONSECUTIVE_FAILURES: i32 = 5;

/// Exponential backoff from the runtime interval (at least 30s), doubling per
/// consecutive failure and capped at 15 minutes.
fn runtime_failure_backoff(interval_seconds: i32, failures: i32) -> chrono::Duration {
    let base = (interval_seconds as i64).max(RUNTIME_MIN_BACKOFF_SECONDS);
    let doublings = (failures.max(1) - 1).min(16) as u32;
    chrono::Duration::seconds(
        base.saturating_mul(1i64 << doublings)
            .min(RUNTIME_MAX_BACKOFF_SECONDS),
    )
}

fn runtime_tick_key(runtime_id: uuid::Uuid, last_execution_id: Option<uuid::Uuid>) -> String {
    match last_execution_id {
        Some(id) => format!("runtime:{runtime_id}:after:{id}"),
        None => format!("runtime:{runtime_id}:first"),
    }
}

/// Advance continuous agent runtimes: create the next execution when the previous one
/// is terminal, and stop runtimes whose agent is no longer active. Every runtime is
/// handled independently: a failure is recorded on that runtime (with backoff, or a stop
/// for permanent errors) and never prevents the others from advancing.
#[doc(hidden)]
pub async fn runtime_tick(state: &AppState) -> anyhow::Result<()> {
    for runtime in state.store.list_running_agent_runtimes().await? {
        let (runtime_id, org_id) = (runtime.id, runtime.org_id);
        let interval = runtime.interval_seconds;
        let Err(failure) = advance_runtime(state, runtime).await else {
            continue;
        };
        // Never moves `stopping`/`stopped` runtimes: both writes are conditional on
        // `status = 'running'`.
        let result = match &failure {
            RuntimeTickError::Permanent(message) => {
                tracing::warn!(%runtime_id, %org_id, error = %message, "agent runtime stopped after permanent error");
                state
                    .store
                    .halt_running_agent_runtime(runtime_id, message)
                    .await
            }
            RuntimeTickError::Transient(error) => {
                let message = error.to_string();
                tracing::warn!(%runtime_id, %org_id, error = %message, "agent runtime tick failed");
                state
                    .store
                    .advance_agent_runtime(
                        runtime_id,
                        Some(&message),
                        chrono::Utc::now() + runtime_failure_backoff(interval.max(60), 1),
                    )
                    .await
            }
        };
        if let Err(error) = result {
            tracing::error!(%runtime_id, %org_id, %error, "could not record runtime failure");
        }
    }
    Ok(())
}

async fn advance_runtime(
    state: &AppState,
    runtime: orbit_store::agent_runtimes::AgentRuntime,
) -> Result<(), RuntimeTickError> {
    let _ = state.store.heartbeat_agent_runtime(runtime.id).await;
    let agent = match state
        .store
        .get_agent(runtime.org_id, runtime.agent_id)
        .await
    {
        Ok(agent) => agent,
        Err(_) => {
            let _ = state
                .store
                .halt_agent_runtime(runtime.id, "agent no longer exists")
                .await;
            return Ok(());
        }
    };
    if agent.status != "active" {
        let _ = state
            .store
            .halt_agent_runtime(runtime.id, "agent is not active")
            .await;
        return Ok(());
    }

    let last = match runtime.last_execution_id {
        Some(id) => state.store.get_execution(runtime.org_id, id).await.ok(),
        None => None,
    };
    let last_terminal = last.as_ref().is_none_or(|execution| {
        matches!(
            ExecutionStatus::parse(&execution.status),
            Some(status) if status.is_terminal()
        )
    });
    if runtime.status == "stopping" {
        if last_terminal {
            let _ = state.store.finish_agent_runtime_stop(runtime.id).await;
        } else if let Some(id) = runtime.last_execution_id {
            // An execution launched by a tick that raced the Stop is not covered by the
            // handler's cancel; cancel it here so Stop does not wait for a full run.
            let _ = state
                .store
                .request_execution_cancel(runtime.org_id, id)
                .await;
        }
        return Ok(());
    }
    if !last_terminal {
        return Ok(());
    }
    let Some(runtime) = state
        .store
        .claim_agent_runtime_tick(
            runtime.id,
            chrono::Utc::now() + chrono::Duration::seconds(5),
        )
        .await
        .map_err(|error| RuntimeTickError::Transient(error.into()))?
    else {
        return Ok(());
    };
    let last_failed = last
        .as_ref()
        // A run the reaper ended as `timed_out` is as unsuccessful as one that failed; if
        // it did not count, a runtime whose runs always time out would loop forever.
        .filter(|execution| matches!(execution.status.as_str(), "failed" | "timed_out"));
    if let Some(failed) = last_failed {
        // First time this failure is seen: count it and back off. Once accounted for, the
        // next due tick falls through and creates a new execution.
        if runtime.failed_execution_id != Some(failed.id) {
            let failures = runtime.consecutive_failures.saturating_add(1);
            let reason = failed
                .error
                .as_ref()
                .map(|error| error.to_string())
                .unwrap_or_else(|| "execution failed".into());
            let (stop, message, next_run_at) = if failures >= RUNTIME_MAX_CONSECUTIVE_FAILURES {
                (
                    true,
                    format!("stopped after {failures} consecutive failed executions: {reason}"),
                    chrono::Utc::now(),
                )
            } else {
                (
                    false,
                    reason,
                    chrono::Utc::now()
                        + runtime_failure_backoff(runtime.interval_seconds, failures),
                )
            };
            state
                .store
                .record_agent_runtime_failure(
                    runtime.id,
                    stop,
                    failed.id,
                    failures,
                    &message,
                    next_run_at,
                )
                .await?;
            return Ok(());
        }
    }

    // An unknown harness is a property of the runtime row itself and can never heal:
    // stop the runtime instead of retrying forever.
    let harness_kind = runtime
        .harness_kind
        .parse::<orbit_domain::HarnessKind>()
        .map_err(|_| {
            RuntimeTickError::Permanent(format!(
                "runtime has an unknown harness '{}'",
                runtime.harness_kind
            ))
        })?;
    // A missing version or a spec that no longer parses can be fixed by editing the
    // agent, so these back off and recover on their own.
    let version = state
        .store
        .get_current_version(runtime.org_id, &agent)
        .await
        .map_err(|error| RuntimeTickError::Transient(error.into()))?;
    let spec: orbit_domain::AgentSpec =
        serde_json::from_value(version.spec.clone()).map_err(|error| {
            RuntimeTickError::Transient(anyhow::anyhow!("agent spec is invalid: {error}"))
        })?;
    // Shared execution preflight (#140) for the runtime creator (falling back to the
    // agent owner). Lost access stops the runtime with a recorded error; an unavailable
    // harness or exhausted hourly quota delays the next tick.
    let access = crate::preflight::background_access(
        state,
        runtime.org_id,
        &[runtime.created_by, agent.owner_user_id],
    )
    .await;
    let denied = match &access {
        Err(error) => Some(error.clone()),
        Ok(Some(access)) => crate::preflight::Preflight {
            state,
            org_id: runtime.org_id,
            access,
            agent: &agent,
            spec: &spec,
            harness: harness_kind,
            extra_connection_ids: &[],
        }
        .authorize_access()
        .await
        .err(),
        Ok(None) => None,
    };
    if let Some(error) = denied {
        crate::preflight::audit_denied_run(
            state,
            runtime.org_id,
            "runtime.run_denied",
            "agent",
            agent.id,
            &error,
            runtime.created_by.or(agent.owner_user_id),
        )
        .await;
        let _ = state
            .store
            .halt_agent_runtime(runtime.id, &format!("run refused: {}", error.message))
            .await;
        return Ok(());
    }
    let delayed = match crate::preflight::require_harness_available(
        state,
        runtime.org_id,
        harness_kind,
    )
    .await
    {
        Ok(()) => {
            crate::preflight::require_hourly_quota(state, runtime.org_id, agent.id, &spec).await
        }
        Err(error) => Err(error),
    };
    if let Err(error) = delayed {
        let _ = state
            .store
            .advance_agent_runtime(
                runtime.id,
                Some(&format!("run delayed: {}", error.message)),
                chrono::Utc::now()
                    + chrono::Duration::seconds(runtime.interval_seconds.max(30) as i64),
            )
            .await;
        return Ok(());
    }
    let input = serde_json::json!({
        "task": runtime.task,
        "payload": { "runtimeId": runtime.id },
        "provenance": {
            "source": "runtime",
            "trusted": true,
            "subject": runtime.id,
            "receivedAt": Utc::now(),
        },
    });
    // Deterministic per tick: the previous execution id only changes once the runtime row
    // is updated below, so a retry after a partial failure reuses the same execution.
    let idempotency_key = runtime_tick_key(runtime.id, runtime.last_execution_id);
    let execution = state
        .store
        .create_execution(orbit_store::executions::NewExecution {
            org_id: runtime.org_id,
            agent_id: agent.id,
            agent_version_id: version.id,
            agent_name: &agent.name,
            harness_kind,
            trigger_source: TriggerSource::Manual,
            placement: spec
                .harness
                .as_ref()
                .map(|harness| harness.placement.as_str())
                .unwrap_or("auto"),
            trigger_id: None,
            input,
            session_ref: last
                .as_ref()
                .and_then(|execution| execution.session_ref.as_deref()),
            skills: &[],
            timeout_seconds: spec.resources.timeout_seconds,
            requested_by: runtime.created_by,
            idempotency_key: Some(&idempotency_key),
            team_run_id: None,
            team_step_index: None,
        })
        .await
        .map_err(|error| RuntimeTickError::Transient(error.into()))?;
    // Does not touch `status`. If the runtime was stopped while this tick was in flight
    // the write is refused: cancel the execution so nothing keeps running for it.
    let recorded = state
        .store
        .record_agent_runtime_launch(
            runtime.id,
            execution.id,
            last_failed.is_none(),
            chrono::Utc::now() + chrono::Duration::seconds(runtime.interval_seconds as i64),
        )
        .await?;
    if recorded.is_none() {
        let _ = state
            .store
            .request_execution_cancel(runtime.org_id, execution.id)
            .await;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{audit_retention_days, license_phase};
    use orbit_security::LicenseStatus;

    #[test]
    fn audit_retention_parsing() {
        assert_eq!(audit_retention_days(None), None);
        assert_eq!(audit_retention_days(Some("")), None);
        assert_eq!(audit_retention_days(Some("90")), Some(90));
        assert_eq!(audit_retention_days(Some("0")), None);
        assert_eq!(audit_retention_days(Some("abc")), None);
    }

    #[test]
    fn license_phases_distinguish_grace_expiry_and_invalid() {
        let status = LicenseStatus::community();
        assert_eq!(license_phase(&status), "community");

        let mut status = LicenseStatus::community();
        status.valid = true;
        assert_eq!(license_phase(&status), "active");
        status.grace = true;
        assert_eq!(license_phase(&status), "grace");
        status.valid = false;
        status.error = Some("license expired".into());
        assert_eq!(license_phase(&status), "expired");
        status.error = Some("license signature is invalid".into());
        assert_eq!(license_phase(&status), "invalid");
    }
}
