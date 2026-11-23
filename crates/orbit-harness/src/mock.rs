use std::time::{Duration, Instant};

use async_trait::async_trait;
use orbit_contracts::{
    ExecutionOutcome, ExecutionRequest, HarnessAdapter, HarnessCapabilities, HarnessConfig,
    HarnessDescriptor, HarnessError, HarnessErrorKind, HarnessEvent, HarnessEventKind,
    HarnessHealth,
};
use orbit_domain::{
    ExecutionMode, ExecutionStatus, HarnessKind, ImplementationStatus, LogLevel, Usage,
    ValidationReport,
};

/// Deterministic, in-process harness used for development, demos and contract tests.
///
/// It never executes untrusted code; it simulates a harness so the full control-plane
/// flow (queue → dispatch → events → terminal state) can be exercised without credentials.
#[derive(Debug, Clone)]
pub struct MockHarnessAdapter {
    pub step_delay: Duration,
}

impl Default for MockHarnessAdapter {
    fn default() -> Self {
        Self {
            step_delay: Duration::from_millis(150),
        }
    }
}

impl MockHarnessAdapter {
    pub fn descriptor() -> HarnessDescriptor {
        HarnessDescriptor {
            kind: HarnessKind::Mock,
            name: "Mock harness".into(),
            description:
                "Deterministic local harness for development and testing. Does not call a model."
                    .into(),
            capabilities: HarnessCapabilities {
                supports_sessions: true,
                supports_resume: true,
                supports_streaming: true,
                supports_cancel: true,
                supports_instances: false,
                supports_tool_events: true,
                supports_usage_reporting: true,
                harness_managed_providers: false,
                supports_platform_providers: true,
                execution_modes: vec![ExecutionMode::OnDemand],
                max_timeout_seconds: 3600,
                max_memory_mb: 2048,
            },
            implementation_status: ImplementationStatus::Available,
            config_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "stepDelayMs": { "type": "integer", "minimum": 0, "maximum": 10000 }
                }
            }),
            docs_url: None,
        }
    }

    async fn emit(sender: &Option<tokio::sync::mpsc::Sender<HarnessEvent>>, event: HarnessEvent) {
        if let Some(sender) = sender {
            let _ = sender.send(event).await;
        }
    }

    /// `session` receives the session ref as soon as it is known, so a run that is
    /// cancelled or times out can still report which session it belonged to.
    async fn run(
        request: ExecutionRequest,
        session: std::sync::Arc<std::sync::OnceLock<String>>,
    ) -> Result<ExecutionOutcome, HarnessError> {
        let started = Instant::now();
        let task = request.input.task.trim().to_string();
        let task_is_empty = task.is_empty();
        let session_ref = request
            .session
            .session_ref
            .clone()
            .unwrap_or_else(|| format!("mock-{}", uuid::Uuid::now_v7()));
        let _ = session.set(session_ref.clone());

        Self::emit(&request.events, HarnessEvent::started(&session_ref)).await;
        Self::emit(
            &request.events,
            HarnessEvent::log(LogLevel::Info, "mock harness: received execution").with_data(
                serde_json::json!({
                    "autonomy": request.autonomy.level(),
                    "trustedSource": request.input.provenance.source,
                    "untrusted": !request.input.provenance.trusted,
                }),
            ),
        )
        .await;

        if request.session.session_ref.is_none() {
            tokio::time::sleep(Duration::from_millis(40)).await;
        }

        if task.contains("fail") {
            Self::emit(
                &request.events,
                HarnessEvent::new(
                    HarnessEventKind::Error,
                    LogLevel::Error,
                    "simulated harness failure",
                ),
            )
            .await;
            let mut outcome = ExecutionOutcome::failed(HarnessError::new(
                HarnessErrorKind::ExecutionFailed,
                "simulated failure requested by the task",
            ));
            outcome.session_ref = Some(session_ref);
            outcome.usage.wall_time_ms = Some(started.elapsed().as_millis() as i64);
            return Ok(outcome);
        }

        if task.contains("slow") {
            tokio::time::sleep(Duration::from_secs(25)).await;
        }

        for (index, step) in [
            "Reading agent instructions",
            "Inspecting provided context",
            "Composing response",
        ]
        .iter()
        .enumerate()
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Self::emit(
                &request.events,
                HarnessEvent::log(LogLevel::Info, format!("{}. {}", index + 1, step)),
            )
            .await;
        }

        let summary = if task_is_empty {
            "No task provided; the agent produced an empty observation.".to_string()
        } else {
            format!(
                "Mock harness processed {} characters of input for agent '{}'.",
                task.chars().count(),
                request.agent_name
            )
        };
        let output = serde_json::json!({
            "summary": summary,
            "task": task,
            "harness": "mock",
            "model": request.model.as_ref().map(|m| serde_json::to_value(&m.selection).unwrap_or_default()),
            "grantedCapabilities": request.grants.iter().map(|g| g.capability.clone()).collect::<Vec<_>>(),
            "observedInput": {
                "source": request.input.provenance.source,
                "trusted": request.input.provenance.trusted,
            },
        });

        Self::emit(&request.events, HarnessEvent::output(summary.clone())).await;

        let mut outcome = ExecutionOutcome::succeeded(output, Some(session_ref));
        outcome.usage = Usage {
            input_tokens: Some((task.chars().count() as i64 / 4).max(1)),
            output_tokens: Some((summary.chars().count() as i64 / 4).max(1)),
            cpu_millis: Some(started.elapsed().as_millis() as i64),
            wall_time_ms: Some(started.elapsed().as_millis() as i64),
        };
        Ok(outcome)
    }
}

#[async_trait]
impl HarnessAdapter for MockHarnessAdapter {
    fn kind(&self) -> HarnessKind {
        HarnessKind::Mock
    }

    fn descriptor(&self) -> HarnessDescriptor {
        Self::descriptor()
    }

    async fn validate_config(&self, config: &HarnessConfig) -> ValidationReport {
        let mut report = ValidationReport::default();
        if config.kind != HarnessKind::Mock {
            report.push("type", "this adapter only accepts the 'mock' harness kind");
        }
        if let Some(delay) = config.settings.get("stepDelayMs").and_then(|v| v.as_u64()) {
            if delay > 10_000 {
                report.push("stepDelayMs", "must be <= 10000");
            }
        }
        report
    }

    async fn health(&self, _config: &HarnessConfig) -> Result<HarnessHealth, HarnessError> {
        Ok(HarnessHealth::healthy())
    }

    async fn execute(&self, request: ExecutionRequest) -> Result<ExecutionOutcome, HarnessError> {
        let timeout = Duration::from_secs(request.resources.timeout_seconds);
        let cancel = request.cancel.clone();
        let events = request.events.clone();
        let timeout_seconds = request.resources.timeout_seconds;
        let started = Instant::now();
        let pause = request.pause.clone();

        let session = std::sync::Arc::new(std::sync::OnceLock::new());
        let future = Self::run(request, session.clone());
        tokio::pin!(future);

        let outcome = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                Self::emit(&events, HarnessEvent::log(LogLevel::Warn, "execution cancelled")).await;
                let mut outcome = ExecutionOutcome::failed(HarnessError::cancelled());
                outcome.status = ExecutionStatus::Cancelled;
                // A continuous runtime resumes from here: keep the session it started.
                outcome.session_ref = session.get().cloned();
                outcome
            }
            result = &mut future => result?,
            _ = pause.deadline(timeout) => {
                Self::emit(&events, HarnessEvent::new(HarnessEventKind::Error, LogLevel::Error, "execution timed out")).await;
                let mut outcome = ExecutionOutcome::failed(HarnessError::timeout(timeout_seconds));
                outcome.status = ExecutionStatus::TimedOut;
                outcome.session_ref = session.get().cloned();
                outcome
            }
        };

        let mut outcome = outcome;
        if outcome.usage.wall_time_ms.is_none() {
            outcome.usage.wall_time_ms = Some(started.elapsed().as_millis() as i64);
        }
        Ok(outcome)
    }
}
