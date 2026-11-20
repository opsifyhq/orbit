use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use orbit_contracts::{
    ExecutionOutcome, ExecutionRequest, HarnessError, HarnessErrorKind, HarnessEvent,
    HarnessEventKind, InstructionsBlock, SessionRequest,
};
use orbit_domain::{
    AutonomyLevel, ExecutionEventKind, ExecutionStatus, HarnessKind, LogLevel, ResourceLimits,
};
use orbit_store::ExecutionRecord;
use tokio::sync::{mpsc, Semaphore};
use uuid::Uuid;

/// Shared worker state.
#[derive(Clone)]
pub struct WorkerState {
    pub worker_id: String,
    pub client: crate::client::WorkerClient,
    pub registry: Arc<orbit_harness::HarnessRegistry>,
    pub workspace_root: std::path::PathBuf,
    /// Harness kinds this worker is allowed to claim.
    pub harnesses: Vec<String>,
    /// `process` or `docker` (per-execution container).
    pub execution_driver: String,
    /// Worker target class (`local`, `docker`, `kubernetes`).
    pub target: String,
    /// Agents this worker is scoped to (empty = all).
    pub profile_agent_ids: Vec<Uuid>,
    /// Skills materialized for every execution handled by this worker.
    pub profile_skill_slugs: Vec<String>,
    pub allow_web_search: bool,
    pub allow_web_fetch: bool,
    pub native_tool_permissions: serde_json::Value,
    /// Environment variables resolved from worker-profile connection mappings. Applied
    /// to each harness process instead of mutating the worker's global environment.
    pub extra_env: Arc<std::collections::BTreeMap<String, String>>,
    /// Executions this worker currently owns (claimed and not yet reported terminal),
    /// each with its cancellation token. Entries are removed by [`ActiveGuard`].
    pub active: ActiveMap,
    /// Cancelled when the worker is shutting down. Every execution token is a child of
    /// it, so cancelling it interrupts all in-flight runs.
    pub shutdown: tokio_util::sync::CancellationToken,
    /// Backoff for the calls that move an execution's lifecycle.
    pub retry: crate::retry::RetryPolicy,
}

pub type ActiveMap =
    Arc<std::sync::Mutex<std::collections::BTreeMap<Uuid, tokio_util::sync::CancellationToken>>>;

/// Removes an execution from [`WorkerState::active`] when dropped, on every exit path
/// including error returns and panics.
pub struct ActiveGuard {
    active: ActiveMap,
    id: Uuid,
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.id);
    }
}

/// Register a claimed execution as active. Do this before spawning its task so a
/// shutdown that begins right after the claim still sees (and reports) it.
pub fn track(state: &WorkerState, id: Uuid) -> (ActiveGuard, tokio_util::sync::CancellationToken) {
    let cancel = state.shutdown.child_token();
    state
        .active
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(id, cancel.clone());
    (
        ActiveGuard {
            active: state.active.clone(),
            id,
        },
        cancel,
    )
}

fn active_ids(state: &WorkerState) -> Vec<Uuid> {
    state
        .active
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .keys()
        .copied()
        .collect()
}

/// Poll the durable queue and dispatch executions, respecting a concurrency limit.
///
/// Once shutdown starts no new claim is issued, but a claim request already in flight is
/// never dropped: it completes and its execution is reported as failed ("worker
/// shutdown") instead of waiting for the unstarted-execution reaper.
pub async fn claim_and_dispatch(
    state: &WorkerState,
    semaphore: &Arc<Semaphore>,
    poll_interval: Duration,
) -> anyhow::Result<()> {
    let mut dispatched = 0usize;
    loop {
        if state.shutdown.is_cancelled() {
            return Ok(());
        }
        let permit = match semaphore.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => break,
        };
        let claimed = match state
            .client
            .claim(&crate::client::ClaimRequest {
                worker_id: &state.worker_id,
                harnesses: &state.harnesses,
                target: &state.target,
                agent_ids: &state.profile_agent_ids,
            })
            .await
        {
            Ok(Some(execution)) => execution,
            Ok(None) => break,
            Err(err) => {
                drop(permit);
                return Err(err);
            }
        };
        dispatched += 1;
        let state = state.clone();
        let execution_id = claimed.execution.id;
        let (guard, cancel) = track(&state, execution_id);
        tokio::spawn(async move {
            let _permit = permit;
            let driver = state.execution_driver.clone();
            let task_state = state.clone();
            let result = supervise(&state, execution_id, guard, async move {
                if cancel.is_cancelled() {
                    // Claimed just as shutdown began: do not start it.
                    return report_unfinished(
                        &task_state,
                        execution_id,
                        SHUTDOWN_CODE,
                        SHUTDOWN_MESSAGE,
                    )
                    .await;
                }
                if driver == "docker" {
                    crate::dispatch::run(&task_state, &claimed, cancel).await
                } else {
                    run_execution(task_state, claimed, cancel).await
                }
            })
            .await;
            if let Err(err) = result {
                tracing::error!(error = %err, "execution runner failed");
            }
        });
    }

    if dispatched == 0 {
        tokio::select! {
            _ = tokio::time::sleep(poll_interval) => {}
            _ = state.shutdown.cancelled() => {}
        }
    }
    Ok(())
}

const SHUTDOWN_CODE: &str = "worker_shutdown";
const SHUTDOWN_MESSAGE: &str = "worker shutdown: the worker stopped before this execution finished";

/// Run an execution future so that it always reaches a terminal state.
///
/// The future runs in its own task: an `Err` or a panic is reported as `failed` with a
/// clear error, and `guard` (the `active` entry) is released when this returns. Returns
/// `Err` when the runner failed, after reporting it.
pub async fn supervise<F>(
    state: &WorkerState,
    execution_id: Uuid,
    guard: ActiveGuard,
    fut: F,
) -> anyhow::Result<()>
where
    F: std::future::Future<Output = anyhow::Result<()>> + Send + 'static,
{
    let _guard = guard;
    let failure = match tokio::spawn(fut).await {
        Ok(Ok(())) => return Ok(()),
        Ok(Err(error)) => format!("execution runner failed: {error:#}"),
        Err(join) if join.is_panic() => {
            let payload = join.into_panic();
            let detail = payload
                .downcast_ref::<&str>()
                .map(|text| (*text).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".into());
            format!("execution runner panicked: {detail}")
        }
        Err(join) => format!("execution runner task was aborted: {join}"),
    };
    // A no-op when the runner already reported a terminal state before failing.
    if let Err(error) =
        report_unfinished(state, execution_id, "worker_runner_failed", &failure).await
    {
        tracing::error!(%execution_id, %error, "could not report failed execution; control-plane reaper will end it");
    }
    Err(anyhow::anyhow!(failure))
}

/// Report an execution as `failed` unless it already reached a terminal state. Only
/// records an error event when this call actually ended it.
async fn report_unfinished(
    state: &WorkerState,
    execution_id: Uuid,
    code: &str,
    message: &str,
) -> anyhow::Result<()> {
    let error = serde_json::json!({ "code": code, "message": message, "retryable": true });
    let updated = complete_with_retry(
        state,
        execution_id,
        &crate::client::CompleteRequest {
            status: ExecutionStatus::Failed.as_str(),
            output: None,
            error: Some(error.clone()),
            usage: None,
            session_ref: None,
        },
    )
    .await?;
    if updated {
        let event = crate::client::EventRequest {
            kind: ExecutionEventKind::Error.as_str(),
            level: LogLevel::Error.as_str(),
            message,
            data: serde_json::json!({ "error": error }),
        };
        if let Err(error) = state.client.event(execution_id, &event).await {
            tracing::warn!(%execution_id, %error, "failed to persist failure event");
        }
    }
    Ok(())
}

async fn complete_with_retry(
    state: &WorkerState,
    execution_id: Uuid,
    request: &crate::client::CompleteRequest<'_>,
) -> anyhow::Result<bool> {
    let policy = state.retry;
    policy
        .run("complete", || state.client.complete(execution_id, request))
        .await
}

/// Graceful shutdown: interrupt in-flight executions, wait up to `grace` for them to
/// report, then report anything still active as failed ("worker shutdown").
pub async fn shutdown(state: &WorkerState, grace: Duration) {
    state.shutdown.cancel();
    let deadline = tokio::time::Instant::now() + grace;
    while !active_ids(state).is_empty() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let remaining = active_ids(state);
    if !remaining.is_empty() {
        tracing::warn!(
            count = remaining.len(),
            "executions still active after shutdown grace; reporting them failed"
        );
    }
    for execution_id in remaining {
        if let Err(error) =
            report_unfinished(state, execution_id, SHUTDOWN_CODE, SHUTDOWN_MESSAGE).await
        {
            tracing::error!(%execution_id, %error, "could not report execution at shutdown; control-plane reaper will end it");
        }
    }
}

/// Single-execution container mode: bind one dispatched execution to this worker and run
/// it, without entering the claim loop or scheduler.
pub async fn run_single(state: WorkerState, execution_id: uuid::Uuid) -> anyhow::Result<()> {
    let context = state.client.context(execution_id).await?;
    let (guard, cancel) = track(&state, execution_id);
    let task_state = state.clone();
    supervise(&state, execution_id, guard, async move {
        run_execution(task_state, context, cancel).await
    })
    .await
}

/// Execute one claimed execution and persist its lifecycle.
fn worker_model_selector(model: &crate::client::WorkerModel) -> Option<String> {
    use orbit_contracts::ModelSelection;
    match &model.selection {
        ModelSelection::Harness { provider, model } => match (provider, model) {
            (Some(provider), Some(model)) if !model.contains('/') => {
                Some(format!("{provider}/{model}"))
            }
            (_, Some(model)) => Some(model.clone()),
            _ => None,
        },
        ModelSelection::Platform {
            provider_name,
            model,
            ..
        } => Some(format!("{provider_name}/{model}")),
    }
}

pub async fn run_execution(
    state: WorkerState,
    context: crate::client::WorkerExecutionContext,
    cancel: tokio_util::sync::CancellationToken,
) -> anyhow::Result<()> {
    let execution_id = context.execution.id;
    let workspace = state.workspace_root.join(execution_id.to_string());
    let result = run_execution_inner(state, context, workspace.clone(), cancel).await;
    finish_execution_workspace(&workspace, execution_id, result).await
}

async fn finish_execution_workspace(
    workspace: &std::path::Path,
    execution_id: Uuid,
    result: anyhow::Result<()>,
) -> anyhow::Result<()> {
    if let Err(error) = cleanup_execution_workspace(workspace).await {
        tracing::error!(%error, %execution_id, workspace = %workspace.display(), "could not clean execution workspace");
    }
    result
}

async fn cleanup_execution_workspace(path: &std::path::Path) -> std::io::Result<()> {
    match tokio::fs::remove_dir_all(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

async fn run_execution_inner(
    state: WorkerState,
    context: crate::client::WorkerExecutionContext,
    workspace: std::path::PathBuf,
    cancel: tokio_util::sync::CancellationToken,
) -> anyhow::Result<()> {
    let execution = context.execution.clone();
    let execution_id = execution.id;
    let mut spec = context.spec;
    // #194: the override can only switch approval mode to Auto for agents already at L3+.
    // The legacy numeric `autonomyOverride` is ignored: it never changes the level.
    orbit_domain::apply_approval_override(
        &mut spec.autonomy,
        execution
            .input
            .get(orbit_domain::APPROVAL_OVERRIDE_KEY)
            .and_then(|value| value.as_str()),
    );
    let harness_kind = execution
        .harness_kind
        .parse::<orbit_domain::HarnessKind>()
        .unwrap_or(HarnessKind::Mock);

    let adapter = match state.registry.require(harness_kind) {
        Ok(adapter) => adapter,
        Err(err) => return fail_execution(&state, &execution, err).await,
    };

    let model = context.model.map(|model| model.into_resolved());
    let mut input: orbit_contracts::ExecutionInput =
        match serde_json::from_value(execution.input.clone()) {
            Ok(input) => input,
            Err(err) => {
                return fail_execution(
                    &state,
                    &execution,
                    HarnessError::new(
                        HarnessErrorKind::InvalidConfig,
                        format!("execution input is invalid: {err}"),
                    ),
                )
                .await
            }
        };

    // Read-only connector hydration (MVP: X recent search). External content is added
    // as untrusted payload; trusted instructions remain separate.
    match crate::connectors::hydrate(
        &state.client,
        execution_id,
        &spec,
        &input.task,
        &input.payload,
    )
    .await
    {
        Ok(hydrated) => {
            input.payload = hydrated.payload;
            for note in hydrated.notes {
                append_event(
                    &state,
                    &execution,
                    ExecutionEventKind::Log,
                    LogLevel::Info,
                    &note,
                    serde_json::json!({ "connector": "x" }),
                )
                .await;
            }
        }
        Err(error) => return fail_execution(&state, &execution, error).await,
    }

    let grants = context.grants;

    // Short-term memory: retrieve recent memory entries for this agent and inject
    // them into the execution input so the harness can use them as context.
    if spec.memory.enabled && spec.memory.strategy != orbit_domain::MemoryStrategy::None {
        if let Ok(entries) = state
            .client
            .list_memory(execution_id, execution.agent_id, spec.memory.retention_days)
            .await
        {
            if !entries.is_empty() {
                let memory_text = entries
                    .iter()
                    .map(|e| format!("[{}] {}", e.kind, e.content))
                    .collect::<Vec<_>>()
                    .join("\n");
                if let Some(payload) = input.payload.as_object_mut() {
                    payload.insert("memory".into(), serde_json::json!(memory_text));
                }
            }
        }
    }

    if let Err(err) = tokio::fs::create_dir_all(&workspace).await {
        return fail_execution(
            &state,
            &execution,
            HarnessError::new(
                HarnessErrorKind::Internal,
                format!("could not create workspace {}: {err}", workspace.display()),
            ),
        )
        .await;
    }
    let checkout_connection = spec
        .connectors
        .iter()
        .find(|binding| {
            matches!(binding.connector.as_str(), "github" | "gitlab")
                && binding
                    .capabilities
                    .iter()
                    .any(|capability| capability == "repo.checkout")
        })
        .and_then(|binding| binding.connection_id);
    let checkout = tokio::select! {
        _ = cancel.cancelled() => Err(anyhow::anyhow!("repository checkout cancelled")),
        result = crate::git::checkout_workspace(
            &state.client,
            execution_id,
            checkout_connection,
            &execution.input,
            &workspace,
            &cancel,
        ) => result,
    };
    match checkout {
        Ok(notes) => {
            for note in notes {
                append_event(
                    &state,
                    &execution,
                    ExecutionEventKind::Status,
                    LogLevel::Info,
                    &note,
                    serde_json::json!({ "connector": "github", "capability": "repo.checkout" }),
                )
                .await;
            }
        }
        Err(error) => {
            let message = error.to_string();
            let error = if cancel.is_cancelled() || message.contains("cancelled") {
                HarnessError::cancelled()
            } else if let Some(seconds) = message
                .split("git clone timed out after ")
                .nth(1)
                .and_then(|value| value.split(" seconds").next())
                .and_then(|value| value.parse::<u64>().ok())
            {
                HarnessError::timeout(seconds)
            } else {
                HarnessError::new(
                    HarnessErrorKind::ExecutionFailed,
                    format!("repository checkout failed: {message}"),
                )
            };
            return fail_execution(&state, &execution, error).await;
        }
    }

    let mut subagent_model_selectors = std::collections::BTreeMap::new();
    for subagent in &context.subagent_models {
        if let Some(selector) = subagent.model.as_ref().and_then(worker_model_selector) {
            subagent_model_selectors.insert(subagent.agent_id, selector);
        }
    }
    match crate::subagents::materialize(&execution.input, &workspace, &subagent_model_selectors)
        .await
    {
        Ok(names) if !names.is_empty() => {
            append_event(
                &state,
                &execution,
                ExecutionEventKind::Status,
                LogLevel::Info,
                &format!("materialized {} native subagent(s)", names.len()),
                serde_json::json!({ "subagents": names }),
            )
            .await;
        }
        Ok(_) => {}
        Err(error) => {
            tracing::warn!(error = %error, "could not materialize native subagents");
        }
    }

    // Shared with the broker: it pauses the run's clock while a person decides on an
    // approval, so that wait does not eat into the execution timeout.
    // Total approval time is capped (ORBIT_MAX_APPROVAL_WAIT_SECONDS); past it the run
    // ends as timed out.
    let pause = orbit_contracts::PauseClock::with_budget(Duration::from_secs(
        orbit_contracts::max_approval_wait_seconds(),
    ));

    // Policy-enforcing tool broker: only for harnesses that can consume MCP today
    // (OpenCode). It is execution-scoped and dropped at the end of this function.
    let broker = if matches!(harness_kind, HarnessKind::OpenCode | HarnessKind::Pi) {
        match crate::broker::start(
            state.client.clone(),
            &spec,
            &execution,
            &workspace,
            &context.tool_policies,
            pause.clone(),
        )
        .await
        {
            Ok(broker) => broker,
            Err(error) => {
                return fail_execution(
                    &state,
                    &execution,
                    HarnessError::new(
                        HarnessErrorKind::InvalidConfig,
                        format!("tool broker could not start: {error}"),
                    ),
                )
                .await
            }
        }
    } else {
        None
    };
    if let Some(handle) = &broker {
        append_event(
            &state,
            &execution,
            ExecutionEventKind::Status,
            LogLevel::Info,
            &format!(
                "policy-enforcing tool broker ready ({} GitHub tools)",
                handle.tool_count
            ),
            serde_json::json!({ "broker": "github", "tools": handle.tool_count }),
        )
        .await;
    }
    let materialized_skills = match crate::skills::materialize(
        &spec,
        &execution.skills,
        &state.profile_skill_slugs,
        &context.skills,
        &workspace,
    )
    .await
    {
        Ok(skills) => skills,
        Err(error) => {
            return fail_execution(
                &state,
                &execution,
                HarnessError::new(
                    HarnessErrorKind::Internal,
                    format!("skill materialization failed: {error:#}"),
                ),
            )
            .await;
        }
    };
    if !materialized_skills.is_empty() {
        append_event(
            &state,
            &execution,
            ExecutionEventKind::Status,
            LogLevel::Info,
            &format!("materialized {} skill(s)", materialized_skills.len()),
            serde_json::json!({ "skills": materialized_skills }),
        )
        .await;
    }

    let (event_tx, mut event_rx) = mpsc::channel::<HarnessEvent>(256);
    let running = state
        .retry
        .run("report running", || {
            state
                .client
                .running(execution_id, execution.session_ref.as_deref())
        })
        .await;
    if let Err(error) = running {
        if crate::retry::status_of(&error) == Some(409) {
            // The platform already ended this run (cancelled, or reaped as unstarted).
            tracing::warn!(%execution_id, %error, "execution already ended; not starting harness");
            return Ok(());
        }
        return fail_execution(
            &state,
            &execution,
            HarnessError::new(
                HarnessErrorKind::ExecutionFailed,
                format!("could not report execution start to the control plane: {error}"),
            ),
        )
        .await;
    }
    append_event(
        &state,
        &execution,
        ExecutionEventKind::Status,
        LogLevel::Info,
        "execution dispatched to harness",
        // Provider and model as configured on the agent, so a run records what it used.
        serde_json::json!({
            "harness": harness_kind.as_str(),
            "worker": state.worker_id,
            "provider": spec.model.provider,
            "model": spec.model.model,
        }),
    )
    .await;

    // Persist harness events as they arrive.
    let event_state = state.clone();
    let event_execution = execution.clone();
    let event_task = tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            let (kind, level, message, data) = map_harness_event(event);
            append_event(&event_state, &event_execution, kind, level, &message, data).await;
        }
    });

    // Deliver cancellation requests from the control plane.
    let watcher_client = state.client.clone();
    let watcher_cancel = cancel.clone();
    let done = Arc::new(AtomicBool::new(false));
    let watcher_done = done.clone();
    let user_cancelled = Arc::new(AtomicBool::new(false));
    let watcher_user_cancelled = user_cancelled.clone();
    let watcher = tokio::spawn(async move {
        while !watcher_done.load(Ordering::Relaxed) {
            tokio::time::sleep(Duration::from_millis(1000)).await;
            match watcher_client.cancel_requested(execution_id).await {
                Ok(true) => {
                    tracing::info!(%execution_id, "cancellation requested; cancelling harness");
                    watcher_user_cancelled.store(true, Ordering::Relaxed);
                    watcher_cancel.cancel();
                    break;
                }
                Ok(false) => {}
                Err(err) => tracing::warn!(error = %err, "cancel poll failed"),
            }
        }
    });

    // Add the MCP bridge details to the harness config. This stays in memory only; the
    // execution-scoped token is invalid as soon as the broker is dropped.
    let mut harness_config = spec
        .harness_config
        .get(harness_kind.as_str())
        .cloned()
        .or_else(|| spec.harness.as_ref().map(|harness| harness.config.clone()))
        .unwrap_or_else(|| serde_json::json!({}));
    if harness_kind == HarnessKind::OpenCode {
        if !harness_config.is_object() {
            harness_config = serde_json::json!({});
        }
        harness_config["webSearch"] = serde_json::json!(state.allow_web_search);
        harness_config["webFetch"] = serde_json::json!(state.allow_web_fetch);
        if state.native_tool_permissions.is_object() {
            harness_config["nativeToolPermissions"] = state.native_tool_permissions.clone();
        }
    }
    if let (Some(handle), HarnessKind::OpenCode | HarnessKind::Pi) = (&broker, harness_kind) {
        if !harness_config.is_object() {
            harness_config = serde_json::json!({});
        }
        harness_config["orbitMcp"] = serde_json::json!({
            "command": handle.mcp_command(),
            "environment": {
                "ORBIT_BROKER_URL": handle.url,
                "ORBIT_BROKER_TOKEN": handle.token,
            }
        });
    }

    let autonomy = AutonomyLevel::from_level(spec.autonomy.level).unwrap_or(AutonomyLevel::OBSERVE);
    let mut system_instructions = spec.instructions.system.clone();
    if let Some(extra) = crate::subagents::system_instruction(&execution.input) {
        if !system_instructions.is_empty() {
            system_instructions.push_str("\n\n");
        }
        system_instructions.push_str(&extra);
    }
    let mut extra_env = (*state.extra_env).clone();
    for item in &context.env {
        extra_env.insert(item.env_var.clone(), item.value.clone());
    }
    let request = ExecutionRequest {
        execution_id,
        agent_id: execution.agent_id,
        agent_version_id: execution.agent_version_id,
        agent_name: execution.agent_name.clone(),
        instructions: InstructionsBlock {
            system: system_instructions,
            constraints: spec.instructions.constraints.clone(),
        },
        input,
        model,
        subagent_models: context
            .subagent_models
            .iter()
            .filter_map(|subagent| subagent.model.clone())
            .map(crate::client::WorkerModel::into_resolved)
            .collect(),
        grants,
        autonomy,
        resources: ResourceLimits {
            timeout_seconds: execution.timeout_seconds.max(1) as u64,
            ..spec.resources.clone()
        },
        harness_config,
        extra_env,
        session: SessionRequest {
            session_ref: execution.session_ref.clone(),
            mode: spec
                .harness
                .as_ref()
                .map(|harness| harness.execution_mode)
                .unwrap_or(orbit_domain::ExecutionMode::OnDemand),
        },
        workspace: orbit_contracts::WorkspaceSpec {
            root: workspace.to_string_lossy().to_string(),
            read_only: false,
        },
        events: Some(event_tx),
        cancel: cancel.clone(),
        pause: pause.clone(),
    };

    let timeout_seconds = request.resources.timeout_seconds;
    let started = std::time::Instant::now();
    let outcome = tokio::select! {
        _ = cancel.cancelled() => {
            let mut outcome = ExecutionOutcome::failed(HarnessError::cancelled());
            outcome.status = ExecutionStatus::Cancelled;
            outcome
        }
        // Backstop for an adapter that ignores its own timeout. Same pause-aware clock.
        _ = pause.deadline(Duration::from_secs(timeout_seconds.saturating_add(5))) => {
            let mut outcome = ExecutionOutcome::failed(HarnessError::timeout(timeout_seconds));
            outcome.status = ExecutionStatus::TimedOut;
            outcome
        }
        result = adapter.execute(request) => match result {
            Ok(outcome) => outcome,
            Err(err) => outcome_from_error(err),
        }
    };

    // Stop the cancellation watcher before finishing.
    done.store(true, Ordering::Relaxed);
    watcher.abort();

    // Drain the event channel: drop the sender by ending the adapter future, then wait.
    let _ = tokio::time::timeout(Duration::from_secs(2), event_task).await;

    let mut outcome = outcome;
    if outcome.status == ExecutionStatus::Cancelled
        && !user_cancelled.load(Ordering::Relaxed)
        && state.shutdown.is_cancelled()
    {
        // Interrupted by worker shutdown rather than by a person: report it as a failed
        // run with a clear reason, not as a user cancellation.
        outcome = ExecutionOutcome::failed(HarnessError::new(
            HarnessErrorKind::ExecutionFailed,
            SHUTDOWN_MESSAGE,
        ));
        outcome.status = ExecutionStatus::Failed;
    }
    if outcome.usage.wall_time_ms.is_none() {
        outcome.usage.wall_time_ms = Some(started.elapsed().as_millis() as i64);
    }

    let status = if outcome.status.is_terminal() {
        outcome.status
    } else {
        ExecutionStatus::Succeeded
    };
    let error_json = outcome
        .error
        .as_ref()
        .and_then(|error| serde_json::to_value(error).ok());

    append_event(
        &state,
        &execution,
        if status == ExecutionStatus::Succeeded {
            ExecutionEventKind::Status
        } else {
            ExecutionEventKind::Error
        },
        if status == ExecutionStatus::Succeeded {
            LogLevel::Info
        } else {
            LogLevel::Error
        },
        &match status {
            ExecutionStatus::Succeeded => "execution succeeded".to_string(),
            other => format!("execution finished with status {other}"),
        },
        serde_json::json!({
            "status": status.as_str(),
            "usage": outcome.usage,
        }),
    )
    .await;

    let updated = complete_with_retry(
        &state,
        execution_id,
        &crate::client::CompleteRequest {
            status: status.as_str(),
            output: outcome.output.clone(),
            error: error_json,
            usage: serde_json::to_value(&outcome.usage).ok(),
            session_ref: outcome.session_ref.as_deref(),
        },
    )
    .await?;
    if updated && status == ExecutionStatus::Succeeded {
        // Store execution output as short-term memory for the agent.
        if spec.memory.enabled {
            let kind = if spec.memory.strategy == orbit_domain::MemoryStrategy::Summary {
                "summary"
            } else {
                "short_term"
            };
            let content = outcome
                .output
                .as_ref()
                .map(|o| o.to_string())
                .unwrap_or_default();
            // The API caps memory entries at 64 KiB; keep the head within that bound.
            let content = truncate_utf8(&content, 60 * 1024);
            if !content.is_empty() {
                if let Err(error) = state
                    .client
                    .store_memory(execution_id, execution.agent_id, kind, content)
                    .await
                {
                    tracing::warn!(%error, %execution_id, "could not store execution memory");
                }
            }
        }
    }
    if updated {
        tracing::info!(
            %execution_id,
            status = status.as_str(),
            duration_ms = started.elapsed().as_millis() as u64,
            "execution finished"
        );
    } else {
        tracing::warn!(%execution_id, "execution was already terminal; result discarded");
    }
    Ok(())
}

fn outcome_from_error(error: HarnessError) -> ExecutionOutcome {
    let status = match error.kind {
        HarnessErrorKind::Cancelled => ExecutionStatus::Cancelled,
        HarnessErrorKind::Timeout => ExecutionStatus::TimedOut,
        _ => ExecutionStatus::Failed,
    };
    let mut outcome = ExecutionOutcome::failed(error);
    outcome.status = status;
    outcome
}

pub(crate) async fn fail_execution(
    state: &WorkerState,
    execution: &ExecutionRecord,
    error: HarnessError,
) -> anyhow::Result<()> {
    let outcome = outcome_from_error(error);
    append_event(
        state,
        execution,
        ExecutionEventKind::Error,
        LogLevel::Error,
        &outcome
            .error
            .as_ref()
            .map(|error| error.message.clone())
            .unwrap_or_else(|| "execution failed".into()),
        serde_json::json!({ "error": outcome.error }),
    )
    .await;
    let error_json = outcome
        .error
        .as_ref()
        .and_then(|error| serde_json::to_value(error).ok());
    complete_with_retry(
        state,
        execution.id,
        &crate::client::CompleteRequest {
            status: outcome.status.as_str(),
            output: None,
            error: error_json,
            usage: None,
            session_ref: None,
        },
    )
    .await?;
    Ok(())
}

async fn append_event(
    state: &WorkerState,
    execution: &ExecutionRecord,
    kind: ExecutionEventKind,
    level: LogLevel,
    message: &str,
    data: serde_json::Value,
) {
    if let Err(error) = state
        .client
        .event(
            execution.id,
            &crate::client::EventRequest {
                kind: kind.as_str(),
                level: level.as_str(),
                message,
                data,
            },
        )
        .await
    {
        tracing::error!(%error, "failed to persist execution event");
    }
}

fn map_harness_event(
    event: HarnessEvent,
) -> (ExecutionEventKind, LogLevel, String, serde_json::Value) {
    let kind = match event.kind {
        HarnessEventKind::Started | HarnessEventKind::Status => ExecutionEventKind::Status,
        HarnessEventKind::Log => ExecutionEventKind::Log,
        HarnessEventKind::ToolCall => ExecutionEventKind::ToolCall,
        HarnessEventKind::OutputDelta => ExecutionEventKind::Output,
        HarnessEventKind::Usage => ExecutionEventKind::Metric,
        HarnessEventKind::Error => ExecutionEventKind::Error,
    };
    (kind, event.level, event.message, event.data)
}

/// Longest prefix of `value` that fits in `max_bytes` without splitting a character.
fn truncate_utf8(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    use axum::extract::{Path, State};
    use axum::http::StatusCode;
    use axum::routing::post;
    use axum::{Json, Router};

    use super::*;
    use crate::retry::RetryPolicy;

    /// Minimal stand-in for the worker-facing control-plane API.
    #[derive(Clone, Default)]
    struct FakeApi {
        /// Number of initial `/complete` calls answered with 503.
        fail_complete: usize,
        complete_calls: Arc<AtomicUsize>,
        completes: Arc<Mutex<Vec<serde_json::Value>>>,
        events: Arc<Mutex<Vec<serde_json::Value>>>,
    }

    async fn complete(
        State(api): State<FakeApi>,
        Path(_id): Path<Uuid>,
        Json(body): Json<serde_json::Value>,
    ) -> (StatusCode, Json<serde_json::Value>) {
        let call = api.complete_calls.fetch_add(1, Ordering::SeqCst);
        if call < api.fail_complete {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "error": { "message": "unavailable" } })),
            );
        }
        api.completes.lock().unwrap().push(body);
        (StatusCode::OK, Json(serde_json::json!({ "updated": true })))
    }

    async fn event(
        State(api): State<FakeApi>,
        Path(_id): Path<Uuid>,
        Json(body): Json<serde_json::Value>,
    ) -> Json<serde_json::Value> {
        api.events.lock().unwrap().push(body);
        Json(serde_json::json!({}))
    }

    async fn conflict(Path(_id): Path<Uuid>) -> (StatusCode, Json<serde_json::Value>) {
        (
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "error": { "message": "already ended" } })),
        )
    }

    async fn serve(api: FakeApi) -> WorkerState {
        let app = Router::new()
            .route("/api/v1/worker/executions/{id}/complete", post(complete))
            .route("/api/v1/worker/executions/{id}/events", post(event))
            .route("/api/v1/worker/executions/{id}/running", post(conflict))
            .with_state(api);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        WorkerState {
            worker_id: "test-worker".into(),
            client: crate::client::WorkerClient::new(format!("http://{addr}"), "token").unwrap(),
            registry: Arc::new(orbit_harness::HarnessRegistry::builtin()),
            workspace_root: std::env::temp_dir(),
            harnesses: vec!["mock".into()],
            execution_driver: "process".into(),
            target: "local".into(),
            profile_agent_ids: vec![],
            profile_skill_slugs: vec![],
            allow_web_search: false,
            allow_web_fetch: false,
            native_tool_permissions: serde_json::Value::Null,
            extra_env: Arc::new(Default::default()),
            active: Default::default(),
            shutdown: tokio_util::sync::CancellationToken::new(),
            retry: RetryPolicy {
                attempts: 4,
                base: Duration::from_millis(5),
                max_delay: Duration::from_millis(20),
            },
        }
    }

    fn succeeded_request() -> crate::client::CompleteRequest<'static> {
        crate::client::CompleteRequest {
            status: "succeeded",
            output: None,
            error: None,
            usage: None,
            session_ref: None,
        }
    }

    #[tokio::test]
    async fn complete_is_retried_after_a_503_and_reaches_terminal() {
        let api = FakeApi {
            fail_complete: 1,
            ..Default::default()
        };
        let state = serve(api.clone()).await;
        let updated = complete_with_retry(&state, Uuid::now_v7(), &succeeded_request())
            .await
            .expect("retry should outlast a single 503");
        assert!(updated);
        assert_eq!(api.complete_calls.load(Ordering::SeqCst), 2);
        let completes = api.completes.lock().unwrap();
        assert_eq!(completes.len(), 1);
        assert_eq!(completes[0]["status"], "succeeded");
    }

    #[tokio::test]
    async fn complete_gives_up_after_bounded_attempts() {
        let api = FakeApi {
            fail_complete: usize::MAX,
            ..Default::default()
        };
        let state = serve(api.clone()).await;
        let result = complete_with_retry(&state, Uuid::now_v7(), &succeeded_request()).await;
        assert!(result.is_err());
        assert_eq!(api.complete_calls.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn definitive_4xx_answers_are_not_retried() {
        let state = serve(FakeApi::default()).await;
        let calls = AtomicUsize::new(0);
        let error = state
            .retry
            .run("running", || {
                calls.fetch_add(1, Ordering::SeqCst);
                state.client.running(Uuid::now_v7(), None)
            })
            .await
            .unwrap_err();
        assert_eq!(crate::retry::status_of(&error), Some(409));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn runner_panic_reports_failed_and_cleans_up_active() {
        let api = FakeApi::default();
        let state = serve(api.clone()).await;
        let id = Uuid::now_v7();
        let (guard, _cancel) = track(&state, id);
        assert_eq!(active_ids(&state), vec![id]);

        let result = supervise(&state, id, guard, async {
            if true {
                panic!("boom");
            }
            Ok(())
        })
        .await;

        assert!(result.is_err());
        assert!(
            active_ids(&state).is_empty(),
            "active entry must be removed"
        );
        let completes = api.completes.lock().unwrap();
        assert_eq!(completes.len(), 1);
        assert_eq!(completes[0]["status"], "failed");
        assert!(completes[0]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("panicked: boom"));
        assert_eq!(api.events.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn runner_error_reports_failed_and_cleans_up_active() {
        let api = FakeApi::default();
        let state = serve(api.clone()).await;
        let id = Uuid::now_v7();
        let (guard, _cancel) = track(&state, id);

        let result = supervise(&state, id, guard, async { Err(anyhow::anyhow!("lost it")) }).await;

        assert!(result.is_err());
        assert!(active_ids(&state).is_empty());
        let completes = api.completes.lock().unwrap();
        assert_eq!(completes[0]["status"], "failed");
        assert!(completes[0]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("lost it"));
    }

    #[tokio::test]
    async fn shutdown_reports_executions_still_running_after_grace() {
        let api = FakeApi::default();
        let state = serve(api.clone()).await;
        let id = Uuid::now_v7();
        let (guard, cancel) = track(&state, id);

        shutdown(&state, Duration::from_millis(150)).await;

        assert!(cancel.is_cancelled(), "in-flight execution is signalled");
        let completes = api.completes.lock().unwrap();
        assert_eq!(completes.len(), 1);
        assert_eq!(completes[0]["status"], "failed");
        assert_eq!(completes[0]["error"]["code"], "worker_shutdown");
        drop(guard);
    }
}

#[cfg(test)]
mod cleanup_tests {
    use super::{finish_execution_workspace, run_execution, WorkerState};
    use crate::client::{WorkerClient, WorkerExecutionContext};
    use axum::{http::Uri, routing::any, Json, Router};
    use orbit_harness::HarnessRegistry;
    use std::{collections::BTreeMap, sync::Arc, time::Duration};
    use uuid::Uuid;

    async fn mock_worker_api(uri: Uri) -> Json<serde_json::Value> {
        let response = if uri.path().ends_with("/cancel") {
            serde_json::json!({ "cancelRequested": false })
        } else if uri.path().ends_with("/complete") {
            serde_json::json!({ "updated": true })
        } else {
            serde_json::json!({})
        };
        Json(response)
    }

    fn execution_context(task: &str) -> WorkerExecutionContext {
        let now = chrono::Utc::now();
        let execution_id = Uuid::now_v7();
        let execution = serde_json::from_value(serde_json::json!({
            "id": execution_id,
            "orgId": Uuid::now_v7(),
            "agentId": Uuid::now_v7(),
            "agentVersionId": Uuid::now_v7(),
            "agentName": "cleanup test",
            "harnessKind": "mock",
            "triggerId": null,
            "triggerSource": "manual",
            "placement": "local",
            "status": "dispatched",
            "sessionRef": null,
            "input": {
                "task": task,
                "payload": {},
                "provenance": {
                    "source": "test",
                    "trusted": false,
                    "receivedAt": now
                }
            },
            "skills": [],
            "output": null,
            "error": null,
            "usage": {},
            "timeoutSeconds": 10,
            "idempotencyKey": null,
            "requestedBy": null,
            "workerId": "cleanup-test",
            "teamRunId": null,
            "teamStepIndex": null,
            "createdAt": now,
            "startedAt": null,
            "finishedAt": null,
            "updatedAt": now,
            "archivedAt": null
        }))
        .unwrap();
        let spec = serde_json::from_value(serde_json::json!({
            "instructions": { "system": "test" }
        }))
        .unwrap();

        WorkerExecutionContext {
            execution,
            spec,
            model: None,
            grants: Vec::new(),
            skills: Vec::new(),
            tool_policies: Vec::new(),
            env: Vec::new(),
            subagent_models: Vec::new(),
        }
    }

    #[tokio::test]
    async fn execution_workspace_is_removed_after_success_or_failure() {
        for failed in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let workspace = root.path().join("execution");
            tokio::fs::create_dir_all(workspace.join("nested"))
                .await
                .unwrap();
            tokio::fs::write(workspace.join("nested/output.txt"), "output")
                .await
                .unwrap();

            let result = if failed {
                Err(anyhow::anyhow!("execution failed"))
            } else {
                Ok(())
            };
            let final_result = finish_execution_workspace(&workspace, Uuid::now_v7(), result).await;

            assert_eq!(final_result.is_err(), failed);
            assert!(!workspace.exists());
        }
    }

    #[tokio::test]
    async fn run_execution_cleans_workspace_after_success_and_harness_failure() {
        let root = tempfile::tempdir().unwrap();
        let workspace_root = root.path().join("workspaces");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, Router::new().fallback(any(mock_worker_api)))
                .await
                .unwrap();
        });
        let state = WorkerState {
            worker_id: "cleanup-test".into(),
            client: WorkerClient::new(format!("http://{address}"), "test-token").unwrap(),
            registry: Arc::new(HarnessRegistry::builtin()),
            workspace_root: workspace_root.clone(),
            harnesses: vec!["mock".into()],
            execution_driver: "process".into(),
            target: "local".into(),
            profile_agent_ids: Vec::new(),
            profile_skill_slugs: Vec::new(),
            allow_web_search: false,
            allow_web_fetch: false,
            native_tool_permissions: serde_json::json!({}),
            extra_env: Arc::new(BTreeMap::new()),
            active: Default::default(),
            shutdown: tokio_util::sync::CancellationToken::new(),
            retry: crate::retry::RetryPolicy {
                attempts: 4,
                base: Duration::from_millis(5),
                max_delay: Duration::from_millis(20),
            },
        };

        for index in 0..10 {
            let task = if index % 2 == 0 {
                "complete normally"
            } else {
                "please fail"
            };
            let context = execution_context(task);
            let execution_id = context.execution.id;
            run_execution(
                state.clone(),
                context,
                tokio_util::sync::CancellationToken::new(),
            )
            .await
            .unwrap();
            assert!(
                !workspace_root.join(execution_id.to_string()).exists(),
                "workspace for {task:?} should be removed"
            );
        }

        assert!(state.active.lock().unwrap().is_empty());
        server.abort();
        let _ = server.await;
    }
}
