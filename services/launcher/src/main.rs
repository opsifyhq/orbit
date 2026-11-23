//! Optional worker launcher.
//!
//! Receives already-authorized launch requests from `orbit-api` and creates Docker
//! containers or Kubernetes Pods. It is the only component with Docker socket or
//! Kubernetes Workload RBAC; the API never receives those privileges.

mod mapping;

use std::collections::BTreeMap;
use std::process::Stdio;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::process::Command;
use uuid::Uuid;

#[derive(Clone)]
struct AppState {
    token: String,
    docker: String,
    kubectl: String,
    namespace: String,
    opencode_auth_export: Option<String>,
    opencode_auth_host_path: Option<String>,
    pi_agent_dir: Option<String>,
    pi_agent_host_dir: Option<String>,
    image_pull_secret: Option<String>,
    image_aliases: Vec<(String, String)>,
    /// Harness auth Secrets a launch may mount (`ORBIT_WORKER_AUTH_SECRETS`). The launcher
    /// cannot read Secrets, so the rule is by name, enforced here and not only in the API.
    auth_secrets: orbit_domain::AuthSecretPolicy,
    /// Capability, privilege, PID and user restrictions for Docker-target containers.
    docker_sandbox: orbit_domain::DockerSandbox,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LaunchRequest {
    launch_id: Uuid,
    profile_id: Uuid,
    name: String,
    harness_kind: String,
    target: String,
    execution_driver: String,
    image: String,
    image_tag: String,
    auth_secret_name: Option<String>,
    #[serde(default)]
    env: Value,
    #[serde(default)]
    resources: Value,
    api_url: String,
    worker_token: String,
    worker_id: String,
    #[serde(default)]
    ca_secret_name: Option<String>,
    #[serde(default = "default_ca_key")]
    ca_secret_key: String,
    #[serde(default)]
    single_execution_id: Option<Uuid>,
    /// Execution timeout for single-execution launches; bounds the Job's
    /// `activeDeadlineSeconds`.
    #[serde(default)]
    timeout_seconds: Option<u64>,
}

/// Deadline for single-execution Jobs whose request carries no timeout (older API).
fn default_job_deadline_seconds() -> u64 {
    std::env::var("ORBIT_LAUNCHER_JOB_DEADLINE_SECONDS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(7200)
}

/// Exited single-execution Docker containers are kept this long (logs and OOM state stay
/// inspectable) and then pruned on the next single-execution launch.
fn docker_retention() -> String {
    std::env::var("ORBIT_LAUNCHER_DOCKER_RETENTION")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "1h".into())
}

fn default_ca_key() -> String {
    "ca-bundle.crt".into()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LaunchStatus {
    status: String,
    runtime_id: Option<String>,
    worker_id: Option<String>,
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    termination: Option<orbit_contracts::WorkerTermination>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Logs {
    logs: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct DriverQuery {
    target: Option<String>,
}

fn docker_target(target: Option<&str>) -> bool {
    matches!(target, Some("docker") | Some("local"))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let state = AppState {
        token: std::env::var("ORBIT_LAUNCHER_TOKEN")
            .map_err(|_| anyhow::anyhow!("ORBIT_LAUNCHER_TOKEN is required"))?,
        docker: std::env::var("ORBIT_LAUNCHER_DOCKER").unwrap_or_else(|_| "docker".into()),
        kubectl: std::env::var("ORBIT_LAUNCHER_KUBECTL").unwrap_or_else(|_| "kubectl".into()),
        namespace: std::env::var("ORBIT_LAUNCHER_NAMESPACE").unwrap_or_else(|_| "orbit".into()),
        opencode_auth_export: std::env::var("ORBIT_OPENCODE_AUTH_EXPORT")
            .ok()
            .filter(|value| !value.trim().is_empty()),
        opencode_auth_host_path: std::env::var("ORBIT_OPENCODE_AUTH_HOST_PATH")
            .ok()
            .filter(|value| !value.trim().is_empty()),
        pi_agent_dir: std::env::var("ORBIT_PI_AGENT_DIR")
            .ok()
            .filter(|value| !value.trim().is_empty()),
        pi_agent_host_dir: std::env::var("ORBIT_PI_AGENT_HOST_DIR")
            .ok()
            .filter(|value| !value.trim().is_empty()),
        image_pull_secret: std::env::var("ORBIT_LAUNCHER_IMAGE_PULL_SECRET")
            .ok()
            .filter(|value| !value.trim().is_empty()),
        image_aliases: std::env::var("ORBIT_LAUNCHER_IMAGE_ALIASES")
            .unwrap_or_default()
            .split(',')
            .filter_map(|pair| pair.trim().split_once('='))
            .map(|(from, to)| (from.trim().to_string(), to.trim().to_string()))
            .filter(|(from, to)| !from.is_empty() && !to.is_empty())
            .collect(),
        auth_secrets: orbit_domain::AuthSecretPolicy::from_env(),
        docker_sandbox: orbit_domain::DockerSandbox::from_env(),
    };
    let addr = std::env::var("ORBIT_LAUNCHER_ADDR").unwrap_or_else(|_| "0.0.0.0:8090".into());
    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/launches", post(launch))
        .route("/launches/{launch_id}", get(status))
        .route("/launches/{launch_id}/stop", post(stop))
        .route("/launches/{launch_id}/logs", get(logs))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!(%addr, "ORBIT launcher listening");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn healthz() -> impl IntoResponse {
    StatusCode::OK
}

fn authorized(headers: &HeaderMap, token: &str) -> bool {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(|value| value == token)
        .unwrap_or(false)
}

/// Workload name for a launch: `orbit-worker-` plus the full UUID without hyphens
/// (45 chars, DNS-1123 safe, well under the 63 char label/name limit even with the
/// suffixes Kubernetes appends to pod names).
///
/// Earlier launchers used only the first 8 hex chars of the UUIDv7, which are timestamp
/// bits and identical for every launch within ~65 s (issue #106).
fn pod_name(launch_id: Uuid) -> String {
    format!("orbit-worker-{}", launch_id.simple())
}

/// Pre-fix name. Only used to find workloads started by an older launcher, and only
/// acted on when the workload's `orbit.launch-id` label proves it belongs to this launch.
fn legacy_pod_name(launch_id: Uuid) -> String {
    format!("orbit-worker-{}", &launch_id.to_string()[..8])
}

/// True when a label value read from a workload identifies `launch_id`.
fn label_matches_launch(label: &str, launch_id: Uuid) -> bool {
    label.trim() == launch_id.to_string()
}

async fn docker_launch_label(state: &AppState, name: &str) -> Option<String> {
    let output = Command::new(&state.docker)
        .args([
            "inspect",
            "-f",
            "{{index .Config.Labels \"orbit.launch-id\"}}",
            name,
        ])
        .stderr(Stdio::null())
        .output()
        .await
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).to_string())
}

/// Container name for a launch: the unique name, or the legacy short name if a container
/// from an older launcher with that name carries this launch's label.
async fn docker_workload_name(state: &AppState, launch_id: Uuid) -> String {
    let name = pod_name(launch_id);
    if docker_exists(state, &name).await {
        return name;
    }
    let legacy = legacy_pod_name(launch_id);
    match docker_launch_label(state, &legacy).await {
        Some(label) if label_matches_launch(&label, launch_id) => legacy,
        _ => name,
    }
}

async fn kubernetes_launch_label(state: &AppState, kind: &str, name: &str) -> Option<String> {
    let output = Command::new(&state.kubectl)
        .args([
            "-n",
            &state.namespace,
            "get",
            kind,
            name,
            "-o",
            "jsonpath={.metadata.labels.orbit\\.launch-id}",
        ])
        .stderr(Stdio::null())
        .output()
        .await
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).to_string())
}

/// Workload name for a launch on Kubernetes (Deployment, Job or legacy Pod), falling back
/// to the legacy short name only when that workload carries this launch's label.
async fn kubernetes_workload_name(state: &AppState, launch_id: Uuid) -> String {
    let name = pod_name(launch_id);
    let legacy = legacy_pod_name(launch_id);
    for kind in ["deployment", "job", "pod"] {
        if kubernetes_launch_label(state, kind, &name).await.is_some() {
            return name;
        }
    }
    for kind in ["deployment", "job", "pod"] {
        if let Some(label) = kubernetes_launch_label(state, kind, &legacy).await {
            if label_matches_launch(&label, launch_id) {
                return legacy;
            }
        }
    }
    name
}

fn rewrite_api(url: &str) -> String {
    url.replace("//localhost:", "//host.docker.internal:")
        .replace("//127.0.0.1:", "//host.docker.internal:")
}

async fn launch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<LaunchRequest>,
) -> Result<Json<LaunchStatus>, (StatusCode, Json<Value>)> {
    if !authorized(&headers, &state.token) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        ));
    }
    let result = match request.target.as_str() {
        "docker" | "local" => launch_docker(&state, &request).await,
        "kubernetes" => launch_kubernetes(&state, &request).await,
        other => Err(anyhow::anyhow!("unsupported launch target '{other}'")),
    };
    match result {
        Ok(status) => Ok(Json(status)),
        Err(error) => {
            tracing::error!(launch_id = %request.launch_id, %error, "launch failed");
            Err((
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": error.to_string() })),
            ))
        }
    }
}

async fn docker_image_exists(state: &AppState, image: &str) -> bool {
    Command::new(&state.docker)
        .args(["image", "inspect", image])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Docker treats an IP and a DNS name for the same registry as different repositories.
/// Resolve configured aliases (`ORBIT_LAUNCHER_IMAGE_ALIASES`) to a locally present image
/// instead of attempting a registry pull while offline.
async fn resolve_docker_image(state: &AppState, image: &str, tag: &str) -> String {
    let exact = format!("{image}:{tag}");
    if docker_image_exists(state, &exact).await {
        return exact;
    }
    for (from, to) in &state.image_aliases {
        if let Some(suffix) = image.strip_prefix(from) {
            let candidate = format!("{to}{suffix}:{tag}");
            if docker_image_exists(state, &candidate).await {
                return candidate;
            }
        }
    }
    exact
}

/// Env values that must never appear in argv, Job/Deployment specs or logs: the worker
/// token and every worker-profile env value (which may carry provider keys).
fn docker_secret_env(request: &LaunchRequest) -> anyhow::Result<Vec<(String, String)>> {
    let mut env = vec![(
        "ORBIT_WORKER_TOKEN".to_string(),
        request.worker_token.clone(),
    )];
    env.extend(profile_env(request)?);
    Ok(env)
}

fn valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Worker-profile env as validated key/value pairs (`env` is an optional JSON object).
fn profile_env(request: &LaunchRequest) -> anyhow::Result<Vec<(String, String)>> {
    let Ok(env) = serde_json::from_value::<BTreeMap<String, String>>(request.env.clone()) else {
        return Ok(Vec::new());
    };
    for key in env.keys() {
        anyhow::ensure!(
            valid_env_name(key),
            "invalid worker profile env name '{key}'"
        );
    }
    Ok(env.into_iter().collect())
}

/// The `--env` / `--env-file` portion of `docker run`. Contains no secret values.
fn docker_env_args(
    request: &LaunchRequest,
    api_url: &str,
    env_file: &std::path::Path,
) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    for (key, value) in [
        ("ORBIT_API_URL", api_url.to_string()),
        ("ORBIT_WORKER_PROFILE_ID", request.profile_id.to_string()),
        ("ORBIT_WORKER_HARNESSES", request.harness_kind.clone()),
        ("ORBIT_WORKER_TARGET", request.target.clone()),
        ("ORBIT_EXECUTION_DRIVER", request.execution_driver.clone()),
        ("ORBIT_WORKER_ID", request.worker_id.clone()),
        ("ORBIT_WORKSPACE_ROOT", "/tmp/orbit-workspaces".to_string()),
        (
            "ORBIT_LOG_FORMAT",
            std::env::var("ORBIT_LOG_FORMAT").unwrap_or_else(|_| "text".into()),
        ),
    ] {
        args.push("--env".into());
        args.push(format!("{key}={value}"));
    }
    if let Some(execution_id) = request.single_execution_id {
        args.push("--env".into());
        args.push(format!("ORBIT_SINGLE_EXECUTION_ID={execution_id}"));
        args.push("--env".into());
        args.push(format!(
            "{}={}",
            orbit_contracts::MAX_APPROVAL_WAIT_ENV,
            orbit_contracts::max_approval_wait_seconds()
        ));
    }
    args.push("--env-file".into());
    args.push(env_file.display().to_string());
    args
}

/// A 0600 `KEY=value` file for `docker run --env-file`, removed on drop.
struct EnvFile {
    path: std::path::PathBuf,
}

impl EnvFile {
    fn write(launch_id: Uuid, env: &[(String, String)]) -> anyhow::Result<Self> {
        use std::io::Write;
        let mut contents = String::new();
        for (key, value) in env {
            anyhow::ensure!(
                !value.contains(['\n', '\r', '\0']),
                "env value for '{key}' contains a line break or NUL and cannot be passed safely"
            );
            contents.push_str(&format!("{key}={value}\n"));
        }
        let path = std::env::temp_dir().join(format!("orbit-launch-{launch_id}.env"));
        let _ = std::fs::remove_file(&path);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&path)?;
        // From here on, Drop removes the file even if the write fails.
        let guard = Self { path };
        file.write_all(contents.as_bytes())?;
        Ok(guard)
    }

    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for EnvFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Start of `docker run` for a worker container: name, launch label and the sandbox
/// flags (capabilities dropped, no-new-privileges, PID limit, non-root user, host gateway
/// only when the API URL needs it). CPU and memory limits are added from the profile.
fn docker_run_base_args(
    sandbox: &orbit_domain::DockerSandbox,
    name: &str,
    launch_id: Uuid,
    api_url: &str,
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "run".into(),
        "-d".into(),
        "--name".into(),
        name.to_string(),
        "--label".into(),
        format!("orbit.launch-id={launch_id}"),
    ];
    args.extend(sandbox.args(api_url));
    args
}

async fn launch_docker(state: &AppState, request: &LaunchRequest) -> anyhow::Result<LaunchStatus> {
    let name = pod_name(request.launch_id);
    let _ = Command::new(&state.docker)
        .args(["rm", "-f", &name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
    let api_url = rewrite_api(&request.api_url);
    let mut args = docker_run_base_args(&state.docker_sandbox, &name, request.launch_id, &api_url);
    if request.single_execution_id.is_some() {
        // Not `--rm`: status (exit code, OOMKilled) and logs are read after the container
        // exits. Instead label it and prune exited ones past the retention window.
        args.push("--label".into());
        args.push("orbit.single-execution=true".into());
        let _ = Command::new(&state.docker)
            .args([
                "container",
                "prune",
                "-f",
                "--filter",
                "label=orbit.single-execution=true",
                "--filter",
                &format!("until={}", docker_retention()),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
    }
    let resources = worker_resources(&request.resources);
    if let Some(cpu) = resources
        .limits
        .cpu
        .as_deref()
        .and_then(orbit_domain::parse_cpu_millis)
    {
        args.push("--cpus".into());
        args.push(format!("{}", cpu as f64 / 1000.0));
    }
    if let Some(memory) = resources
        .limits
        .memory
        .as_deref()
        .and_then(orbit_domain::parse_memory_bytes)
    {
        args.push("--memory".into());
        args.push(memory.to_string());
    }
    let secret_env = docker_secret_env(request)?;
    // Secret values travel in a 0600 env-file that is removed right after `docker run`
    // returns, never in argv (visible in `ps`) and never in the launcher's own logs.
    let env_file = EnvFile::write(request.launch_id, &secret_env)?;
    args.extend(docker_env_args(request, &api_url, env_file.path()));
    let command = "mkdir -p \"$HOME/.local/share/opencode\"; \
         if command -v opencode >/dev/null 2>&1 && [ -f /seed/auth-export.json ]; then \
           opencode auth import --standalone /seed/auth-export.json || true; \
         fi; exec /usr/local/bin/orbit-worker";
    if request.harness_kind == "opencode" {
        // A launcher running in a container must pass the host path used by the Docker
        // daemon; a native launcher can use the path it can see directly.
        let path = state
            .opencode_auth_host_path
            .as_ref()
            .or(state.opencode_auth_export.as_ref())
            .filter(|path| {
                state.opencode_auth_host_path.is_some() || std::path::Path::new(path).exists()
            });
        if let Some(path) = path {
            args.push("-v".into());
            args.push(format!("{path}:/seed/auth-export.json:ro"));
        }
    }
    if request.harness_kind == "pi" {
        let path = state
            .pi_agent_host_dir
            .as_ref()
            .or(state.pi_agent_dir.as_ref())
            .filter(|path| {
                state.pi_agent_host_dir.is_some() || std::path::Path::new(path).exists()
            });
        if let Some(path) = path {
            args.push("-v".into());
            args.push(format!("{path}:/home/orbit/.pi/agent"));
        }
    }
    args.push("--entrypoint".into());
    args.push("/bin/sh".into());
    args.push(resolve_docker_image(state, &request.image, &request.image_tag).await);
    args.push("-c".into());
    args.push(command.to_string());
    let output = Command::new(&state.docker).args(&args).output().await?;
    drop(env_file);
    if !output.status.success() {
        anyhow::bail!(
            "docker run failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    tracing::info!(launch_id = %request.launch_id, profile = %request.name, container = %name, "worker container started");
    Ok(LaunchStatus {
        status: "running".into(),
        runtime_id: Some(name),
        worker_id: Some(request.worker_id.clone()),
        error: None,
        termination: None,
    })
}

async fn docker_exists(state: &AppState, name: &str) -> bool {
    Command::new(&state.docker)
        .args(["inspect", name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|status| status.success())
        .unwrap_or(false)
}

async fn docker_status(state: &AppState, name: &str) -> LaunchStatus {
    let output = match Command::new(&state.docker)
        .args([
            "inspect",
            "-f",
            "{{.State.Status}}|{{.State.OOMKilled}}|{{.State.ExitCode}}|{{.State.FinishedAt}}|{{.RestartCount}}|{{if .State.Error}}{{.State.Error}}{{end}}",
            name,
        ])
        .output()
        .await
    {
        Ok(output) => output,
        Err(error) => {
            return LaunchStatus {
                status: "failed".into(),
                runtime_id: Some(name.into()),
                worker_id: None,
                error: Some(error.to_string()),
                termination: None,
            }
        }
    };
    if !output.status.success() {
        return LaunchStatus {
            status: "stopped".into(),
            runtime_id: Some(name.into()),
            worker_id: None,
            error: None,
            termination: None,
        };
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    // The free-text daemon error is last so a `|` inside it cannot shift the other fields.
    let mut parts = stdout.trim().splitn(6, '|');
    let state_value = parts.next().unwrap_or_default();
    // OOM comes from Docker's own flag, never from exit code 137 (which is any SIGKILL).
    let oom_killed = parts.next().unwrap_or_default() == "true";
    let exit_code = parts
        .next()
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0);
    let finished_at = parts
        .next()
        .filter(|value| !value.is_empty() && !value.starts_with("0001-01-01"));
    let restart_count = parts.next().and_then(|value| value.parse::<i64>().ok());
    let error = parts.next().unwrap_or_default();
    let mapped = mapping::map_docker(mapping::DockerState {
        status: state_value,
        oom_killed,
        exit_code,
        error,
        name,
        restart_count,
        finished_at,
    });
    LaunchStatus {
        status: mapped.status.into(),
        runtime_id: Some(name.into()),
        worker_id: None,
        error: mapped.error,
        termination: mapped.termination,
    }
}

fn worker_resources(value: &Value) -> orbit_domain::WorkerResources {
    serde_json::from_value::<orbit_domain::WorkerResources>(value.clone())
        .unwrap_or_default()
        .with_defaults()
}

fn kubernetes_resources(resources: &orbit_domain::WorkerResources) -> Option<Value> {
    let mut spec = serde_json::Map::new();
    if resources.requests.cpu.is_some() || resources.requests.memory.is_some() {
        spec.insert(
            "requests".into(),
            json!({
                "cpu": resources.requests.cpu,
                "memory": resources.requests.memory,
            }),
        );
    }
    if resources.limits.cpu.is_some() || resources.limits.memory.is_some() {
        spec.insert(
            "limits".into(),
            json!({
                "cpu": resources.limits.cpu,
                "memory": resources.limits.memory,
            }),
        );
    }
    (!spec.is_empty()).then_some(Value::Object(spec))
}

/// Container env for a worker pod. Non-secret values stay inline; the worker token and
/// all worker-profile env values go into a per-launch Secret referenced with
/// `secretKeyRef`, so they never appear in Job/Deployment specs. Returns the env list
/// and the Secret `stringData`.
fn kubernetes_env(
    request: &LaunchRequest,
    secret_name: &str,
) -> anyhow::Result<(Vec<Value>, BTreeMap<String, String>)> {
    let mut env = vec![
        json!({ "name": "ORBIT_API_URL", "value": request.api_url }),
        json!({ "name": "ORBIT_WORKER_PROFILE_ID", "value": request.profile_id.to_string() }),
        json!({ "name": "ORBIT_WORKER_HARNESSES", "value": request.harness_kind }),
        json!({ "name": "ORBIT_WORKER_TARGET", "value": request.target }),
        json!({ "name": "ORBIT_EXECUTION_DRIVER", "value": request.execution_driver }),
        json!({ "name": "ORBIT_WORKER_ID", "value": request.worker_id }),
        json!({ "name": "ORBIT_WORKSPACE_ROOT", "value": "/tmp/orbit-workspaces" }),
        json!({ "name": "HOME", "value": "/home/orbit" }),
    ];
    if let Some(execution_id) = request.single_execution_id {
        env.push(json!({
            "name": "ORBIT_SINGLE_EXECUTION_ID",
            "value": execution_id.to_string()
        }));
        // The in-pod broker enforces the same approval budget the Job deadline allows for.
        env.push(json!({
            "name": orbit_contracts::MAX_APPROVAL_WAIT_ENV,
            "value": orbit_contracts::max_approval_wait_seconds().to_string()
        }));
    }
    let mut data = BTreeMap::new();
    for (key, value) in docker_secret_env(request)? {
        env.push(json!({
            "name": key,
            "valueFrom": { "secretKeyRef": { "name": secret_name, "key": key } }
        }));
        data.insert(key, value);
    }
    Ok((env, data))
}

/// Pod spec skeleton shared by Jobs and Deployments. Worker pods never need Kubernetes
/// API access, so the service-account token is not mounted.
fn base_pod_spec(container: Value, volumes: Vec<Value>) -> Value {
    json!({
        "automountServiceAccountToken": false,
        "securityContext": {
            "runAsNonRoot": true,
            "seccompProfile": { "type": "RuntimeDefault" }
        },
        "containers": [container],
        "volumes": volumes
    })
}

fn kubernetes_secret_manifest(
    namespace: &str,
    name: &str,
    launch_id: Uuid,
    data: &BTreeMap<String, String>,
    owner: Option<Value>,
) -> Value {
    let mut manifest = json!({
        "apiVersion": "v1",
        "kind": "Secret",
        "type": "Opaque",
        "metadata": {
            "name": name,
            "namespace": namespace,
            "labels": {
                "app.kubernetes.io/name": "orbit-worker",
                "orbit.launch-id": launch_id.to_string()
            }
        },
        "stringData": data
    });
    if let Some(owner) = owner {
        manifest["metadata"]["ownerReferences"] = json!([owner]);
    }
    manifest
}

/// Refuse a launch whose profile names a Secret the installer has not allowed, so a
/// profile editor cannot get an arbitrary Secret mounted into an agent pod.
fn check_auth_secret(
    policy: &orbit_domain::AuthSecretPolicy,
    request: &LaunchRequest,
) -> anyhow::Result<()> {
    match request
        .auth_secret_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        Some(name) => policy
            .check(name)
            .map_err(|reason| anyhow::anyhow!("authSecretName refused: {reason}")),
        None => Ok(()),
    }
}

async fn launch_kubernetes(
    state: &AppState,
    request: &LaunchRequest,
) -> anyhow::Result<LaunchStatus> {
    check_auth_secret(&state.auth_secrets, request)?;
    let name = pod_name(request.launch_id);
    let secret_name = format!("{name}-env");
    let (mut env, secret_data) = kubernetes_env(request, &secret_name)?;
    let mut command = vec!["/usr/local/bin/orbit-worker".to_string()];
    let mut mounts = vec![json!({ "name": "tmp", "mountPath": "/tmp" })];
    let mut pod_volumes = vec![json!({ "name": "tmp", "emptyDir": {} })];
    if let Some(secret_name) = &request.ca_secret_name {
        pod_volumes.push(json!({
            "name": "ca-bundle",
            "secret": { "secretName": secret_name }
        }));
        mounts.push(json!({
            "name": "ca-bundle",
            "mountPath": "/etc/ssl/certs/ca-bundle.crt",
            "subPath": request.ca_secret_key,
            "readOnly": true
        }));
        for variable in [
            "SSL_CERT_FILE",
            "REQUESTS_CA_BUNDLE",
            "NODE_EXTRA_CA_CERTS",
            "GIT_SSL_CAINFO",
        ] {
            env.push(json!({
                "name": variable,
                "value": "/etc/ssl/certs/ca-bundle.crt"
            }));
        }
    }
    if request.harness_kind == "opencode" {
        // OpenCode writes its data under $HOME/.local; keep it on an emptyDir so it
        // never writes into the image layer.
        pod_volumes.push(json!({ "name": "opencode-local", "emptyDir": {} }));
        mounts.push(json!({
            "name": "opencode-local",
            "mountPath": "/home/orbit/.local"
        }));
        env.push(json!({
            "name": "XDG_DATA_HOME",
            "value": "/home/orbit/.local/share"
        }));
        env.push(json!({
            "name": "XDG_CONFIG_HOME",
            "value": "/home/orbit/.config"
        }));
        env.push(json!({ "name": "XDG_CACHE_HOME", "value": "/tmp/.cache" }));
        if let Some(secret) = &request.auth_secret_name {
            pod_volumes.push(json!({
                "name": "harness-auth",
                "secret": { "secretName": secret, "optional": true }
            }));
            mounts.push(json!({
                "name": "harness-auth",
                "mountPath": "/seed/auth-export.json",
                "subPath": "auth-export.json",
                "readOnly": true
            }));
            command = vec![
                "/bin/sh".into(),
                "-c".into(),
                "mkdir -p \"$HOME/.local/share/opencode\"; if command -v opencode >/dev/null 2>&1 && [ -f /seed/auth-export.json ]; then opencode auth import --standalone /seed/auth-export.json || true; fi; exec /usr/local/bin/orbit-worker".into(),
            ];
        }
    } else if request.harness_kind == "pi" {
        pod_volumes.push(json!({ "name": "pi-home", "emptyDir": {} }));
        mounts.push(json!({
            "name": "pi-home",
            "mountPath": "/home/orbit/.pi"
        }));
        env.push(json!({
            "name": "PI_CODING_AGENT_DIR",
            "value": "/home/orbit/.pi/agent"
        }));
        if let Some(secret) = &request.auth_secret_name {
            pod_volumes.push(json!({
                "name": "harness-auth",
                "secret": { "secretName": secret, "optional": true }
            }));
            mounts.push(json!({
                "name": "harness-auth",
                "mountPath": "/seed/pi-agent",
                "readOnly": true
            }));
            command = vec![
                "/bin/sh".into(),
                "-c".into(),
                "mkdir -p \"$HOME/.pi/agent\"; cp -a /seed/pi-agent/. \"$HOME/.pi/agent/\" 2>/dev/null || true; exec /usr/local/bin/orbit-worker".into(),
            ];
        }
    }
    let resources = worker_resources(&request.resources);
    if let Some(size) = resources.workspace.size_limit.as_deref() {
        pod_volumes[0] = json!({ "name": "tmp", "emptyDir": { "sizeLimit": size } });
    }
    let mut container = json!({
        "name": "worker",
        "image": format!("{}:{}", request.image, request.image_tag),
        "imagePullPolicy": "IfNotPresent",
        "env": env,
        "command": command,
        "volumeMounts": mounts,
        "securityContext": {
            "runAsNonRoot": true,
            "allowPrivilegeEscalation": false,
            "readOnlyRootFilesystem": false,
            "capabilities": { "drop": ["ALL"] }
        }
    });
    if let Some(resources) = kubernetes_resources(&resources) {
        container["resources"] = resources;
    }
    let labels = json!({
        "app.kubernetes.io/name": "orbit-worker",
        "orbit.launch-id": request.launch_id.to_string()
    });
    let mut pod_spec = base_pod_spec(container, pod_volumes);
    if let Some(secret) = &state.image_pull_secret {
        pod_spec["imagePullSecrets"] = json!([{ "name": secret }]);
    }

    let manifest = if request.single_execution_id.is_some() {
        pod_spec["restartPolicy"] = json!("Never");
        json!({
            "apiVersion": "batch/v1",
            "kind": "Job",
            "metadata": { "name": name, "namespace": state.namespace, "labels": labels },
            "spec": {
                "ttlSecondsAfterFinished": 300,
                "backoffLimit": 0,
                "activeDeadlineSeconds": mapping::job_deadline_seconds(
                    request.timeout_seconds,
                    orbit_contracts::max_approval_wait_seconds(),
                    default_job_deadline_seconds(),
                ),
                "template": {
                    "metadata": { "labels": labels },
                    "spec": pod_spec
                }
            }
        })
    } else {
        // Long-lived workers run as a Deployment so Kubernetes recreates them after node
        // drains, evictions or maintenance. The worker id is stable across restarts.
        json!({
            "apiVersion": "apps/v1",
            "kind": "Deployment",
            "metadata": { "name": name, "namespace": state.namespace, "labels": labels },
            "spec": {
                "replicas": 1,
                "selector": { "matchLabels": labels },
                "template": {
                    "metadata": { "labels": labels },
                    "spec": pod_spec
                }
            }
        })
    };

    // Clean up any previous incarnation (including legacy Pods from older launchers).
    for resource_type in ["deployment", "job", "pod", "secret"] {
        let target = if resource_type == "secret" {
            &secret_name
        } else {
            &name
        };
        let _ = Command::new(&state.kubectl)
            .args([
                "-n",
                &state.namespace,
                "delete",
                resource_type,
                target,
                "--ignore-not-found",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
    }
    // The workload goes first and the Secret follows with its owner reference already
    // set, so Kubernetes deletes it with the workload (stop, relaunch, Job TTL). The
    // launcher has no `get` on Secrets, and `kubectl patch` needs one, so the reference
    // cannot be added afterwards. Pods wait for the Secret (the kubelet retries
    // `secretKeyRef`s). Manifests are piped on stdin, never passed as arguments.
    kubectl_create(state, &manifest).await?;
    let owner = match workload_owner_reference(state, &manifest, &name).await {
        Ok(owner) => Some(owner),
        Err(error) => {
            tracing::warn!(%error, "could not read the workload uid; the worker env Secret is deleted with the next relaunch instead");
            None
        }
    };
    let secret_manifest = kubernetes_secret_manifest(
        &state.namespace,
        &secret_name,
        request.launch_id,
        &secret_data,
        owner,
    );
    if let Err(error) = kubectl_create(state, &secret_manifest).await {
        delete_workload(state, &manifest, &name).await;
        return Err(error);
    }
    tracing::info!(launch_id = %request.launch_id, workload = %name, "worker workload created");
    Ok(LaunchStatus {
        status: "starting".into(),
        runtime_id: Some(name),
        worker_id: Some(request.worker_id.clone()),
        error: None,
        termination: None,
    })
}

async fn kubectl_create(state: &AppState, manifest: &Value) -> anyhow::Result<()> {
    let mut child = Command::new(&state.kubectl)
        .args(["-n", &state.namespace, "create", "-f", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(stdin) = child.stdin.as_mut() {
        use tokio::io::AsyncWriteExt;
        stdin.write_all(manifest.to_string().as_bytes()).await?;
    }
    let output = child.wait_with_output().await?;
    if !output.status.success() {
        anyhow::bail!(
            "kubectl create failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// Owner reference to a just-created Job/Deployment, for the worker env Secret.
async fn workload_owner_reference(
    state: &AppState,
    workload: &Value,
    workload_name: &str,
) -> anyhow::Result<Value> {
    let kind = workload["kind"].as_str().unwrap_or("Deployment");
    let api_version = workload["apiVersion"].as_str().unwrap_or("apps/v1");
    let uid = Command::new(&state.kubectl)
        .args([
            "-n",
            &state.namespace,
            "get",
            &kind.to_ascii_lowercase(),
            workload_name,
            "-o",
            "jsonpath={.metadata.uid}",
        ])
        .output()
        .await?;
    anyhow::ensure!(uid.status.success(), "could not read workload uid");
    let uid = String::from_utf8_lossy(&uid.stdout).trim().to_string();
    anyhow::ensure!(!uid.is_empty(), "workload uid is empty");
    Ok(owner_reference(api_version, kind, workload_name, &uid))
}

fn owner_reference(api_version: &str, kind: &str, name: &str, uid: &str) -> Value {
    json!({
        "apiVersion": api_version,
        "kind": kind,
        "name": name,
        "uid": uid,
        "blockOwnerDeletion": false
    })
}

/// Remove a workload whose env Secret could not be created; it would never start.
async fn delete_workload(state: &AppState, workload: &Value, name: &str) {
    let kind = workload["kind"]
        .as_str()
        .unwrap_or("Deployment")
        .to_ascii_lowercase();
    let _ = Command::new(&state.kubectl)
        .args([
            "-n",
            &state.namespace,
            "delete",
            &kind,
            name,
            "--ignore-not-found",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
}

async fn kubernetes_status(state: &AppState, name: &str, launch_id: Uuid) -> LaunchStatus {
    // One label query returns the Deployment/Job and its pods (legacy bare Pods carry the
    // same label); the mapping itself is a pure function in `mapping.rs`.
    let output = match Command::new(&state.kubectl)
        .args([
            "-n",
            &state.namespace,
            "get",
            "deployments,jobs,pods",
            "-l",
            &format!("orbit.launch-id={launch_id}"),
            "-o",
            "json",
        ])
        .output()
        .await
    {
        Ok(output) => output,
        Err(error) => {
            return LaunchStatus {
                status: "failed".into(),
                runtime_id: Some(name.into()),
                worker_id: None,
                error: Some(error.to_string()),
                termination: None,
            }
        }
    };
    // A transient kubectl/API error must not be reported as a terminal state.
    let parsed = output
        .status
        .success()
        .then(|| serde_json::from_slice::<Value>(&output.stdout).ok())
        .flatten();
    let Some(list) = parsed else {
        return LaunchStatus {
            status: "starting".into(),
            runtime_id: Some(name.into()),
            worker_id: None,
            error: Some(format!(
                "could not read workload state: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )),
            termination: None,
        };
    };
    let items = list
        .get("items")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mapped = mapping::map_kubernetes(items);
    LaunchStatus {
        status: mapped.status.into(),
        runtime_id: Some(name.into()),
        worker_id: None,
        error: mapped.error,
        termination: mapped.termination,
    }
}

async fn status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(launch_id): Path<Uuid>,
    Query(query): Query<DriverQuery>,
) -> Result<Json<LaunchStatus>, (StatusCode, Json<Value>)> {
    if !authorized(&headers, &state.token) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        ));
    }
    let status = if docker_target(query.target.as_deref()) {
        let name = docker_workload_name(&state, launch_id).await;
        docker_status(&state, &name).await
    } else if query.target.as_deref() == Some("kubernetes") {
        let name = kubernetes_workload_name(&state, launch_id).await;
        kubernetes_status(&state, &name, launch_id).await
    } else {
        let name = docker_workload_name(&state, launch_id).await;
        if docker_exists(&state, &name).await {
            docker_status(&state, &name).await
        } else {
            let name = kubernetes_workload_name(&state, launch_id).await;
            kubernetes_status(&state, &name, launch_id).await
        }
    };
    Ok(Json(status))
}

async fn stop(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(launch_id): Path<Uuid>,
    Query(query): Query<DriverQuery>,
) -> Result<Json<LaunchStatus>, (StatusCode, Json<Value>)> {
    if !authorized(&headers, &state.token) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        ));
    }
    let kubernetes = query.target.as_deref() == Some("kubernetes");
    let name = if kubernetes {
        kubernetes_workload_name(&state, launch_id).await
    } else {
        docker_workload_name(&state, launch_id).await
    };
    if kubernetes {
        for resource_type in ["deployment", "job", "pod"] {
            let _ = Command::new(&state.kubectl)
                .args([
                    "-n",
                    &state.namespace,
                    "delete",
                    resource_type,
                    &name,
                    "--ignore-not-found",
                ])
                .status()
                .await;
        }
    } else {
        let _ = Command::new(&state.docker)
            .args(["rm", "-f", &name])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
    }
    Ok(Json(LaunchStatus {
        status: "stopped".into(),
        runtime_id: Some(name),
        worker_id: None,
        error: None,
        termination: None,
    }))
}

async fn logs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(launch_id): Path<Uuid>,
    Query(query): Query<DriverQuery>,
) -> Result<Json<Logs>, (StatusCode, Json<Value>)> {
    if !authorized(&headers, &state.token) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        ));
    }
    let kubernetes = query.target.as_deref() == Some("kubernetes");
    let name = if kubernetes {
        kubernetes_workload_name(&state, launch_id).await
    } else {
        docker_workload_name(&state, launch_id).await
    };
    let output = if kubernetes {
        let by_label = Command::new(&state.kubectl)
            .args([
                "-n",
                &state.namespace,
                "logs",
                "-l",
                &format!("orbit.launch-id={launch_id}"),
                "--tail=200",
            ])
            .output()
            .await;
        match by_label {
            Ok(output) if output.status.success() => Ok(output),
            _ => {
                Command::new(&state.kubectl)
                    .args(["-n", &state.namespace, "logs", &name, "--tail=200"])
                    .output()
                    .await
            }
        }
    } else {
        Command::new(&state.docker)
            .args(["logs", "--tail", "200", &name])
            .output()
            .await
    };
    let logs = match output {
        Ok(output) => {
            let mut text = String::from_utf8_lossy(&output.stdout).to_string();
            if !output.status.success() {
                text.push_str(&String::from_utf8_lossy(&output.stderr));
            }
            text
        }
        Err(error) => format!("could not read logs: {error}"),
    };
    Ok(Json(Logs { logs }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dns_label_safe(name: &str) -> bool {
        name.len() <= 63
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            && name.starts_with(|c: char| c.is_ascii_alphanumeric())
            && name.ends_with(|c: char| c.is_ascii_alphanumeric())
    }

    #[test]
    fn back_to_back_launches_get_distinct_names() {
        let a = Uuid::now_v7();
        let b = Uuid::now_v7();
        assert_ne!(pod_name(a), pod_name(b));
    }

    #[test]
    fn names_are_dns_safe_and_short_enough_for_pod_suffixes() {
        let name = pod_name(Uuid::now_v7());
        assert!(dns_label_safe(&name), "{name}");
        // Deployment pods append `-<10 char hash>-<5 char suffix>`.
        assert!(name.len() + 17 <= 63, "{name}");
        assert!(dns_label_safe(&legacy_pod_name(Uuid::now_v7())));
    }

    #[test]
    fn legacy_label_must_match_exact_launch() {
        let id = Uuid::now_v7();
        assert!(label_matches_launch(&format!("{id}\n"), id));
        assert!(!label_matches_launch(&Uuid::now_v7().to_string(), id));
        assert!(!label_matches_launch("", id));
        assert_ne!(legacy_pod_name(id), pod_name(id));
    }
}

#[cfg(test)]
mod credential_tests {
    use super::*;

    fn request(env: Value, single: Option<Uuid>) -> LaunchRequest {
        LaunchRequest {
            launch_id: Uuid::now_v7(),
            profile_id: Uuid::now_v7(),
            name: "p".into(),
            harness_kind: "mock".into(),
            target: "kubernetes".into(),
            execution_driver: "process".into(),
            image: "img".into(),
            image_tag: "1".into(),
            auth_secret_name: None,
            env,
            resources: Value::Null,
            api_url: "http://api:8080".into(),
            worker_token: "orbit_wk_supersecret".into(),
            worker_id: "w1".into(),
            ca_secret_name: None,
            ca_secret_key: default_ca_key(),
            single_execution_id: single,
            timeout_seconds: None,
        }
    }

    #[test]
    fn docker_args_contain_no_secret_values() {
        let req = request(json!({ "OPENAI_API_KEY": "sk-profile-secret" }), None);
        let args = docker_env_args(&req, "http://host:8080", std::path::Path::new("/tmp/x.env"));
        let joined = args.join(" ");
        assert!(!joined.contains("orbit_wk_supersecret"));
        assert!(!joined.contains("sk-profile-secret"));
        assert!(!joined.contains("ORBIT_WORKER_TOKEN"));
        assert!(!joined.contains("OPENAI_API_KEY"));
        assert!(args.windows(2).any(|w| w == ["--env-file", "/tmp/x.env"]));
    }

    #[test]
    fn env_file_is_private_contains_secrets_and_is_removed() {
        let req = request(json!({ "OPENAI_API_KEY": "sk-profile-secret" }), None);
        let env = docker_secret_env(&req).unwrap();
        let file = EnvFile::write(req.launch_id, &env).unwrap();
        let path = file.path().to_path_buf();
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.contains("ORBIT_WORKER_TOKEN=orbit_wk_supersecret\n"));
        assert!(contents.contains("OPENAI_API_KEY=sk-profile-secret\n"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        drop(file);
        assert!(!path.exists());
    }

    #[test]
    fn env_file_rejects_line_breaks_and_bad_names() {
        let bad_value = request(json!({ "A": "x\nINJECTED=1" }), None);
        let env = docker_secret_env(&bad_value).unwrap();
        assert!(EnvFile::write(bad_value.launch_id, &env).is_err());
        let bad_name = request(json!({ "A B": "x" }), None);
        assert!(docker_secret_env(&bad_name).is_err());
    }

    #[test]
    fn kubernetes_env_uses_secret_key_refs_not_values() {
        let single = Some(Uuid::now_v7());
        let req = request(json!({ "OPENAI_API_KEY": "sk-profile-secret" }), single);
        let (env, data) = kubernetes_env(&req, "orbit-worker-x-env").unwrap();
        let rendered = serde_json::to_string(&env).unwrap();
        assert!(!rendered.contains("orbit_wk_supersecret"));
        assert!(!rendered.contains("sk-profile-secret"));
        for name in ["ORBIT_WORKER_TOKEN", "OPENAI_API_KEY"] {
            let entry = env.iter().find(|e| e["name"] == name).unwrap();
            assert!(entry.get("value").is_none());
            assert_eq!(
                entry["valueFrom"]["secretKeyRef"]["name"],
                "orbit-worker-x-env"
            );
            assert_eq!(entry["valueFrom"]["secretKeyRef"]["key"], name);
        }
        assert_eq!(data["ORBIT_WORKER_TOKEN"], "orbit_wk_supersecret");
        assert_eq!(data["OPENAI_API_KEY"], "sk-profile-secret");
        let secret = kubernetes_secret_manifest(
            "ns",
            "orbit-worker-x-env",
            req.launch_id,
            &data,
            Some(owner_reference(
                "batch/v1",
                "Job",
                "orbit-worker-x",
                "uid-1",
            )),
        );
        assert_eq!(secret["kind"], "Secret");
        assert_eq!(secret["type"], "Opaque");
        assert_eq!(secret["metadata"]["ownerReferences"][0]["kind"], "Job");
        assert_eq!(secret["metadata"]["ownerReferences"][0]["uid"], "uid-1");
        assert_eq!(
            secret["metadata"]["labels"]["orbit.launch-id"],
            req.launch_id.to_string()
        );
    }

    #[test]
    fn worker_pods_do_not_automount_the_service_account_token() {
        let spec = base_pod_spec(json!({ "name": "worker" }), vec![]);
        assert_eq!(spec["automountServiceAccountToken"], false);
        assert_eq!(spec["securityContext"]["runAsNonRoot"], true);
    }

    fn state_with(policy: orbit_domain::AuthSecretPolicy) -> AppState {
        AppState {
            token: "t".into(),
            docker: "docker".into(),
            // Must never be reached for a refused launch.
            kubectl: "/nonexistent/kubectl".into(),
            namespace: "orbit".into(),
            opencode_auth_export: None,
            opencode_auth_host_path: None,
            pi_agent_dir: None,
            pi_agent_host_dir: None,
            image_pull_secret: None,
            image_aliases: Vec::new(),
            auth_secrets: policy,
            docker_sandbox: orbit_domain::DockerSandbox::default(),
        }
    }

    #[test]
    fn docker_run_args_carry_the_sandbox_flags() {
        let id = Uuid::now_v7();
        let sandbox = orbit_domain::DockerSandbox::default();
        let args = docker_run_base_args(&sandbox, "orbit-worker-x", id, "http://orbit-api:8080");
        let pair = |flag: &str| args.windows(2).find(|w| w[0] == flag).map(|w| w[1].clone());
        assert_eq!(args[..2], ["run".to_string(), "-d".to_string()]);
        assert_eq!(pair("--cap-drop").as_deref(), Some("ALL"));
        assert_eq!(pair("--security-opt").as_deref(), Some("no-new-privileges"));
        assert!(pair("--pids-limit").is_some());
        assert_eq!(pair("--user").as_deref(), Some("10001:10001"));
        assert_eq!(pair("--label"), Some(format!("orbit.launch-id={id}")));
        // The API is reachable without the host mapping, so it is not added.
        assert!(pair("--add-host").is_none());
        let local = docker_run_base_args(
            &sandbox,
            "orbit-worker-x",
            id,
            &rewrite_api("http://localhost:8080"),
        );
        assert!(local
            .windows(2)
            .any(|w| w == ["--add-host", "host.docker.internal:host-gateway"]));
    }

    #[tokio::test]
    async fn kubernetes_launch_refuses_a_secret_outside_the_allowlist() {
        let state = state_with(orbit_domain::AuthSecretPolicy::default_policy());
        for name in ["orbit-orbit", "orbit", "postgres-credentials", "Bad_Name"] {
            let mut req = request(json!({}), None);
            req.harness_kind = "pi".into();
            req.auth_secret_name = Some(name.into());
            let error = launch_kubernetes(&state, &req).await.unwrap_err();
            assert!(
                error.to_string().contains("authSecretName refused"),
                "{name}: {error}"
            );
        }
    }

    #[tokio::test]
    async fn kubernetes_launch_passes_the_check_for_allowed_and_absent_names() {
        let state = state_with(orbit_domain::AuthSecretPolicy::default_policy());
        for name in [Some("orbit-pi-auth"), Some("orbit-opencode-auth"), None] {
            let mut req = request(json!({}), None);
            req.auth_secret_name = name.map(str::to_string);
            // Allowed: it gets past the check and only then fails on the missing kubectl.
            let error = launch_kubernetes(&state, &req).await.unwrap_err();
            assert!(
                !error.to_string().contains("authSecretName refused"),
                "{error}"
            );
        }
    }
}
