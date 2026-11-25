//! Pure launch-status mapping over parsed `kubectl get -o json` output and `docker inspect`
//! state. No I/O lives here so every rule can be unit-tested with fixtures.
//!
//! Rules (see `docs/development/kubernetes-launcher.md`):
//! - Status is derived from the CURRENT container state. A past termination
//!   (`lastState`) is only reported as a hint unless the container is not running now.
//! - A container is "OOM" only when the kubelet says `reason == "OOMKilled"`. Exit code 137
//!   alone is SIGKILL (cancel, eviction, `kubectl delete`) and is reported as such.
//! - Waiting reasons that Kubernetes retries forever (CrashLoopBackOff, ImagePullBackOff, ...)
//!   are reported as `failed` with a message instead of staying `starting`. When the
//!   container never ran (no `lastState`), the cause is `worker_did_not_start`, which keeps
//!   the run's retry; only a container that ran and then stopped ends work without retry.
//! - In a Deployment, a terminal pod (evicted or otherwise `Failed`) does not decide the
//!   status while a replacement pod is still live.

use orbit_contracts::{WorkerTermination, WorkerTerminationReason};
use serde_json::Value;

/// Result of mapping workload state to an ORBIT launch status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mapped {
    pub status: &'static str,
    pub error: Option<String>,
    /// Structured, non-secret cause for a terminal failure.
    pub termination: Option<WorkerTermination>,
}

impl Mapped {
    fn new(status: &'static str, error: Option<String>) -> Self {
        Self {
            status,
            error,
            termination: None,
        }
    }

    fn failed(error: String, termination: WorkerTermination) -> Self {
        Self {
            status: "failed",
            error: Some(error),
            termination: Some(termination),
        }
    }
}

/// Waiting reasons that will not resolve without intervention.
const FATAL_WAITING: &[&str] = &[
    "CrashLoopBackOff",
    "ImagePullBackOff",
    "ErrImagePull",
    "InvalidImageName",
    "CreateContainerConfigError",
    "CreateContainerError",
    "RunContainerError",
];

fn str_at<'a>(value: &'a Value, pointer: &str) -> Option<&'a str> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

fn memory_limit(pod: &Value) -> &str {
    str_at(pod, "/spec/containers/0/resources/limits/memory").unwrap_or("unknown")
}

fn container_statuses(pod: &Value) -> Vec<&Value> {
    ["/status/containerStatuses", "/status/initContainerStatuses"]
        .into_iter()
        .flat_map(|pointer| {
            pod.pointer(pointer)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .collect()
}

fn oom_message(pod: &Value) -> String {
    format!(
        "worker container was OOMKilled (memory limit {}); increase the worker profile memory limit",
        memory_limit(pod)
    )
}

fn is_oom(terminated: &Value) -> bool {
    terminated.get("reason").and_then(Value::as_str) == Some("OOMKilled")
}

fn describe_termination(terminated: &Value) -> String {
    let reason = str_at(terminated, "/reason").unwrap_or("Error");
    match terminated.get("exitCode").and_then(Value::as_i64) {
        Some(137) => format!(
            "worker container was killed (exit code 137 / SIGKILL, reason {reason}); this is not an out-of-memory kill"
        ),
        Some(code) => format!("worker container exited with code {code} ({reason})"),
        None => format!("worker container terminated ({reason})"),
    }
}

fn termination_message(pod: &Value, terminated: &Value) -> String {
    if is_oom(terminated) {
        oom_message(pod)
    } else {
        describe_termination(terminated)
    }
}

fn restarts(status: &Value) -> i64 {
    status
        .get("restartCount")
        .and_then(Value::as_i64)
        .unwrap_or(0)
}

fn pod_name(pod: &Value) -> String {
    str_at(pod, "/metadata/name")
        .unwrap_or("unknown-worker")
        .to_string()
}

fn pod_status_reason(pod: &Value) -> Option<&str> {
    str_at(pod, "/status/reason")
}

fn is_evicted(pod: &Value, terminated: Option<&Value>) -> bool {
    pod_status_reason(pod) == Some("Evicted")
        || terminated.is_some_and(|value| str_at(value, "/reason") == Some("Evicted"))
}

/// A pod that will never run again: phase `Failed` or `Succeeded`.
fn is_terminal_pod(pod: &Value) -> bool {
    matches!(str_at(pod, "/status/phase"), Some("Failed" | "Succeeded"))
}

fn is_node_loss(pod: &Value) -> bool {
    str_at(pod, "/status/phase") == Some("Unknown")
        || matches!(
            pod_status_reason(pod),
            Some("NodeLost" | "NodeNotFound" | "NodeUnreachable" | "NodeAffinity")
        )
}

fn terminated_at(terminated: &Value) -> Option<String> {
    str_at(terminated, "/finishedAt").map(str::to_string)
}

fn termination(
    reason: WorkerTerminationReason,
    name: impl Into<String>,
    restarts: Option<i64>,
    terminated: Option<&Value>,
    detail: Option<String>,
) -> WorkerTermination {
    WorkerTermination {
        reason,
        worker_name: name.into(),
        restart_count: restarts,
        terminated_at: terminated.and_then(terminated_at),
        exit_code: terminated.and_then(|value| value.get("exitCode").and_then(Value::as_i64)),
        detail,
    }
}

/// Pick the most specific reason available from a terminated container state.
fn container_termination(pod: &Value, status: &Value, terminated: &Value) -> WorkerTermination {
    let reason = if is_oom(terminated) {
        WorkerTerminationReason::WorkerOom
    } else if is_evicted(pod, Some(terminated)) {
        WorkerTerminationReason::WorkerEvicted
    } else {
        WorkerTerminationReason::WorkerExited
    };
    termination(
        reason,
        pod_name(pod),
        Some(restarts(status)),
        Some(terminated),
        str_at(terminated, "/reason").map(str::to_string),
    )
}

/// Map the current state of one pod's containers. `restart_always` is true for Deployment
/// pods, where a terminated container is about to be restarted.
fn map_pod(pod: &Value, restart_always: bool) -> Mapped {
    let statuses = container_statuses(pod);
    // Pod-level eviction/node loss can arrive with no usable container state at all.
    if is_evicted(pod, None) {
        return Mapped::failed(
            format!(
                "worker pod was evicted ({})",
                str_at(pod, "/status/message").unwrap_or("node pressure or drain")
            ),
            termination(
                WorkerTerminationReason::WorkerEvicted,
                pod_name(pod),
                None,
                None,
                Some("Evicted".into()),
            ),
        );
    }
    if is_node_loss(pod) {
        return Mapped::failed(
            format!(
                "worker pod was lost ({})",
                pod_status_reason(pod).unwrap_or("node or pod disappeared")
            ),
            termination(
                WorkerTerminationReason::WorkerLost,
                pod_name(pod),
                None,
                None,
                pod_status_reason(pod).map(str::to_string),
            ),
        );
    }
    let mut starting = statuses.is_empty();
    let mut hint: Option<String> = None;
    for status in &statuses {
        if hint.is_none() && status.pointer("/lastState/terminated").is_some_and(is_oom) {
            hint = Some(format!(
                "worker container was OOMKilled earlier (restarts: {}, memory limit {}); consider raising the memory limit",
                restarts(status),
                memory_limit(pod)
            ));
        }
        if let Some(waiting) = status.pointer("/state/waiting") {
            let reason = str_at(waiting, "/reason").unwrap_or("");
            if FATAL_WAITING.contains(&reason) {
                let mut message = format!(
                    "worker container is {reason} (restarts: {})",
                    restarts(status)
                );
                let last = status.pointer("/lastState/terminated");
                if let Some(last) = last {
                    message = format!(
                        "{message}; last termination: {}",
                        termination_message(pod, last)
                    );
                } else if let Some(detail) = str_at(waiting, "/message") {
                    message = format!("{message}: {detail}");
                }
                // With no earlier termination the container never ran (image pull or
                // config error); CrashLoopBackOff always means it ran and stopped.
                let never_ran_reason = if reason == "CrashLoopBackOff" {
                    WorkerTerminationReason::WorkerExited
                } else {
                    WorkerTerminationReason::WorkerDidNotStart
                };
                let termination = match last {
                    Some(last) => container_termination(pod, status, last),
                    None => termination(
                        never_ran_reason,
                        pod_name(pod),
                        Some(restarts(status)),
                        None,
                        Some(reason.to_string()),
                    ),
                };
                return Mapped::failed(message, termination);
            }
            starting = true;
        } else if let Some(terminated) = status.pointer("/state/terminated") {
            if is_oom(terminated) {
                return Mapped::failed(
                    oom_message(pod),
                    container_termination(pod, status, terminated),
                );
            }
            if is_evicted(pod, Some(terminated)) {
                return Mapped::failed(
                    format!(
                        "worker container was evicted ({})",
                        str_at(terminated, "/message").unwrap_or("Evicted")
                    ),
                    container_termination(pod, status, terminated),
                );
            }
            if restart_always {
                // Kubernetes is restarting it; this is a transient state.
                starting = true;
            } else if terminated.get("exitCode").and_then(Value::as_i64) == Some(0) {
                return Mapped::new("stopped", None);
            } else {
                return Mapped::failed(
                    describe_termination(terminated),
                    container_termination(pod, status, terminated),
                );
            }
        } else if status.pointer("/state/running").is_none() {
            starting = true;
        }
    }
    if starting {
        Mapped::new("starting", hint)
    } else {
        Mapped::new("running", hint)
    }
}

fn kind(item: &Value) -> &str {
    item.get("kind").and_then(Value::as_str).unwrap_or("")
}

fn job_condition<'a>(job: &'a Value, condition: &str) -> Option<&'a Value> {
    job.pointer("/status/conditions")
        .and_then(Value::as_array)?
        .iter()
        .find(|c| {
            c.get("type").and_then(Value::as_str) == Some(condition)
                && c.get("status").and_then(Value::as_str) == Some("True")
        })
}

fn job_count(job: &Value, field: &str) -> i64 {
    job.pointer(&format!("/status/{field}"))
        .and_then(Value::as_i64)
        .unwrap_or(0)
}

fn map_job(job: &Value, pods: &[&Value]) -> Mapped {
    if job_condition(job, "Complete").is_some() || job_count(job, "succeeded") > 0 {
        return Mapped::new("stopped", None);
    }
    let failed_condition = job_condition(job, "Failed");
    if failed_condition.is_some() || job_count(job, "failed") > 0 {
        let mut parts: Vec<String> = Vec::new();
        match failed_condition {
            Some(condition) => {
                let reason = str_at(condition, "/reason").unwrap_or("Failed");
                match str_at(condition, "/message") {
                    Some(message) => parts.push(format!("job failed ({reason}): {message}")),
                    None => parts.push(format!("job failed ({reason})")),
                }
            }
            None => parts.push("job failed".into()),
        }
        // Add the most specific container-level cause while the pod is still around.
        let mut cause: Option<WorkerTermination> = None;
        for pod in pods {
            for status in container_statuses(pod) {
                if let Some(terminated) = status.pointer("/state/terminated") {
                    parts.push(termination_message(pod, terminated));
                    if cause.is_none() {
                        cause = Some(container_termination(pod, status, terminated));
                    }
                }
            }
        }
        let condition_reason = failed_condition
            .and_then(|condition| str_at(condition, "/reason"))
            .unwrap_or("Failed");
        let cause = cause.unwrap_or_else(|| {
            let reason = match condition_reason {
                "Evicted" => WorkerTerminationReason::WorkerEvicted,
                "DeadlineExceeded" | "BackoffLimitExceeded" => {
                    WorkerTerminationReason::WorkerExited
                }
                _ => WorkerTerminationReason::WorkerLost,
            };
            termination(
                reason,
                pods.first().map(|pod| pod_name(pod)).unwrap_or_else(|| {
                    str_at(job, "/metadata/name")
                        .unwrap_or("unknown-worker")
                        .to_string()
                }),
                None,
                None,
                Some(condition_reason.to_string()),
            )
        });
        return Mapped::failed(parts.join("; "), cause);
    }
    // Still running or starting: a stuck pod (image pull, crash) is a failure.
    let mut best = Mapped::new("starting", None);
    for pod in pods {
        let mapped = map_pod(pod, false);
        match mapped.status {
            "failed" => return mapped,
            "running" | "stopped" => best = mapped,
            _ => {}
        }
    }
    if best.status == "starting" && pods.is_empty() && job_count(job, "active") > 0 {
        return Mapped::new("running", None);
    }
    best
}

/// Map the items of `kubectl get deployments,jobs,pods -l orbit.launch-id=<id> -o json`.
///
/// An empty list means the workload no longer exists (stopped, or a finished Job whose TTL
/// expired). The API persists a terminal status before that happens and only refreshes
/// non-terminal launches, so `stopped` for "gone" never overwrites a stored `failed`.
pub fn map_kubernetes(items: &[Value]) -> Mapped {
    let has_deployment = items.iter().any(|i| kind(i) == "Deployment");
    let job = items.iter().find(|i| kind(i) == "Job");
    let pods: Vec<&Value> = items.iter().filter(|i| kind(i) == "Pod").collect();
    if let Some(job) = job {
        return map_job(job, &pods);
    }
    if has_deployment {
        // Ignore pods that are being replaced.
        let current: Vec<&Value> = pods
            .iter()
            .copied()
            .filter(|p| p.pointer("/metadata/deletionTimestamp").is_none())
            .collect();
        // A terminal pod (an evicted pod stays listed as `Failed`) does not decide the
        // status while the ReplicaSet's replacement is live.
        let replacement_live = current.iter().any(|pod| !is_terminal_pod(pod));
        let mut best = Mapped::new("starting", None);
        for pod in current
            .into_iter()
            .filter(|pod| !replacement_live || !is_terminal_pod(pod))
        {
            let mapped = map_pod(pod, true);
            match mapped.status {
                "failed" => return mapped,
                "running" => best = mapped,
                _ if best.status != "running" => best = mapped,
                _ => {}
            }
        }
        return best;
    }
    if let Some(pod) = pods.first() {
        // Legacy bare Pod launches.
        return match str_at(pod, "/status/phase") {
            Some("Succeeded") => Mapped::new("stopped", None),
            Some("Failed") => {
                let mapped = map_pod(pod, false);
                if mapped.status == "failed" {
                    return mapped;
                }
                // Eviction is identifiable; anything else that ends a bare pod is a loss.
                let reason = if is_evicted(pod, None) {
                    WorkerTerminationReason::WorkerEvicted
                } else {
                    WorkerTerminationReason::WorkerLost
                };
                let message = str_at(pod, "/status/message")
                    .map(str::to_string)
                    .unwrap_or_else(|| "pod failed".into());
                Mapped::failed(
                    message,
                    termination(
                        reason,
                        pod_name(pod),
                        None,
                        None,
                        pod_status_reason(pod).map(str::to_string),
                    ),
                )
            }
            _ => map_pod(pod, false),
        };
    }
    Mapped::new("stopped", None)
}

/// Parsed `docker inspect` state for one worker container.
pub struct DockerState<'a> {
    pub status: &'a str,
    pub oom_killed: bool,
    pub exit_code: i64,
    pub error: &'a str,
    pub name: &'a str,
    pub restart_count: Option<i64>,
    pub finished_at: Option<&'a str>,
}

/// Map `docker inspect` state. `oom_killed` comes from `State.OOMKilled`, never from the
/// exit code.
pub fn map_docker(state: DockerState<'_>) -> Mapped {
    let docker_termination =
        |reason: WorkerTerminationReason, detail: Option<String>| WorkerTermination {
            reason,
            worker_name: state.name.to_string(),
            restart_count: state.restart_count,
            terminated_at: state.finished_at.map(str::to_string),
            exit_code: Some(state.exit_code),
            detail,
        };
    if state.oom_killed {
        return Mapped::failed(
            "worker container was OOMKilled (Docker memory limit); increase the worker profile memory limit"
                .into(),
            docker_termination(WorkerTerminationReason::WorkerOom, Some("OOMKilled".into())),
        );
    }
    let error = (!state.error.is_empty()).then(|| state.error.to_string());
    match state.status {
        "running" => Mapped::new("running", error),
        "created" | "restarting" => Mapped::new("starting", error),
        "exited" if state.exit_code == 0 => Mapped::new("stopped", error),
        "exited" => Mapped::failed(
            error.unwrap_or_else(|| {
                if state.exit_code == 137 {
                    "worker container was killed (exit code 137 / SIGKILL); not an out-of-memory kill"
                        .into()
                } else {
                    format!("worker container exited with code {}", state.exit_code)
                }
            }),
            docker_termination(WorkerTerminationReason::WorkerExited, None),
        ),
        "removing" => Mapped::new("stopped", error),
        "dead" | "paused" => Mapped::failed(
            error.unwrap_or_else(|| format!("worker container is {}", state.status)),
            docker_termination(WorkerTerminationReason::WorkerLost, None),
        ),
        _ => Mapped::new("starting", error),
    }
}

/// `activeDeadlineSeconds` for a single-execution Job: the execution timeout, plus the
/// approval budget (the run's own clock pauses while a person decides, so the wall-clock
/// kill deadline must not), plus a grace window for scheduling, image pull and teardown.
/// Falls back to `default_seconds` when the request has no timeout.
pub fn job_deadline_seconds(
    timeout_seconds: Option<u64>,
    approval_budget_seconds: u64,
    default_seconds: u64,
) -> u64 {
    const GRACE: u64 = 300;
    match timeout_seconds {
        Some(t) if t > 0 => t
            .saturating_add(approval_budget_seconds)
            .saturating_add(GRACE),
        _ => default_seconds.max(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pod(container_status: Value) -> Value {
        json!({
            "kind": "Pod",
            "metadata": { "name": "orbit-worker-1234" },
            "spec": { "containers": [{ "resources": { "limits": { "memory": "1Gi" } } }] },
            "status": { "phase": "Running", "containerStatuses": [container_status] }
        })
    }

    fn deployment() -> Value {
        json!({ "kind": "Deployment", "metadata": { "name": "orbit-worker-1234" } })
    }

    fn job(status: Value) -> Value {
        json!({ "kind": "Job", "metadata": { "name": "orbit-worker-1234" }, "status": status })
    }

    #[test]
    fn oomkilled_current_state_is_failed_oom() {
        let items = [
            deployment(),
            pod(json!({
                "restartCount": 0,
                "state": { "terminated": { "reason": "OOMKilled", "exitCode": 137 } }
            })),
        ];
        let mapped = map_kubernetes(&items);
        assert_eq!(mapped.status, "failed");
        let error = mapped.error.unwrap();
        assert!(
            error.contains("OOMKilled") && error.contains("1Gi"),
            "{error}"
        );
        let termination = mapped.termination.unwrap();
        assert_eq!(termination.reason, WorkerTerminationReason::WorkerOom);
        assert_eq!(termination.worker_name, "orbit-worker-1234");
        assert_eq!(termination.restart_count, Some(0));
        assert_eq!(termination.exit_code, Some(137));
    }

    #[test]
    fn sigkill_137_without_oom_reason_is_not_oom() {
        let items = [
            job(json!({ "failed": 1 })),
            pod(json!({
                "state": { "terminated": { "reason": "Error", "exitCode": 137 } }
            })),
        ];
        let mapped = map_kubernetes(&items);
        assert_eq!(mapped.status, "failed");
        let error = mapped.error.unwrap();
        assert!(
            error.contains("SIGKILL") && error.contains("not an out-of-memory"),
            "{error}"
        );
        assert!(!error.contains("increase the worker profile memory limit"));
        // The API keys OOM handling off the word "oom": make sure "not an out-of-memory"
        // does not trip it.
        assert!(!error.to_lowercase().contains("oom"), "{error}");
    }

    #[test]
    fn restarted_after_past_oom_and_now_running_is_running() {
        let items = [
            deployment(),
            pod(json!({
                "ready": true,
                "restartCount": 2,
                "state": { "running": { "startedAt": "2026-11-19T10:00:00Z" } },
                "lastState": { "terminated": { "reason": "OOMKilled", "exitCode": 137 } }
            })),
        ];
        let mapped = map_kubernetes(&items);
        assert_eq!(mapped.status, "running");
        assert!(mapped.error.unwrap().contains("earlier"));
    }

    #[test]
    fn crash_loop_backoff_is_failed_with_restart_count() {
        let items = [
            deployment(),
            pod(json!({
                "restartCount": 5,
                "state": { "waiting": { "reason": "CrashLoopBackOff" } },
                "lastState": { "terminated": { "reason": "Error", "exitCode": 1 } }
            })),
        ];
        let mapped = map_kubernetes(&items);
        assert_eq!(mapped.status, "failed");
        let error = mapped.error.unwrap();
        assert!(
            error.contains("CrashLoopBackOff") && error.contains("restarts: 5"),
            "{error}"
        );
        assert!(error.contains("code 1"), "{error}");
    }

    #[test]
    fn crash_loop_after_oom_keeps_oom_message() {
        let items = [
            deployment(),
            pod(json!({
                "restartCount": 3,
                "state": { "waiting": { "reason": "CrashLoopBackOff" } },
                "lastState": { "terminated": { "reason": "OOMKilled", "exitCode": 137 } }
            })),
        ];
        let mapped = map_kubernetes(&items);
        assert_eq!(mapped.status, "failed");
        assert!(mapped.error.unwrap().contains("OOMKilled"));
    }

    #[test]
    fn image_pull_backoff_is_failed() {
        let items = [
            deployment(),
            pod(json!({
                "restartCount": 0,
                "state": { "waiting": {
                    "reason": "ImagePullBackOff",
                    "message": "Back-off pulling image \"registry/x:1\""
                } }
            })),
        ];
        let mapped = map_kubernetes(&items);
        assert_eq!(mapped.status, "failed");
        assert!(mapped.error.unwrap().contains("ImagePullBackOff"));
    }

    #[test]
    fn waiting_errors_without_last_state_did_not_start() {
        for reason in [
            "ImagePullBackOff",
            "ErrImagePull",
            "InvalidImageName",
            "CreateContainerConfigError",
            "CreateContainerError",
            "RunContainerError",
        ] {
            let items = [
                deployment(),
                pod(json!({ "restartCount": 0, "state": { "waiting": { "reason": reason } } })),
            ];
            let mapped = map_kubernetes(&items);
            assert_eq!(mapped.status, "failed", "{reason}");
            let termination = mapped.termination.unwrap();
            assert_eq!(
                termination.reason,
                WorkerTerminationReason::WorkerDidNotStart,
                "{reason}"
            );
            assert!(!termination.reason.ends_without_retry(), "{reason}");
            assert_eq!(termination.detail.as_deref(), Some(reason));
        }
    }

    #[test]
    fn crash_loop_and_waiting_after_a_run_end_without_retry() {
        let crash = [
            deployment(),
            pod(
                json!({ "restartCount": 3, "state": { "waiting": { "reason": "CrashLoopBackOff" } } }),
            ),
        ];
        let termination = map_kubernetes(&crash).termination.unwrap();
        assert_eq!(termination.reason, WorkerTerminationReason::WorkerExited);
        assert!(termination.reason.ends_without_retry());

        // An image pull error after the container already ran keeps the earlier cause.
        let after_run = [
            deployment(),
            pod(json!({
                "restartCount": 1,
                "state": { "waiting": { "reason": "ImagePullBackOff" } },
                "lastState": { "terminated": { "reason": "Error", "exitCode": 1 } }
            })),
        ];
        let termination = map_kubernetes(&after_run).termination.unwrap();
        assert_eq!(termination.reason, WorkerTerminationReason::WorkerExited);
    }

    fn evicted_deployment_pod() -> Value {
        let mut pod = pod(json!({
            "state": { "terminated": { "reason": "Evicted", "exitCode": 137 } }
        }));
        pod["metadata"]["name"] = json!("orbit-worker-1234-old");
        pod["status"]["phase"] = json!("Failed");
        pod["status"]["reason"] = json!("Evicted");
        pod
    }

    #[test]
    fn evicted_deployment_pod_with_a_running_replacement_is_running() {
        let replacement = pod(json!({ "state": { "running": {} } }));
        for items in [
            [deployment(), evicted_deployment_pod(), replacement.clone()],
            [deployment(), replacement.clone(), evicted_deployment_pod()],
        ] {
            let mapped = map_kubernetes(&items);
            assert_eq!(mapped.status, "running");
            assert!(mapped.termination.is_none());
        }
        // A replacement that is still being created keeps the launch starting.
        let creating = pod(json!({ "state": { "waiting": { "reason": "ContainerCreating" } } }));
        let mut pending = creating;
        pending["status"]["phase"] = json!("Pending");
        assert_eq!(
            map_kubernetes(&[deployment(), evicted_deployment_pod(), pending]).status,
            "starting"
        );
    }

    #[test]
    fn evicted_deployment_pod_alone_is_failed_evicted() {
        let mapped = map_kubernetes(&[deployment(), evicted_deployment_pod()]);
        assert_eq!(mapped.status, "failed");
        assert_eq!(
            mapped.termination.unwrap().reason,
            WorkerTerminationReason::WorkerEvicted
        );
    }

    #[test]
    fn a_live_replacement_that_fails_still_fails_the_launch() {
        let crash = pod(
            json!({ "restartCount": 2, "state": { "waiting": { "reason": "CrashLoopBackOff" } } }),
        );
        let mapped = map_kubernetes(&[deployment(), evicted_deployment_pod(), crash]);
        assert_eq!(mapped.status, "failed");
        assert_eq!(
            mapped.termination.unwrap().reason,
            WorkerTerminationReason::WorkerExited
        );
    }

    #[test]
    fn container_creating_is_starting() {
        let items = [
            deployment(),
            pod(json!({ "state": { "waiting": { "reason": "ContainerCreating" } } })),
        ];
        assert_eq!(map_kubernetes(&items).status, "starting");
    }

    #[test]
    fn job_failed_carries_condition_message() {
        let items = [job(json!({
            "failed": 1,
            "conditions": [{
                "type": "Failed", "status": "True",
                "reason": "DeadlineExceeded",
                "message": "Job was active longer than specified deadline"
            }]
        }))];
        let mapped = map_kubernetes(&items);
        assert_eq!(mapped.status, "failed");
        let error = mapped.error.unwrap();
        assert!(
            error.contains("DeadlineExceeded") && error.contains("deadline"),
            "{error}"
        );
    }

    #[test]
    fn job_succeeded_is_stopped_without_error() {
        let items = [job(json!({
            "succeeded": 1,
            "conditions": [{ "type": "Complete", "status": "True" }]
        }))];
        assert_eq!(
            map_kubernetes(&items),
            Mapped {
                status: "stopped",
                error: None,
                termination: None,
            }
        );
    }

    #[test]
    fn job_with_active_running_pod_is_running() {
        let items = [
            job(json!({ "active": 1 })),
            pod(json!({ "ready": true, "state": { "running": {} } })),
        ];
        assert_eq!(map_kubernetes(&items).status, "running");
    }

    #[test]
    fn gone_workload_is_stopped() {
        assert_eq!(map_kubernetes(&[]).status, "stopped");
    }

    fn docker(status: &str, oom_killed: bool, exit_code: i64) -> Mapped {
        map_docker(DockerState {
            status,
            oom_killed,
            exit_code,
            error: "",
            name: "orbit-worker-1234",
            restart_count: Some(1),
            finished_at: Some("2026-11-23T10:00:00Z"),
        })
    }

    #[test]
    fn docker_oom_uses_flag_not_exit_code() {
        let oom = docker("exited", true, 137);
        assert_eq!(oom.status, "failed");
        let termination = oom.termination.unwrap();
        assert_eq!(termination.reason, WorkerTerminationReason::WorkerOom);
        assert_eq!(termination.exit_code, Some(137));
        assert_eq!(termination.restart_count, Some(1));
        let killed = docker("exited", false, 137);
        assert_eq!(killed.status, "failed");
        assert!(killed.error.unwrap().contains("not an out-of-memory"));
        assert_eq!(
            killed.termination.unwrap().reason,
            WorkerTerminationReason::WorkerExited
        );
        assert_eq!(docker("exited", false, 0).status, "stopped");
        assert_eq!(docker("running", false, 0).status, "running");
    }

    #[test]
    fn evicted_pod_is_failed_evicted() {
        let mut pod = pod(json!({
            "state": { "terminated": { "reason": "Evicted", "exitCode": 137 } }
        }));
        pod["status"]["reason"] = json!("Evicted");
        pod["status"]["message"] = json!("The node was low on resource: memory.");
        let mapped = map_kubernetes(&[pod]);
        assert_eq!(mapped.status, "failed");
        let termination = mapped.termination.unwrap();
        assert_eq!(termination.reason, WorkerTerminationReason::WorkerEvicted);
        assert_eq!(termination.detail.as_deref(), Some("Evicted"));
    }

    #[test]
    fn node_loss_pod_is_failed_worker_lost() {
        let mut pod = pod(json!({ "state": { "waiting": { "reason": "ContainerCreating" } } }));
        pod["status"]["phase"] = json!("Unknown");
        pod["status"]["reason"] = json!("NodeLost");
        let mapped = map_kubernetes(&[pod]);
        assert_eq!(mapped.status, "failed");
        assert_eq!(
            mapped.termination.unwrap().reason,
            WorkerTerminationReason::WorkerLost
        );
    }

    #[test]
    fn non_zero_exit_carries_exit_code_and_time() {
        let items = [
            job(json!({ "failed": 1 })),
            pod(json!({
                "restartCount": 1,
                "state": { "terminated": {
                    "reason": "Error",
                    "exitCode": 2,
                    "finishedAt": "2026-11-23T10:00:00Z"
                } }
            })),
        ];
        let mapped = map_kubernetes(&items);
        assert_eq!(mapped.status, "failed");
        let termination = mapped.termination.unwrap();
        assert_eq!(termination.reason, WorkerTerminationReason::WorkerExited);
        assert_eq!(termination.exit_code, Some(2));
        assert_eq!(termination.restart_count, Some(1));
        assert_eq!(
            termination.terminated_at.as_deref(),
            Some("2026-11-23T10:00:00Z")
        );
    }

    #[test]
    fn crash_loop_after_oom_reports_worker_oom() {
        let items = [
            deployment(),
            pod(json!({
                "restartCount": 3,
                "state": { "waiting": { "reason": "CrashLoopBackOff" } },
                "lastState": { "terminated": { "reason": "OOMKilled", "exitCode": 137 } }
            })),
        ];
        let mapped = map_kubernetes(&items);
        assert_eq!(
            mapped.termination.unwrap().reason,
            WorkerTerminationReason::WorkerOom
        );
    }

    #[test]
    fn job_failed_without_pod_reports_condition_reason() {
        let items = [job(json!({
            "failed": 1,
            "conditions": [{ "type": "Failed", "status": "True", "reason": "Evicted" }]
        }))];
        let mapped = map_kubernetes(&items);
        assert_eq!(mapped.status, "failed");
        let termination = mapped.termination.unwrap();
        assert_eq!(termination.reason, WorkerTerminationReason::WorkerEvicted);
        assert_eq!(termination.worker_name, "orbit-worker-1234");
    }

    #[test]
    fn job_deadline_derives_from_timeout() {
        assert_eq!(job_deadline_seconds(Some(600), 0, 7200), 900);
        assert_eq!(job_deadline_seconds(None, 3600, 7200), 7200);
        assert_eq!(job_deadline_seconds(Some(0), 3600, 7200), 7200);
    }

    #[test]
    fn job_deadline_includes_the_approval_budget() {
        // timeout 120 s + budget 3600 s + grace 300 s: an approval answered after 8
        // minutes no longer hits DeadlineExceeded.
        assert_eq!(job_deadline_seconds(Some(120), 3600, 7200), 4020);
        assert!(job_deadline_seconds(Some(120), 3600, 7200) > 120 + 8 * 60);
        assert_eq!(job_deadline_seconds(Some(u64::MAX), 3600, 7200), u64::MAX);
    }
}
