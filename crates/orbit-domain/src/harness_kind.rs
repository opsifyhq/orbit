use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Agent harness families ORBIT can orchestrate.
///
/// The enumeration is intentionally small and lives in the domain so that the API and
/// UI can reason about harnesses without depending on adapter crates. New harnesses can be
/// added here plus a registered `HarnessAdapter`; no other domain changes are required.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum HarnessKind {
    /// In-process deterministic harness used for development and contract tests.
    Mock,
    #[serde(rename = "opencode")]
    OpenCode,
    ClaudeCode,
    Pi,
    /// Custom worker that speaks the ORBIT harness protocol.
    Generic,
}

impl HarnessKind {
    pub const ALL: [HarnessKind; 5] = [
        Self::Mock,
        Self::OpenCode,
        Self::ClaudeCode,
        Self::Pi,
        Self::Generic,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Mock => "mock",
            Self::OpenCode => "opencode",
            Self::ClaudeCode => "claude_code",
            Self::Pi => "pi",
            Self::Generic => "generic",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Mock => "Mock harness",
            Self::OpenCode => "OpenCode",
            Self::ClaudeCode => "Claude Code",
            Self::Pi => "Pi",
            Self::Generic => "Generic harness",
        }
    }

    /// Whether the adapter is functional in this build. The UI surfaces this so users
    /// are never misled into believing an unimplemented integration works.
    pub fn implementation_status(&self) -> ImplementationStatus {
        match self {
            Self::Mock => ImplementationStatus::Available,
            Self::OpenCode => ImplementationStatus::Scaffolded,
            Self::ClaudeCode | Self::Pi | Self::Generic => ImplementationStatus::Planned,
        }
    }
}

impl fmt::Display for HarnessKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for HarnessKind {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "mock" => Ok(Self::Mock),
            "opencode" => Ok(Self::OpenCode),
            "claude_code" | "claude-code" | "claudecode" => Ok(Self::ClaudeCode),
            "pi" => Ok(Self::Pi),
            "generic" => Ok(Self::Generic),
            other => Err(format!("unknown harness kind: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImplementationStatus {
    /// Implemented and exercised by contract tests.
    Available,
    /// Contract implemented, real execution not yet certified against the harness.
    Scaffolded,
    /// Registered descriptor only.
    Planned,
}

impl ImplementationStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Scaffolded => "scaffolded",
            Self::Planned => "planned",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    /// A harness is created per execution (containers/Jobs).
    #[default]
    OnDemand,
    /// A harness instance stays alive and receives multiple executions.
    LongRunning,
}

impl ExecutionMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::OnDemand => "on_demand",
            Self::LongRunning => "long_running",
        }
    }
}

impl FromStr for ExecutionMode {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "on_demand" | "on-demand" => Ok(Self::OnDemand),
            "long_running" | "long-running" => Ok(Self::LongRunning),
            other => Err(format!("unknown execution mode: {other}")),
        }
    }
}

/// Where an agent is allowed to execute. This is a routing constraint, not a guarantee
/// of process isolation; isolation depends on the worker's execution driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlacementTarget {
    /// Any worker that supports the selected harness.
    #[default]
    Auto,
    /// A worker running natively on the host.
    Local,
    /// A Docker-based worker (Compose or host container dispatcher).
    Docker,
    /// A Kubernetes worker.
    Kubernetes,
}

impl PlacementTarget {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Local => "local",
            Self::Docker => "docker",
            Self::Kubernetes => "kubernetes",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "auto" => Self::Auto,
            "local" => Self::Local,
            "docker" => Self::Docker,
            "kubernetes" => Self::Kubernetes,
            _ => return None,
        })
    }
}
