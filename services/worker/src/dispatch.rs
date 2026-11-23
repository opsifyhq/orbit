//! Per-execution container dispatch.
//!
//! Instead of running the harness inside the long-lived worker process, the parent
//! claims the execution and starts a short-lived container running `orbit-worker` in
//! single-execution mode. The child connects to PostgreSQL/NATS, runs exactly one
//! execution (including its own tool broker/MCP bridge), writes events and exits.
//!
//! Docker socket access is root-equivalent. This driver is for single-user/homelab
//! deployments; Kubernetes Job dispatch is the production-grade equivalent.

use std::time::Duration;

use chrono::Utc;
use orbit_contracts::{HarnessError, HarnessErrorKind};
use tokio::process::Command;

use crate::runner::WorkerState;

fn image_for(harness: &str) -> Option<String> {
    let key = format!(
        "ORBIT_EXECUTION_IMAGE_{}",
        harness.to_uppercase().replace('-', "_")
    );
    std::env::var(&key)
        .ok()
        .or_else(|| std::env::var("ORBIT_EXECUTION_IMAGE").ok())
        .filter(|value| !value.trim().is_empty())
}

fn rewrite_host(url: &str) -> String {
    if std::env::var("ORBIT_DISPATCH_REWRITE_HOSTS")
        .ok()
        .as_deref()
        == Some("false")
    {
        return url.to_string();
    }
    url.replace("//localhost:", "//host.docker.internal:")
        .replace("//127.0.0.1:", "//host.docker.internal:")
        // postgres URLs put credentials before the host: user:pass@localhost:5432
        .replace("@localhost:", "@host.docker.internal:")
        .replace("@127.0.0.1:", "@host.docker.internal:")
}

pub async fn run(
    state: &WorkerState,
    context: &crate::client::WorkerExecutionContext,
    cancel: tokio_util::sync::CancellationToken,
) -> anyhow::Result<()> {
    let execution = &context.execution;
    let Some(image) = image_for(&execution.harness_kind) else {
        crate::runner::fail_execution(
            state,
            execution,
            HarnessError::new(
                HarnessErrorKind::InvalidConfig,
                format!(
                    "docker execution driver has no image for harness '{}' (set ORBIT_EXECUTION_IMAGE or ORBIT_EXECUTION_IMAGE_{})",
                    execution.harness_kind,
                    execution.harness_kind.to_uppercase()
                ),
            ),
        )
        .await?;
        return Ok(());
    };

    let docker = std::env::var("ORBIT_DOCKER_BIN").unwrap_or_else(|_| "docker".into());
    let container_name = format!(
        "orbit-exec-{}-{}",
        &execution.id.to_string()[..8],
        Utc::now().timestamp_millis()
    );
    let worker_id = format!("container-{container_name}");
    let api_url = rewrite_host(state.client.base_url());
    let worker_token = state.client.token().to_string();

    let mut args = dispatch_base_args(
        &container_name,
        &orbit_domain::DockerSandbox::from_env(),
        &DispatchLimits::from_env(),
        &api_url,
    );

    let auth_export = std::env::var("ORBIT_OPENCODE_AUTH_EXPORT").ok();
    if execution.harness_kind == "opencode" {
        if let Some(path) = &auth_export {
            if std::path::Path::new(path).exists() {
                args.push("-v".into());
                args.push(format!("{path}:/seed/auth-export.json:ro"));
            }
        }
    }

    let envs: Vec<(&str, String)> = vec![
        (
            "ORBIT_ENV",
            std::env::var("ORBIT_ENV").unwrap_or_else(|_| "development".into()),
        ),
        ("ORBIT_API_URL", api_url),
        ("ORBIT_WORKER_TOKEN", worker_token),
        ("ORBIT_SINGLE_EXECUTION_ID", execution.id.to_string()),
        ("ORBIT_WORKER_ID", worker_id.clone()),
        ("ORBIT_WORKER_HARNESSES", execution.harness_kind.clone()),
        ("ORBIT_WORKER_TARGET", "docker".into()),
        ("ORBIT_EXECUTION_DRIVER", "process".into()),
        ("ORBIT_WORKER_POLL_MS", "1000".into()),
        (
            orbit_contracts::MAX_APPROVAL_WAIT_ENV,
            orbit_contracts::max_approval_wait_seconds().to_string(),
        ),
        ("ORBIT_WORKSPACE_ROOT", "/tmp/orbit-workspaces".into()),
        ("ORBIT_MCP_BROKER_BIN", "/usr/local/bin/orbit-worker".into()),
        (
            "ORBIT_LOG_FORMAT",
            std::env::var("ORBIT_LOG_FORMAT").unwrap_or_else(|_| "json".into()),
        ),
    ];
    let (env_args, secret_env) = docker_env_args(&envs);
    args.extend(env_args);

    // Override the image entrypoint so the auth import wrapper runs and then execs the
    // worker binary. Without this, `sh -c ...` would be passed as arguments to the
    // worker entrypoint and the import would be skipped.
    args.push("--entrypoint".into());
    args.push("/bin/sh".into());
    args.push(image.clone());
    args.push("-c".into());
    args.push(
        "mkdir -p \"$HOME/.local/share/opencode\"; \
         if [ -n \"$XDG_CACHE_HOME\" ]; then mkdir -p \"$XDG_CACHE_HOME\"; fi; \
         if command -v opencode >/dev/null 2>&1 && [ -f /seed/auth-export.json ]; then \
           opencode auth import --standalone /seed/auth-export.json || true; \
         fi; \
         exec /usr/local/bin/orbit-worker"
            .into(),
    );

    tracing::info!(
        execution_id = %execution.id,
        harness = %execution.harness_kind,
        image = %image,
        container = %container_name,
        "dispatching execution to a per-execution container"
    );

    let mut command = Command::new(&docker);
    command.args(&args);
    // Secret values reach the docker CLI through its environment, never its argv.
    command.envs(secret_env);
    // Inherit logs instead of piping: a per-execution container can produce a lot of
    // output, and an undrained pipe would deadlock the parent.
    command.stdout(std::process::Stdio::inherit());
    command.stderr(std::process::Stdio::inherit());
    let mut child = command.spawn().map_err(|error| {
        anyhow::anyhow!("could not start docker for per-execution container: {error}")
    })?;

    let wait = child.wait();
    // The container enforces the real timeout (pausing for approvals); this is only a
    // backstop, so it allows for the whole approval budget on top.
    let timeout = dispatch_backstop(
        execution.timeout_seconds.max(1) as u64,
        orbit_contracts::max_approval_wait_seconds(),
    );
    // Only worker shutdown cancels this token (user cancellation is delivered to the
    // container's own watcher). The container is stopped, then the run is reported as
    // failed by `supervise`'s caller path below.
    let status = tokio::select! {
        _ = cancel.cancelled() => {
            let _ = Command::new(&docker)
                .args(["rm", "-f", &container_name])
                .status()
                .await;
            crate::runner::fail_execution(
                state,
                execution,
                HarnessError::new(
                    HarnessErrorKind::ExecutionFailed,
                    "worker shutdown: the per-execution container was stopped",
                ),
            )
            .await?;
            return Ok(());
        }
        waited = tokio::time::timeout(timeout, wait) => match waited {
        Ok(result) => result?,
        Err(_) => {
            let _ = Command::new(&docker)
                .args(["rm", "-f", &container_name])
                .status()
                .await;
            crate::runner::fail_execution(
                state,
                execution,
                HarnessError::timeout(execution.timeout_seconds as u64),
            )
            .await?;
            return Ok(());
        }
        }
    };

    let stderr = String::new();
    if !status.success() {
        let message = format!(
            "per-execution container exited with {}: {}",
            status,
            stderr.trim()
        );
        tracing::warn!(execution_id = %execution.id, %message, "container execution failed");
        let current = state.client.context(execution.id).await?;
        if !matches!(
            orbit_domain::ExecutionStatus::parse(&current.execution.status),
            Some(status) if status.is_terminal()
        ) {
            crate::runner::fail_execution(
                state,
                execution,
                HarnessError::new(HarnessErrorKind::ExecutionFailed, message),
            )
            .await?;
        }
    } else {
        tracing::info!(execution_id = %execution.id, "per-execution container finished");
    }
    Ok(())
}

/// CPU and memory limits for per-execution containers (`ORBIT_DISPATCH_CPUS`,
/// `ORBIT_DISPATCH_MEMORY`, in `docker run` syntax). `0` disables a limit; an invalid
/// value keeps the default so a typo never removes it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DispatchLimits {
    cpus: Option<String>,
    memory: Option<String>,
}

const DEFAULT_DISPATCH_CPUS: &str = "2";
const DEFAULT_DISPATCH_MEMORY: &str = "2g";

impl DispatchLimits {
    fn from_env() -> Self {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let read = |key: &str, default: &str, valid: fn(&str) -> bool| match lookup(key)
            .map(|value| value.trim().to_ascii_lowercase())
        {
            Some(value) if value == "0" => None,
            Some(value) if valid(&value) => Some(value),
            _ => Some(default.to_string()),
        };
        Self {
            cpus: read("ORBIT_DISPATCH_CPUS", DEFAULT_DISPATCH_CPUS, |value| {
                value
                    .parse::<f64>()
                    .is_ok_and(|cpus| cpus > 0.0 && cpus.is_finite())
            }),
            memory: read("ORBIT_DISPATCH_MEMORY", DEFAULT_DISPATCH_MEMORY, |value| {
                let digits = value.trim_end_matches(['b', 'k', 'm', 'g']);
                value.len() - digits.len() <= 1
                    && !digits.is_empty()
                    && digits.bytes().all(|b| b.is_ascii_digit())
                    && digits.parse::<u64>().is_ok_and(|amount| amount > 0)
            }),
        }
    }
}

/// Start of `docker run` for a per-execution container: the sandbox flags shared with
/// the launcher, then CPU and memory limits.
fn dispatch_base_args(
    container_name: &str,
    sandbox: &orbit_domain::DockerSandbox,
    limits: &DispatchLimits,
    api_url: &str,
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "run".into(),
        "--rm".into(),
        "--name".into(),
        container_name.to_string(),
    ];
    args.extend(sandbox.args(api_url));
    if let Some(cpus) = &limits.cpus {
        args.push("--cpus".into());
        args.push(cpus.clone());
    }
    if let Some(memory) = &limits.memory {
        args.push("--memory".into());
        args.push(memory.clone());
    }
    args
}

/// Variables whose values must not appear in the docker CLI's argv (visible in `ps`).
/// Grace on top of timeout and approval budget for container start-up and teardown.
const BACKSTOP_GRACE_SECONDS: u64 = 180;

/// Outer wall-clock kill deadline for a per-execution container:
/// `timeout + approval budget + grace`. The in-container pause-aware clock is the real
/// timeout authority; this only catches a container that never finishes.
fn dispatch_backstop(timeout_seconds: u64, approval_budget_seconds: u64) -> Duration {
    let timeout = timeout_seconds.max(1);
    Duration::from_secs(
        timeout
            .saturating_add(approval_budget_seconds)
            .saturating_add(BACKSTOP_GRACE_SECONDS),
    )
}

const SECRET_ENV: &[&str] = &["ORBIT_WORKER_TOKEN"];

/// Split container env into `docker run` arguments and the secret values that must be
/// set in the docker child's environment. Secrets are passed as `-e NAME` (no value),
/// which makes the docker CLI read the value from its own environment.
fn docker_env_args(envs: &[(&str, String)]) -> (Vec<String>, Vec<(String, String)>) {
    let mut args = Vec::new();
    let mut secrets = Vec::new();
    for (key, value) in envs {
        if value.is_empty() {
            continue;
        }
        args.push("-e".to_string());
        if SECRET_ENV.contains(key) {
            args.push((*key).to_string());
            secrets.push(((*key).to_string(), value.clone()));
        } else {
            args.push(format!("{key}={value}"));
        }
    }
    (args, secrets)
}

#[cfg(test)]
mod docker_env_tests {
    use super::*;

    #[test]
    fn worker_token_is_not_in_docker_argv() {
        let envs: Vec<(&str, String)> = vec![
            ("ORBIT_API_URL", "http://api".into()),
            ("ORBIT_WORKER_TOKEN", "orbit_wk_supersecret".into()),
            ("EMPTY", String::new()),
        ];
        let (args, secrets) = docker_env_args(&envs);
        assert!(!args.iter().any(|a| a.contains("orbit_wk_supersecret")));
        assert!(args.windows(2).any(|w| w == ["-e", "ORBIT_WORKER_TOKEN"]));
        assert!(args.contains(&"ORBIT_API_URL=http://api".to_string()));
        assert!(!args.iter().any(|a| a.starts_with("EMPTY")));
        assert_eq!(
            secrets,
            vec![(
                "ORBIT_WORKER_TOKEN".to_string(),
                "orbit_wk_supersecret".to_string()
            )]
        );
    }

    fn limits(vars: &[(&str, &str)]) -> DispatchLimits {
        let vars: std::collections::HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        DispatchLimits::from_lookup(|key| vars.get(key).cloned())
    }

    #[test]
    fn dispatched_containers_are_sandboxed_and_limited() {
        let args = dispatch_base_args(
            "orbit-exec-x",
            &orbit_domain::DockerSandbox::default(),
            &limits(&[]),
            "http://orbit-api:8080",
        );
        let pair = |flag: &str| args.windows(2).find(|w| w[0] == flag).map(|w| w[1].clone());
        assert_eq!(pair("--cap-drop").as_deref(), Some("ALL"));
        assert_eq!(pair("--security-opt").as_deref(), Some("no-new-privileges"));
        assert!(pair("--pids-limit").is_some());
        assert_eq!(pair("--user").as_deref(), Some("10001:10001"));
        assert_eq!(pair("--cpus").as_deref(), Some("2"));
        assert_eq!(pair("--memory").as_deref(), Some("2g"));
        assert!(pair("--add-host").is_none());

        let local = dispatch_base_args(
            "orbit-exec-x",
            &orbit_domain::DockerSandbox::default(),
            &limits(&[]),
            &rewrite_host("http://localhost:8080"),
        );
        assert!(local
            .windows(2)
            .any(|w| w == ["--add-host", "host.docker.internal:host-gateway"]));
    }

    #[test]
    fn dispatch_limits_accept_overrides_and_reject_garbage() {
        assert_eq!(
            limits(&[
                ("ORBIT_DISPATCH_CPUS", "0.5"),
                ("ORBIT_DISPATCH_MEMORY", "512M")
            ]),
            DispatchLimits {
                cpus: Some("0.5".into()),
                memory: Some("512m".into())
            }
        );
        assert_eq!(
            limits(&[("ORBIT_DISPATCH_CPUS", "0"), ("ORBIT_DISPATCH_MEMORY", "0")]),
            DispatchLimits {
                cpus: None,
                memory: None
            }
        );
        for (cpus, memory) in [("-1", "1gg"), ("lots", "--privileged"), ("NaN", "g")] {
            assert_eq!(
                limits(&[
                    ("ORBIT_DISPATCH_CPUS", cpus),
                    ("ORBIT_DISPATCH_MEMORY", memory)
                ]),
                DispatchLimits {
                    cpus: Some("2".into()),
                    memory: Some("2g".into())
                },
                "{cpus} {memory}"
            );
        }
    }

    #[test]
    fn backstop_covers_timeout_approval_budget_and_grace() {
        assert_eq!(
            dispatch_backstop(120, 3600),
            Duration::from_secs(120 + 3600 + 180)
        );
        // Two ~14 minute approvals fit inside the default budget.
        assert!(dispatch_backstop(120, 3600) > Duration::from_secs(120 + 2 * 14 * 60));
        assert_eq!(dispatch_backstop(0, 60), Duration::from_secs(1 + 60 + 180));
        assert_eq!(
            dispatch_backstop(u64::MAX, u64::MAX),
            Duration::from_secs(u64::MAX)
        );
    }
}
