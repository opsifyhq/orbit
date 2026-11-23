//! Worker heartbeat.
//!
//! Orphaned-execution reaping is a control-plane responsibility and runs in `orbit-api`
//! against the same heartbeat table. Every worker, including a single-execution worker
//! started by the launcher, keeps its heartbeat fresh for as long as it runs: the API
//! fails executions whose worker has not reported for 90 seconds.

use std::time::Duration;

use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::runner::WorkerState;

/// Beat now, then every `heartbeat_interval` until the returned task is aborted.
pub fn spawn(state: WorkerState, heartbeat_interval: Duration) -> JoinHandle<()> {
    spawn_at(state, heartbeat_interval, Instant::now())
}

/// Like [`spawn`], with the first beat at `start`. Abort the task before calling
/// `offline`, so a late beat cannot recreate the heartbeat row the worker just removed.
pub fn spawn_at(
    state: WorkerState,
    heartbeat_interval: Duration,
    start: Instant,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval_at(start, heartbeat_interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            beat(&state).await;
        }
    })
}

/// Write one heartbeat. Failures are logged; the next tick tries again.
pub async fn beat(state: &WorkerState) {
    let hostname = std::env::var("HOSTNAME").ok();
    if let Err(error) = state
        .client
        .heartbeat(&crate::client::HeartbeatRequest {
            worker_id: &state.worker_id,
            harnesses: &state.harnesses,
            target: &state.target,
            hostname: hostname.as_deref(),
        })
        .await
    {
        tracing::warn!(%error, "failed to write worker heartbeat");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::routing::post;
    use axum::Router;

    use super::*;
    use crate::retry::RetryPolicy;

    async fn heartbeat(State(count): State<Arc<AtomicUsize>>) -> StatusCode {
        count.fetch_add(1, Ordering::SeqCst);
        StatusCode::NO_CONTENT
    }

    async fn state_with_counter() -> (WorkerState, Arc<AtomicUsize>) {
        let count = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/api/v1/worker/heartbeat", post(heartbeat))
            .with_state(count.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = WorkerState {
            worker_id: "worker-test".into(),
            client: crate::client::WorkerClient::new(format!("http://{addr}"), "token").unwrap(),
            registry: Arc::new(orbit_harness::HarnessRegistry::builtin()),
            workspace_root: std::env::temp_dir(),
            harnesses: vec!["mock".into()],
            execution_driver: "process".into(),
            target: "docker".into(),
            profile_agent_ids: vec![],
            profile_skill_slugs: vec![],
            allow_web_search: false,
            allow_web_fetch: false,
            native_tool_permissions: serde_json::Value::Null,
            active: Default::default(),
            shutdown: tokio_util::sync::CancellationToken::new(),
            retry: RetryPolicy {
                attempts: 1,
                base: Duration::from_millis(5),
                max_delay: Duration::from_millis(5),
            },
        };
        (state, count)
    }

    /// The single-execution path used to send one heartbeat and nothing after it, so the
    /// API reaped every run longer than 90 seconds as `worker_lost` (#298).
    #[tokio::test]
    async fn heartbeats_repeat_until_aborted() {
        let (state, count) = state_with_counter().await;
        let handle = spawn(state, Duration::from_millis(40));
        tokio::time::sleep(Duration::from_millis(150)).await;
        handle.abort();
        let _ = handle.await;
        let beats = count.load(Ordering::SeqCst);
        assert!(beats >= 3, "expected repeated heartbeats, got {beats}");

        tokio::time::sleep(Duration::from_millis(120)).await;
        assert_eq!(
            count.load(Ordering::SeqCst),
            beats,
            "no heartbeat may be written after the task is aborted"
        );
    }

    #[tokio::test]
    async fn spawn_at_waits_for_the_first_beat() {
        let (state, count) = state_with_counter().await;
        let handle = spawn_at(
            state,
            Duration::from_millis(40),
            Instant::now() + Duration::from_millis(100),
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(count.load(Ordering::SeqCst), 0);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(count.load(Ordering::SeqCst) >= 1);
        handle.abort();
    }
}
