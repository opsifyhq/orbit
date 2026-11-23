mod broker;
mod client;
mod connectors;
mod dispatch;
mod git;
mod github;
mod gitlab;
mod liveness;
mod mcp_broker;
mod prompt;
mod retry;
mod runner;
mod skills;
mod subagents;
mod x;

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use orbit_harness::HarnessRegistry;
use tokio::sync::Semaphore;

use crate::client::WorkerClient;

#[derive(Debug, Clone)]
pub struct WorkerConfig {
    pub worker_id: String,
    pub api_url: String,
    pub worker_token: String,
    pub poll_interval: Duration,
    pub concurrency: usize,
    pub workspace_root: String,
    /// Harness kinds this worker may claim. Enables separate harness images/workers.
    pub harnesses: Vec<String>,
    pub heartbeat_interval: Duration,
    pub execution_driver: String,
    pub target: String,
    pub profile_id: Option<uuid::Uuid>,
}

impl WorkerConfig {
    fn from_env() -> anyhow::Result<Self> {
        let worker_id = std::env::var("ORBIT_WORKER_ID").unwrap_or_else(|_| {
            format!(
                "worker-{}",
                std::env::var("HOSTNAME").unwrap_or_else(|_| uuid::Uuid::now_v7().to_string())
            )
        });
        Ok(Self {
            worker_id,
            api_url: std::env::var("ORBIT_API_URL")
                .unwrap_or_else(|_| "http://localhost:8080".into()),
            worker_token: std::env::var("ORBIT_WORKER_TOKEN")
                .context("ORBIT_WORKER_TOKEN is required (generate it from a worker profile)")?,
            poll_interval: Duration::from_millis(
                std::env::var("ORBIT_WORKER_POLL_MS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1000),
            ),
            concurrency: std::env::var("ORBIT_WORKER_CONCURRENCY")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(4)
                .max(1),
            workspace_root: std::env::var("ORBIT_WORKSPACE_ROOT")
                .unwrap_or_else(|_| "/tmp/orbit-workspaces".into()),
            harnesses: std::env::var("ORBIT_WORKER_HARNESSES")
                .unwrap_or_else(|_| "mock,opencode,claude_code,pi,generic".into())
                .split(',')
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .collect(),
            heartbeat_interval: Duration::from_secs(
                std::env::var("ORBIT_WORKER_HEARTBEAT_SECONDS")
                    .ok()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(10)
                    .max(2),
            ),
            execution_driver: std::env::var("ORBIT_EXECUTION_DRIVER")
                .unwrap_or_else(|_| "process".into()),
            target: std::env::var("ORBIT_WORKER_TARGET").unwrap_or_else(|_| "local".into()),
            profile_id: std::env::var("ORBIT_WORKER_PROFILE_ID")
                .ok()
                .and_then(|value| uuid::Uuid::parse_str(&value).ok()),
        })
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Subcommand dispatch: `orbit-worker mcp-broker` runs the MCP stdio bridge
    // so a single binary can be shipped in custom worker images. It runs before
    // `.env` is loaded: the bridge's working directory is the agent workspace, and a
    // repository's `.env` must not set the proxy, CA or broker URL for a client that
    // carries the broker token (#325).
    if std::env::args().nth(1).as_deref() == Some("mcp-broker") {
        return mcp_broker::run().await;
    }

    dotenvy::dotenv().ok();

    orbit_api::telemetry::init("orbit-worker");

    let config = WorkerConfig::from_env().context("invalid worker configuration")?;
    let client = WorkerClient::new(&config.api_url, &config.worker_token)
        .context("invalid worker API configuration")?;

    // Worker-profile connection mappings are resolved per execution (with retry) so a
    // rotated value is picked up and an API outage never yields a silently empty env.
    let registry = Arc::new(HarnessRegistry::builtin());

    // The API may still be migrating/seeding while a worker starts. Retry session
    // discovery instead of crash-looping the container.
    let mut session = None;
    for attempt in 1..=45 {
        match client.session().await {
            Ok(found) => {
                session = Some(found);
                break;
            }
            Err(error) => {
                tracing::warn!(%error, attempt, "waiting for orbit-api worker session");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
    let session = session.context(
        "could not establish a worker session after 90s; check ORBIT_API_URL and ORBIT_WORKER_TOKEN",
    )?;

    let mut harnesses = session.harnesses.clone();
    if harnesses.is_empty() {
        harnesses = config.harnesses.clone();
    }
    let target = session
        .target
        .clone()
        .unwrap_or_else(|| config.target.clone());
    let execution_driver = session
        .execution_driver
        .clone()
        .unwrap_or_else(|| config.execution_driver.clone());

    let boot_id = uuid::Uuid::now_v7();
    let boot_started_at = chrono::Utc::now();

    let state = runner::WorkerState {
        worker_id: config.worker_id.clone(),
        boot_id,
        boot_started_at,
        client: client.clone(),
        registry: registry.clone(),
        workspace_root: std::path::PathBuf::from(&config.workspace_root),
        harnesses,
        execution_driver,
        target,
        profile_agent_ids: session.agent_ids.clone(),
        profile_skill_slugs: session.skill_slugs.clone(),
        allow_web_search: session.allow_web_search,
        allow_web_fetch: session.allow_web_fetch,
        native_tool_permissions: session.native_tool_permissions,
        active: Arc::new(std::sync::Mutex::new(std::collections::BTreeMap::new())),
        shutdown: tokio_util::sync::CancellationToken::new(),
        retry: retry::RetryPolicy::lifecycle(),
    };

    tracing::info!(
        worker_id = %config.worker_id,
        concurrency = config.concurrency,
        harnesses = ?state.harnesses,
        profile = ?session.profile_name,
        "ORBIT worker started"
    );

    // Per-execution container mode: run exactly the assigned execution and exit.
    if let Ok(execution_id) = std::env::var("ORBIT_SINGLE_EXECUTION_ID") {
        let execution_id = uuid::Uuid::parse_str(&execution_id)
            .context("ORBIT_SINGLE_EXECUTION_ID must be a UUID")?;
        tracing::info!(%execution_id, worker_id = %state.worker_id, "single-execution worker starting");
        // Report in before the run starts, then keep the heartbeat fresh for the whole run:
        // the API fails executions whose worker has been silent for 90 seconds (#298).
        liveness::beat(&state).await;
        let heartbeat = liveness::spawn_at(
            state.clone(),
            config.heartbeat_interval,
            tokio::time::Instant::now() + config.heartbeat_interval,
        );
        // This process is PID 1 in its container, which ignores an unhandled SIGTERM:
        // handle it like the long-running path (cancel, let the adapter stop, report).
        let result = runner::run_single_until(
            state.clone(),
            execution_id,
            shutdown_signal(),
            shutdown_grace(),
        )
        .await;
        heartbeat.abort();
        let _ = state.client.offline(&state.worker_id).await;
        result?;
        tracing::info!(%execution_id, "single-execution worker finished");
        return Ok(());
    }

    let semaphore = Arc::new(Semaphore::new(config.concurrency));
    let heartbeat = liveness::spawn(state.clone(), config.heartbeat_interval);

    // The signal only flips `state.shutdown`; it never drops the dispatch loop, so a claim
    // request already in flight finishes and its execution is tracked and reported.
    let signal_state = state.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        tracing::info!("shutdown signal received; stopping claims and interrupting executions");
        signal_state.shutdown.cancel();
    });

    while !state.shutdown.is_cancelled() {
        if let Err(err) = runner::claim_and_dispatch(&state, &semaphore, config.poll_interval).await
        {
            tracing::error!(error = %err, "dispatch loop error");
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                _ = state.shutdown.cancelled() => {}
            }
        }
    }

    // Interrupt in-flight executions, wait a bounded grace period for them to report,
    // then report whatever is left as failed ("worker shutdown").
    runner::shutdown(&state, shutdown_grace()).await;
    heartbeat.abort();
    if let Err(error) = state.client.offline(&state.worker_id).await {
        tracing::warn!(%error, "failed to remove worker heartbeat");
    }
    tracing::info!("ORBIT worker stopped");
    Ok(())
}

fn shutdown_grace() -> Duration {
    Duration::from_secs(
        std::env::var("ORBIT_WORKER_SHUTDOWN_GRACE_SECONDS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(15),
    )
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.expect("ctrl-c handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
