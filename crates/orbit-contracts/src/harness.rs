use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use orbit_domain::{HarnessKind, ValidationReport};

/// Capabilities a harness advertises. The UI and dispatch logic must branch on these,
/// never on a hardcoded harness name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HarnessCapabilities {
    pub supports_sessions: bool,
    pub supports_resume: bool,
    pub supports_streaming: bool,
    pub supports_cancel: bool,
    pub supports_instances: bool,
    pub supports_tool_events: bool,
    pub supports_usage_reporting: bool,
    /// The harness owns its provider inventory and credentials (OpenCode Go, Claude Code).
    pub harness_managed_providers: bool,
    /// Harnesses that can consume a provider configured by ORBIT.
    pub supports_platform_providers: bool,
    pub execution_modes: Vec<orbit_domain::ExecutionMode>,
    pub max_timeout_seconds: u64,
    pub max_memory_mb: u32,
}

impl Default for HarnessCapabilities {
    fn default() -> Self {
        Self {
            supports_sessions: true,
            supports_resume: false,
            supports_streaming: true,
            supports_cancel: true,
            supports_instances: false,
            supports_tool_events: false,
            supports_usage_reporting: false,
            harness_managed_providers: false,
            supports_platform_providers: true,
            execution_modes: vec![orbit_domain::ExecutionMode::OnDemand],
            max_timeout_seconds: 3600,
            max_memory_mb: 8192,
        }
    }
}

/// Static description of a harness family, exposed through `GET /api/v1/harnesses`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HarnessDescriptor {
    pub kind: HarnessKind,
    pub name: String,
    pub description: String,
    pub capabilities: HarnessCapabilities,
    pub implementation_status: orbit_domain::ImplementationStatus,
    pub config_schema: serde_json::Value,
    #[serde(default)]
    pub docs_url: Option<String>,
}

/// Harness settings that only the platform may set: the CLI binary, raw CLI arguments,
/// the execution-scoped MCP bridge (which carries the policy hook) and a shared server
/// URL. An agent spec is author-controlled, so the API rejects these in
/// `spec.harnessConfig` and the worker removes them from configs saved earlier (#303).
pub const PLATFORM_ONLY_HARNESS_SETTINGS: &[&str] =
    &["binary", "extraArgs", "orbitMcp", "serverUrl"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessConfig {
    pub kind: HarnessKind,
    #[serde(default)]
    pub settings: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessHealth {
    pub healthy: bool,
    pub message: Option<String>,
    #[serde(default)]
    pub details: BTreeMap<String, String>,
}

impl HarnessHealth {
    pub fn healthy() -> Self {
        Self {
            healthy: true,
            message: None,
            details: BTreeMap::new(),
        }
    }

    pub fn degraded(message: impl Into<String>) -> Self {
        Self {
            healthy: false,
            message: Some(message.into()),
            details: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSpec {
    /// Per-execution scratch directory. Must be writable and isolated.
    pub root: String,
    #[serde(default)]
    pub read_only: bool,
}

impl Default for WorkspaceSpec {
    fn default() -> Self {
        Self {
            root: "/workspace".to_string(),
            read_only: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstructionsBlock {
    /// Trusted system instructions. Must never be concatenated with untrusted content
    /// by the adapter without clear delimiting/provenance metadata.
    pub system: String,
    #[serde(default)]
    pub constraints: Vec<String>,
}

/// Provenance of execution input. External content is never trusted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputProvenance {
    pub source: String,
    #[serde(default)]
    pub trusted: bool,
    #[serde(default)]
    pub subject: Option<String>,
    pub received_at: chrono::DateTime<chrono::Utc>,
}

impl InputProvenance {
    pub fn manual(user: impl Into<String>) -> Self {
        Self {
            source: format!("user:{}", user.into()),
            trusted: true,
            subject: None,
            received_at: chrono::Utc::now(),
        }
    }

    pub fn external(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            trusted: false,
            subject: None,
            received_at: chrono::Utc::now(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionInput {
    /// The single explicit run instruction for this execution (user task or delegated
    /// task). Configured agent/team system instructions are NEVER copied here: they are
    /// transported in [`ExecutionRequest::instructions`] and rendered as trusted system
    /// context by the adapter. This is the only text the adapter presents as the task.
    pub task: String,
    #[serde(default)]
    pub payload: serde_json::Value,
    pub provenance: InputProvenance,
}

/// Provenance of prompt-related execution events.
///
/// Events carry this tag in `data.promptInputKind` so the timeline can distinguish the
/// user's run instruction from configured system instructions and subagent delegation.
/// The full instruction text is not included in these events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptInputKind {
    RunInstruction,
    SystemInstructions,
    Delegation,
}

impl PromptInputKind {
    pub const DATA_KEY: &'static str = "promptInputKind";

    pub fn as_str(self) -> &'static str {
        match self {
            Self::RunInstruction => "run_instruction",
            Self::SystemInstructions => "system_instructions",
            Self::Delegation => "delegation",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRequest {
    #[serde(default)]
    pub session_ref: Option<String>,
    #[serde(default)]
    pub mode: orbit_domain::ExecutionMode,
}

impl Default for SessionRequest {
    fn default() -> Self {
        Self {
            session_ref: None,
            mode: orbit_domain::ExecutionMode::OnDemand,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionRequest {
    pub execution_id: Uuid,
    pub agent_id: Uuid,
    pub agent_version_id: Uuid,
    pub agent_name: String,
    pub instructions: InstructionsBlock,
    pub input: ExecutionInput,
    /// Resolved model/provider selection. Harness-managed harnesses may receive `None`
    /// and use their own inventory.
    #[serde(default)]
    pub model: Option<crate::provider::ResolvedModel>,
    /// Resolved models for native subagents (credentials stay in-memory).
    #[serde(default)]
    pub subagent_models: Vec<crate::provider::ResolvedModel>,
    #[serde(default)]
    pub grants: Vec<orbit_domain::GrantedCapability>,
    pub autonomy: orbit_domain::AutonomyLevel,
    pub resources: orbit_domain::ResourceLimits,
    #[serde(default)]
    pub harness_config: serde_json::Value,
    /// Extra environment variables for the harness process, resolved by the control
    /// plane (for example worker-profile connection mappings used by skill scripts).
    /// Never serialized into events or logs.
    #[serde(default, skip_serializing)]
    pub extra_env: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub session: SessionRequest,
    #[serde(default)]
    pub workspace: WorkspaceSpec,
    /// Incremental harness events. Required for streaming; adapters may ignore it when
    /// `capabilities().supports_streaming` is false.
    #[serde(skip)]
    pub events: Option<tokio::sync::mpsc::Sender<HarnessEvent>>,
    #[serde(skip, default = "default_cancellation")]
    pub cancel: tokio_util::sync::CancellationToken,
    /// Time the run spends waiting on a person (an approval) does not count toward
    /// `resources.timeout_seconds`. The platform pauses this clock while it waits;
    /// adapters enforce their timeout with [`PauseClock::deadline`].
    #[serde(skip)]
    pub pause: PauseClock,
}

fn default_cancellation() -> tokio_util::sync::CancellationToken {
    tokio_util::sync::CancellationToken::new()
}

/// Environment variable (worker and launcher) that sets the approval budget.
pub const MAX_APPROVAL_WAIT_ENV: &str = "ORBIT_MAX_APPROVAL_WAIT_SECONDS";

/// Default approval budget: the most time, in total, a run may spend paused waiting for
/// human approvals. Outer deadlines (Kubernetes Job, Docker dispatch backstop) are
/// `timeout + approval budget + grace`.
pub const DEFAULT_MAX_APPROVAL_WAIT_SECONDS: u64 = 3600;

/// Parse an approval budget value; anything unset, unparsable or zero yields the default.
pub fn parse_max_approval_wait_seconds(value: Option<&str>) -> u64 {
    value
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_MAX_APPROVAL_WAIT_SECONDS)
}

/// The approval budget from `ORBIT_MAX_APPROVAL_WAIT_SECONDS` (default 3600 s).
pub fn max_approval_wait_seconds() -> u64 {
    parse_max_approval_wait_seconds(std::env::var(MAX_APPROVAL_WAIT_ENV).ok().as_deref())
}

/// Tracks how long an execution has been paused waiting for a human decision.
///
/// Cheap to clone; all clones share one clock. Pauses may overlap (two tool calls
/// waiting at once): the clock runs again only when every pause has ended.
///
/// An optional budget caps the cumulative paused time that is excluded from the timeout:
/// once it is used up, further waiting counts as active time and callers should stop
/// granting new pauses (see [`PauseClock::remaining_budget`]). A run therefore always ends
/// within `timeout + budget`, which is what the outer (Job / container) deadlines assume.
#[derive(Debug, Clone, Default)]
pub struct PauseClock {
    inner: std::sync::Arc<std::sync::Mutex<PauseState>>,
    budget: Option<std::time::Duration>,
}

#[derive(Debug, Default)]
struct PauseState {
    /// Completed pause time.
    paused: std::time::Duration,
    /// Number of pauses currently open, and when the first of them began.
    open: u32,
    since: Option<std::time::Instant>,
}

/// Ends its pause when dropped, so an early return or error cannot leave the clock stopped.
#[derive(Debug)]
pub struct PauseGuard {
    clock: PauseClock,
}

impl Drop for PauseGuard {
    fn drop(&mut self) {
        let mut state = self.clock.inner.lock().unwrap_or_else(|e| e.into_inner());
        state.open = state.open.saturating_sub(1);
        if state.open == 0 {
            if let Some(since) = state.since.take() {
                state.paused += since.elapsed();
            }
        }
    }
}

impl PauseClock {
    /// A clock whose cumulative paused time is capped at `budget`.
    pub fn with_budget(budget: std::time::Duration) -> Self {
        Self {
            inner: Default::default(),
            budget: Some(budget),
        }
    }

    /// Paused time still available, or `None` when the clock has no budget.
    pub fn remaining_budget(&self) -> Option<std::time::Duration> {
        self.budget
            .map(|budget| budget.saturating_sub(self.total_paused()))
    }

    /// True once a budgeted clock has been paused for its whole budget.
    pub fn budget_exhausted(&self) -> bool {
        self.remaining_budget()
            .is_some_and(|remaining| remaining.is_zero())
    }

    /// Stop the run's clock until the returned guard is dropped.
    pub fn pause(&self) -> PauseGuard {
        let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if state.open == 0 {
            state.since = Some(std::time::Instant::now());
        }
        state.open += 1;
        PauseGuard {
            clock: self.clone(),
        }
    }

    pub fn is_paused(&self) -> bool {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).open > 0
    }

    /// Total time spent paused so far, including a pause still in progress.
    pub fn total_paused(&self) -> std::time::Duration {
        let state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        state.paused + state.since.map(|since| since.elapsed()).unwrap_or_default()
    }

    /// Resolves once the run has been *active* (not paused) for `timeout`. Paused time
    /// beyond the approval budget counts as active.
    ///
    /// Use in place of `tokio::time::sleep(timeout)` when enforcing an execution timeout.
    pub async fn deadline(&self, timeout: std::time::Duration) {
        let started = std::time::Instant::now();
        loop {
            let excluded = match self.budget {
                Some(budget) => self.total_paused().min(budget),
                None => self.total_paused(),
            };
            let active = started.elapsed().saturating_sub(excluded);
            if (!self.is_paused() || self.budget_exhausted()) && active >= timeout {
                return;
            }
            // While paused there is nothing to count down; poll for the pause to end.
            let wait = if self.is_paused() {
                std::time::Duration::from_millis(250)
            } else {
                timeout.saturating_sub(active).clamp(
                    std::time::Duration::from_millis(10),
                    std::time::Duration::from_secs(1),
                )
            };
            tokio::time::sleep(wait).await;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessEventKind {
    Started,
    Status,
    Log,
    ToolCall,
    OutputDelta,
    Usage,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessEvent {
    pub kind: HarnessEventKind,
    pub level: orbit_domain::LogLevel,
    pub message: String,
    #[serde(default)]
    pub data: serde_json::Value,
}

impl HarnessEvent {
    pub fn new(
        kind: HarnessEventKind,
        level: orbit_domain::LogLevel,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            level,
            message: message.into(),
            data: serde_json::Value::Null,
        }
    }

    pub fn log(level: orbit_domain::LogLevel, message: impl Into<String>) -> Self {
        Self::new(HarnessEventKind::Log, level, message)
    }

    pub fn started(session_ref: &str) -> Self {
        let mut event = Self::new(
            HarnessEventKind::Started,
            orbit_domain::LogLevel::Info,
            format!("harness session {session_ref} started"),
        );
        event.data = serde_json::json!({ "sessionRef": session_ref });
        event
    }

    pub fn output(text: impl Into<String>) -> Self {
        Self::new(
            HarnessEventKind::OutputDelta,
            orbit_domain::LogLevel::Info,
            text,
        )
    }

    /// Metadata event describing where a prompt input came from. Never include the full
    /// instruction text: system instructions are configuration, not user-visible output.
    pub fn input(kind: PromptInputKind, message: impl Into<String>) -> Self {
        Self::input_with_data(kind, message, serde_json::Value::Null)
    }

    /// Same as [`Self::input`], with additional non-sensitive metadata (counts, sources).
    pub fn input_with_data(
        kind: PromptInputKind,
        message: impl Into<String>,
        data: serde_json::Value,
    ) -> Self {
        let mut data = if data.is_object() {
            data
        } else {
            serde_json::json!({})
        };
        data[PromptInputKind::DATA_KEY] = serde_json::json!(kind.as_str());
        Self::new(
            HarnessEventKind::Log,
            orbit_domain::LogLevel::Debug,
            message,
        )
        .with_data(data)
    }

    pub fn with_data(mut self, data: serde_json::Value) -> Self {
        self.data = data;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionOutcome {
    pub status: orbit_domain::ExecutionStatus,
    #[serde(default)]
    pub output: Option<serde_json::Value>,
    #[serde(default)]
    pub error: Option<HarnessError>,
    #[serde(default)]
    pub usage: orbit_domain::Usage,
    #[serde(default)]
    pub session_ref: Option<String>,
}

impl ExecutionOutcome {
    pub fn succeeded(output: serde_json::Value, session_ref: Option<String>) -> Self {
        Self {
            status: orbit_domain::ExecutionStatus::Succeeded,
            output: Some(output),
            error: None,
            usage: default_usage(),
            session_ref,
        }
    }

    pub fn failed(error: HarnessError) -> Self {
        Self {
            status: orbit_domain::ExecutionStatus::Failed,
            output: None,
            error: Some(error),
            usage: default_usage(),
            session_ref: None,
        }
    }
}

fn default_usage() -> orbit_domain::Usage {
    orbit_domain::Usage::default()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessErrorKind {
    InvalidConfig,
    Unauthorized,
    Unavailable,
    Unsupported,
    Timeout,
    Cancelled,
    ExecutionFailed,
    Protocol,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessError {
    pub kind: HarnessErrorKind,
    pub message: String,
    #[serde(default)]
    pub retryable: bool,
    #[serde(default)]
    pub details: serde_json::Value,
}

impl HarnessError {
    pub fn new(kind: HarnessErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            retryable: false,
            details: serde_json::Value::Null,
        }
    }

    pub fn unsupported(what: impl fmt::Display) -> Self {
        Self::new(
            HarnessErrorKind::Unsupported,
            format!("harness does not support {what}"),
        )
    }

    pub fn unavailable(message: impl Into<String>) -> Self {
        let mut err = Self::new(HarnessErrorKind::Unavailable, message);
        err.retryable = true;
        err
    }

    pub fn cancelled() -> Self {
        Self::new(HarnessErrorKind::Cancelled, "execution cancelled")
    }

    pub fn timeout(seconds: u64) -> Self {
        Self::new(
            HarnessErrorKind::Timeout,
            format!("execution timed out after {seconds}s"),
        )
    }
}

impl std::fmt::Display for HarnessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for HarnessError {}

/// Adapter contract. See `docs/architecture/runtime-contract.md`.
#[async_trait::async_trait]
pub trait HarnessAdapter: Send + Sync {
    fn kind(&self) -> HarnessKind;
    fn descriptor(&self) -> HarnessDescriptor;
    fn capabilities(&self) -> HarnessCapabilities {
        self.descriptor().capabilities
    }

    /// Validate harness-specific configuration. Returns issues instead of failing, so
    /// the UI can present all problems at once.
    async fn validate_config(&self, config: &HarnessConfig) -> ValidationReport;

    async fn health(&self, config: &HarnessConfig) -> Result<HarnessHealth, HarnessError>;

    /// Run one execution to completion. Must honour cancellation and timeouts and clean
    /// up all child processes/containers before returning.
    async fn execute(&self, request: ExecutionRequest) -> Result<ExecutionOutcome, HarnessError>;
}

#[cfg(test)]
mod pause_clock_tests {
    #[tokio::test]
    async fn pause_clock_does_not_count_paused_time() {
        let clock = super::PauseClock::default();
        let started = std::time::Instant::now();
        let waiter = {
            let clock = clock.clone();
            tokio::spawn(async move { clock.deadline(std::time::Duration::from_millis(200)).await })
        };
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        {
            let _guard = clock.pause();
            let _overlapping = clock.pause();
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
            assert!(clock.is_paused());
            assert!(!waiter.is_finished(), "deadline must not fire while paused");
        }
        assert!(!clock.is_paused());
        waiter.await.unwrap();
        assert!(started.elapsed() >= std::time::Duration::from_millis(590));
        assert!(clock.total_paused() >= std::time::Duration::from_millis(390));
    }

    #[test]
    fn approval_budget_parsing_defaults_and_overrides() {
        use super::{parse_max_approval_wait_seconds as parse, DEFAULT_MAX_APPROVAL_WAIT_SECONDS};
        assert_eq!(parse(None), DEFAULT_MAX_APPROVAL_WAIT_SECONDS);
        assert_eq!(parse(Some("")), DEFAULT_MAX_APPROVAL_WAIT_SECONDS);
        assert_eq!(parse(Some("abc")), DEFAULT_MAX_APPROVAL_WAIT_SECONDS);
        assert_eq!(parse(Some("0")), DEFAULT_MAX_APPROVAL_WAIT_SECONDS);
        assert_eq!(parse(Some(" 120 ")), 120);
    }

    #[test]
    fn unbudgeted_clock_has_no_limit() {
        let clock = super::PauseClock::default();
        assert_eq!(clock.remaining_budget(), None);
        assert!(!clock.budget_exhausted());
    }

    #[tokio::test]
    async fn budget_caps_excluded_pause_time() {
        let clock = super::PauseClock::with_budget(std::time::Duration::from_millis(200));
        assert!(!clock.budget_exhausted());
        {
            let _guard = clock.pause();
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            // Still paused: the budget is gone but the deadline waits for the pause to end.
            assert!(clock.budget_exhausted());
            assert_eq!(clock.remaining_budget(), Some(std::time::Duration::ZERO));
        }
        // Exhausting the budget does not end the run by itself: a long timeout keeps running…
        assert!(tokio::time::timeout(
            std::time::Duration::from_millis(100),
            clock.deadline(std::time::Duration::from_secs(3600)),
        )
        .await
        .is_err());
        // …while paused time beyond the budget counts as active, so a pause that outlives
        // the budget lets a short timeout fire even while still paused.
        let clock = super::PauseClock::with_budget(std::time::Duration::from_millis(50));
        let _guard = clock.pause();
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            clock.deadline(std::time::Duration::from_millis(100)),
        )
        .await
        .expect("time paused beyond the budget must count toward the timeout");
    }

    #[tokio::test]
    async fn pause_clock_without_pauses_is_a_plain_timeout() {
        let clock = super::PauseClock::default();
        let started = std::time::Instant::now();
        clock.deadline(std::time::Duration::from_millis(120)).await;
        let elapsed = started.elapsed();
        assert!(
            elapsed >= std::time::Duration::from_millis(120)
                && elapsed < std::time::Duration::from_millis(600)
        );
    }
}

#[cfg(test)]
mod prompt_input_event_tests {
    use super::{HarnessEvent, PromptInputKind};

    #[test]
    fn input_events_are_tagged_without_embedding_instruction_text() {
        let configured = "AURORA-259-CONFIGURED-INSTRUCTIONS";
        let event = HarnessEvent::input(
            PromptInputKind::SystemInstructions,
            "system instructions configured (33 characters)",
        );
        assert_eq!(event.kind, super::HarnessEventKind::Log);
        assert_eq!(event.data[PromptInputKind::DATA_KEY], "system_instructions");
        assert!(!event.message.contains(configured));
        assert!(!event.data.to_string().contains(configured));

        let tagged = HarnessEvent::input_with_data(
            PromptInputKind::Delegation,
            "delegated",
            serde_json::json!({ "subagents": ["reviewer"] }),
        );
        assert_eq!(
            tagged.data[PromptInputKind::DATA_KEY],
            PromptInputKind::Delegation.as_str()
        );
        assert_eq!(tagged.data["subagents"][0], "reviewer");

        for kind in [
            PromptInputKind::RunInstruction,
            PromptInputKind::SystemInstructions,
            PromptInputKind::Delegation,
        ] {
            assert_eq!(
                HarnessEvent::input(kind, "metadata").data[PromptInputKind::DATA_KEY],
                kind.as_str()
            );
        }
    }
}
