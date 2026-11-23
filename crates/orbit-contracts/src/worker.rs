//! Contracts for worker runtime termination reasons shared by the launcher and the API.
//!
//! These values are deliberately non-secret: pod/container name, restart count, exit code
//! and termination time. Container env, command lines and logs never belong here.

use serde::{Deserialize, Serialize};

/// Why a worker container stopped while work might still have been in flight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkerTerminationReason {
    /// The kubelet or Docker reported an OOM kill (never inferred from exit code 137).
    WorkerOom,
    /// Kubernetes evicted the pod (node pressure, drain or preemption).
    WorkerEvicted,
    /// The container exited non-zero for a reason the driver could identify.
    WorkerExited,
    /// The node/pod disappeared or the cause could not be determined.
    WorkerLost,
}

impl WorkerTerminationReason {
    pub const ALL: &'static [&'static str] = &[
        "worker_oom",
        "worker_evicted",
        "worker_exited",
        "worker_lost",
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::WorkerOom => "worker_oom",
            Self::WorkerEvicted => "worker_evicted",
            Self::WorkerExited => "worker_exited",
            Self::WorkerLost => "worker_lost",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "worker_oom" => Some(Self::WorkerOom),
            "worker_evicted" => Some(Self::WorkerEvicted),
            "worker_exited" => Some(Self::WorkerExited),
            "worker_lost" => Some(Self::WorkerLost),
            _ => None,
        }
    }

    /// Audit action recorded when this reason ends work.
    pub fn audit_action(self) -> &'static str {
        match self {
            Self::WorkerOom => "worker.oom_killed",
            Self::WorkerEvicted => "worker.evicted",
            Self::WorkerExited => "worker.exited",
            Self::WorkerLost => "execution.worker_lost",
        }
    }
}

/// Structured, non-secret termination information reported by the launcher.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkerTermination {
    pub reason: WorkerTerminationReason,
    /// Pod name on Kubernetes, container name on Docker.
    pub worker_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart_count: Option<i64>,
    /// RFC 3339 time reported by the runtime, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    /// Short driver reason/message (e.g. `OOMKilled`, `Evicted`), never container env.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl WorkerTermination {
    pub fn new(reason: WorkerTerminationReason, worker_name: impl Into<String>) -> Self {
        Self {
            reason,
            worker_name: worker_name.into(),
            restart_count: None,
            terminated_at: None,
            exit_code: None,
            detail: None,
        }
    }

    /// User-facing sentence for execution/team error displays.
    pub fn message(&self) -> String {
        let identity = format!("worker/pod '{}'", self.worker_name);
        let mut message = match self.reason {
            WorkerTerminationReason::WorkerOom => {
                format!("{identity} was OOMKilled; increase the worker profile memory limit")
            }
            WorkerTerminationReason::WorkerEvicted => {
                format!("{identity} was evicted by the cluster; check node pressure or drains")
            }
            WorkerTerminationReason::WorkerExited => match self.exit_code {
                Some(code) => format!("{identity} exited with code {code}"),
                None => format!("{identity} exited unexpectedly"),
            },
            WorkerTerminationReason::WorkerLost => {
                format!("{identity} was lost; the run outcome is unknown")
            }
        };
        if let Some(detail) = self.detail.as_deref().filter(|value| !value.is_empty()) {
            message.push_str(&format!(" ({detail})"));
        }
        message
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasons_round_trip_as_stable_codes() {
        for code in WorkerTerminationReason::ALL {
            let reason = WorkerTerminationReason::parse(code).expect(code);
            assert_eq!(reason.as_str(), *code);
        }
        assert!(WorkerTerminationReason::parse("oom").is_none());
    }

    #[test]
    fn termination_json_is_camel_case_and_has_no_env() {
        let termination = WorkerTermination {
            reason: WorkerTerminationReason::WorkerOom,
            worker_name: "orbit-worker-1".into(),
            restart_count: Some(2),
            terminated_at: Some("2026-11-23T10:00:00Z".into()),
            exit_code: Some(137),
            detail: Some("OOMKilled".into()),
        };
        let value = serde_json::to_value(&termination).unwrap();
        assert_eq!(value["reason"], "worker_oom");
        assert_eq!(value["workerName"], "orbit-worker-1");
        assert_eq!(value["restartCount"], 2);
        assert_eq!(value["terminatedAt"], "2026-11-23T10:00:00Z");
        assert_eq!(value["exitCode"], 137);
        let rendered = value.to_string();
        assert!(!rendered.contains("ORBIT_WORKER_TOKEN"));
    }
}
