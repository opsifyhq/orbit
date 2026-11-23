use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::ValidationReport;

/// Execution lifecycle. Terminal states are final and immutable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    Queued,
    Dispatched,
    Running,
    AwaitingApproval,
    Succeeded,
    Failed,
    TimedOut,
    Cancelled,
    Rejected,
}

impl ExecutionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Dispatched => "dispatched",
            Self::Running => "running",
            Self::AwaitingApproval => "awaiting_approval",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::TimedOut => "timed_out",
            Self::Cancelled => "cancelled",
            Self::Rejected => "rejected",
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::TimedOut | Self::Cancelled | Self::Rejected
        )
    }

    pub fn can_transition_to(&self, next: ExecutionStatus) -> bool {
        use ExecutionStatus::*;
        if self.is_terminal() {
            return false;
        }
        match (*self, next) {
            (a, b) if a == b => true,
            (Queued, Dispatched) => true,
            (Queued, Cancelled) => true,
            (Dispatched, Running) => true,
            (Dispatched, Cancelled) => true,
            (Dispatched, Failed) => true,
            (Dispatched, TimedOut) => true,
            (Running, AwaitingApproval) => true,
            (Running, Succeeded) => true,
            (Running, Failed) => true,
            (Running, TimedOut) => true,
            (Running, Cancelled) => true,
            (Running, Rejected) => true,
            (AwaitingApproval, Running) => true,
            (AwaitingApproval, Rejected) => true,
            (AwaitingApproval, Cancelled) => true,
            (AwaitingApproval, TimedOut) => true,
            (AwaitingApproval, Failed) => true,
            _ => false,
        }
    }

    /// Every status in lifecycle order.
    pub const ALL: [ExecutionStatus; 9] = [
        Self::Queued,
        Self::Dispatched,
        Self::Running,
        Self::AwaitingApproval,
        Self::Succeeded,
        Self::Failed,
        Self::TimedOut,
        Self::Cancelled,
        Self::Rejected,
    ];

    /// Non-terminal statuses from which `next` is a legal transition (excluding `next`
    /// itself). The store uses this to reject e.g. `queued -> succeeded`.
    pub fn sources_for(next: ExecutionStatus) -> Vec<ExecutionStatus> {
        Self::ALL
            .into_iter()
            .filter(|from| *from != next && from.can_transition_to(next))
            .collect()
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "queued" => Self::Queued,
            "dispatched" => Self::Dispatched,
            "running" => Self::Running,
            "awaiting_approval" => Self::AwaitingApproval,
            "succeeded" => Self::Succeeded,
            "failed" => Self::Failed,
            "timed_out" => Self::TimedOut,
            "cancelled" => Self::Cancelled,
            "rejected" => Self::Rejected,
            _ => return None,
        })
    }
}

impl fmt::Display for ExecutionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ExecutionStatus {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value).ok_or_else(|| format!("unknown execution status: {value}"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TriggerSource {
    Manual,
    Schedule,
    Webhook,
    ConnectorEvent,
    AgentEvent,
    Api,
}

impl TriggerSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Schedule => "schedule",
            Self::Webhook => "webhook",
            Self::ConnectorEvent => "connector_event",
            Self::AgentEvent => "agent_event",
            Self::Api => "api",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "manual" => Self::Manual,
            "schedule" => Self::Schedule,
            "webhook" => Self::Webhook,
            "connector_event" => Self::ConnectorEvent,
            "agent_event" => Self::AgentEvent,
            "api" => Self::Api,
            _ => return None,
        })
    }
}

/// Requested network egress mode.
///
/// **Not yet enforced by the platform.** The value is validated and recorded on the
/// agent spec only; no ORBIT component restricts outbound traffic. Enforce egress at
/// the network layer (for example a Kubernetes NetworkPolicy).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum EgressMode {
    /// Deployment/harness default; no ORBIT-imposed restriction.
    Default,
    /// Only the listed hosts are reachable.
    Allowlist,
    /// No outbound network.
    Denied,
}

/// Desired network egress policy for an agent.
///
/// **Recorded but not yet enforced by the platform.** Neither the worker nor any
/// harness adapter reads these values; enforce egress at the network layer (for
/// example a Kubernetes NetworkPolicy).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NetworkPolicy {
    /// Requested egress mode. Recorded but not yet enforced.
    #[serde(default)]
    pub mode: Option<EgressMode>,
    /// Hostnames or `*.suffix` patterns. A bare `*` is rejected. Recorded but not yet enforced.
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
}

/// Limits enforced by the platform and honoured by harness isolation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourceLimits {
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
    #[serde(default = "default_concurrency")]
    pub max_concurrent_executions: u8,
    #[serde(default = "default_memory")]
    pub memory_mb: u32,
    #[serde(default = "default_cpu")]
    pub cpu_millis: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<NetworkPolicy>,
}

fn default_timeout() -> u64 {
    300
}
fn default_concurrency() -> u8 {
    1
}
fn default_memory() -> u32 {
    1024
}
fn default_cpu() -> u32 {
    500
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            timeout_seconds: default_timeout(),
            max_concurrent_executions: default_concurrency(),
            memory_mb: default_memory(),
            cpu_millis: default_cpu(),
            max_output_tokens: None,
            network: None,
        }
    }
}

impl ResourceLimits {
    pub fn validate(&self) -> ValidationReport {
        let mut report = ValidationReport::default();
        if !(1..=86_400).contains(&self.timeout_seconds) {
            report.push("spec.resources.timeoutSeconds", "must be 1..=86400");
        }
        if !(1..=20).contains(&self.max_concurrent_executions) {
            report.push("spec.resources.maxConcurrentExecutions", "must be 1..=20");
        }
        if !(64..=65_536).contains(&self.memory_mb) {
            report.push("spec.resources.memoryMb", "must be 64..=65536");
        }
        if !(100..=64_000).contains(&self.cpu_millis) {
            report.push("spec.resources.cpuMillis", "must be 100..=64000");
        }
        if let Some(tokens) = self.max_output_tokens {
            if tokens == 0 || tokens > 1_000_000 {
                report.push("spec.resources.maxOutputTokens", "must be 1..=1000000");
            }
        }
        if let Some(network) = &self.network {
            if network.mode == Some(EgressMode::Allowlist) && network.allowed_hosts.is_empty() {
                report.push(
                    "spec.resources.network.allowedHosts",
                    "allowlist mode requires at least one host",
                );
            }
            for host in &network.allowed_hosts {
                if let Some(message) = invalid_host_reason(host) {
                    report.push("spec.resources.network.allowedHosts", message);
                }
            }
        }
        report
    }
}

/// Why an egress host pattern is invalid, or `None` when acceptable.
/// Accepts `example.com`, `api.example.com` or `*.example.com`; never a bare `*`.
fn invalid_host_reason(host: &str) -> Option<String> {
    let trimmed = host.trim();
    if trimmed.is_empty() {
        return Some("host must not be empty or whitespace".into());
    }
    if trimmed != host {
        return Some(format!(
            "host must not have surrounding whitespace: '{host}'"
        ));
    }
    if host == "*" {
        return Some("'*' is not allowed; list explicit hosts or a '*.domain' pattern".into());
    }
    if host.len() > 253 {
        return Some(format!("host is too long: {host}"));
    }
    let body = host.strip_prefix("*.").unwrap_or(host);
    let labels_ok = body.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    });
    if !labels_ok {
        return Some(format!("invalid host pattern: {host}"));
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionEventKind {
    Status,
    Log,
    ToolCall,
    Output,
    Error,
    Metric,
}

impl ExecutionEventKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Log => "log",
            Self::ToolCall => "tool_call",
            Self::Output => "output",
            Self::Error => "error",
            Self::Metric => "metric",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "status" => Self::Status,
            "log" => Self::Log,
            "tool_call" => Self::ToolCall,
            "output" => Self::Output,
            "error" => Self::Error,
            "metric" => Self::Metric,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_millis: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_time_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionError {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub retryable: bool,
    #[serde(default)]
    pub details: serde_json::Value,
}

impl ExecutionError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable: false,
            details: serde_json::Value::Null,
        }
    }
}

/// Durable execution record as stored by the control plane.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Execution {
    pub id: Uuid,
    pub org_id: Uuid,
    pub agent_id: Uuid,
    pub agent_version_id: Uuid,
    pub agent_name: String,
    pub harness_kind: crate::HarnessKind,
    pub status: ExecutionStatus,
    pub trigger_source: TriggerSource,
    pub trigger_id: Option<Uuid>,
    pub session_ref: Option<String>,
    pub input: serde_json::Value,
    pub output: Option<serde_json::Value>,
    pub error: Option<ExecutionError>,
    #[serde(default)]
    pub usage: Usage,
    pub timeout_seconds: u64,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

impl Execution {
    pub fn is_terminal(&self) -> bool {
        self.status.is_terminal()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_states_are_final() {
        for status in [
            ExecutionStatus::Succeeded,
            ExecutionStatus::Failed,
            ExecutionStatus::TimedOut,
            ExecutionStatus::Cancelled,
            ExecutionStatus::Rejected,
        ] {
            assert!(status.is_terminal());
            assert!(!status.can_transition_to(ExecutionStatus::Running));
        }
    }

    #[test]
    fn happy_path_transitions() {
        use ExecutionStatus::*;
        assert!(Queued.can_transition_to(Dispatched));
        assert!(Dispatched.can_transition_to(Running));
        assert!(Running.can_transition_to(Succeeded));
        assert!(Running.can_transition_to(AwaitingApproval));
        assert!(AwaitingApproval.can_transition_to(Running));
        assert!(!Queued.can_transition_to(Running));
        assert!(!Succeeded.can_transition_to(Failed));
        assert!(!Queued.can_transition_to(Succeeded));
        assert!(!Dispatched.can_transition_to(Succeeded));
    }

    #[test]
    fn terminal_sources_follow_the_table() {
        use ExecutionStatus::*;
        assert_eq!(ExecutionStatus::sources_for(Succeeded), vec![Running]);
        assert_eq!(
            ExecutionStatus::sources_for(Cancelled),
            vec![Queued, Dispatched, Running, AwaitingApproval]
        );
        assert!(!ExecutionStatus::sources_for(Failed).contains(&Queued));
        for terminal in [Succeeded, Failed, TimedOut, Cancelled, Rejected] {
            assert!(!ExecutionStatus::sources_for(Failed).contains(&terminal));
        }
    }

    #[test]
    fn resource_bounds() {
        let mut limits = ResourceLimits::default();
        assert!(limits.validate().is_valid());
        limits.timeout_seconds = 0;
        assert!(!limits.validate().is_valid());
        limits = ResourceLimits::default();
        limits.network = Some(NetworkPolicy {
            mode: Some(EgressMode::Allowlist),
            allowed_hosts: vec![],
        });
        assert!(!limits.validate().is_valid());
    }

    fn network_report(mode: Option<EgressMode>, hosts: &[&str]) -> ValidationReport {
        let limits = ResourceLimits {
            network: Some(NetworkPolicy {
                mode,
                allowed_hosts: hosts.iter().map(|h| h.to_string()).collect(),
            }),
            ..ResourceLimits::default()
        };
        limits.validate()
    }

    #[test]
    fn egress_hosts_reject_wildcard_empty_and_invalid() {
        for bad in [
            "*",
            "",
            "   ",
            " example.com",
            "exa mple.com",
            "-a.com",
            "a..com",
            "*.",
            "a.*.com",
            "**.com",
            "http://x.com",
        ] {
            let report = network_report(Some(EgressMode::Allowlist), &[bad]);
            assert!(!report.is_valid(), "expected '{bad}' to be rejected");
        }
    }

    #[test]
    fn egress_hosts_accept_hostnames_and_suffix_patterns() {
        let report = network_report(
            Some(EgressMode::Allowlist),
            &[
                "example.com",
                "api.example.com",
                "*.example.com",
                "localhost",
            ],
        );
        assert!(report.is_valid());
    }
}
