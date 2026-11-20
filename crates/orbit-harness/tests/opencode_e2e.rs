//! Real OpenCode integration test.
//!
//! Requires the OpenCode CLI (tested with v2.3.0) and a working provider auth store:
//!
//!   ORBIT_OPENCODE_E2E=1 \
//!   ORBIT_OPENCODE_TEST_MODEL=opencode-go/deepseek-v4-pro \
//!   cargo test -p orbit-harness --test opencode_e2e -- --ignored --nocapture
//!
//! The test is ignored by default so `cargo test --workspace` never spends model credits.
use std::time::Duration;

use orbit_contracts::{
    ExecutionRequest, HarnessAdapter, HarnessConfig, InputProvenance, InstructionsBlock,
};
use orbit_domain::{AutonomyLevel, ExecutionMode, ExecutionStatus, HarnessKind, ResourceLimits};
use orbit_harness::OpenCodeHarnessAdapter;

fn request(model: &str) -> ExecutionRequest {
    let (events, mut receiver) = tokio::sync::mpsc::channel(256);
    tokio::spawn(async move { while receiver.recv().await.is_some() {} });
    ExecutionRequest {
        execution_id: uuid::Uuid::now_v7(),
        agent_id: uuid::Uuid::now_v7(),
        agent_version_id: uuid::Uuid::now_v7(),
        agent_name: "opencode-e2e".into(),
        instructions: InstructionsBlock {
            system: "You are a terse test agent. Follow the user's task exactly.".into(),
            constraints: vec!["Do not modify any files.".into()],
        },
        input: orbit_contracts::ExecutionInput {
            task: "Reply with exactly: ORBIT_OPENCODE_E2E_OK".into(),
            payload: serde_json::json!({}),
            provenance: InputProvenance::manual("e2e-test"),
        },
        model: None,
        subagent_models: Vec::new(),
        grants: vec![],
        autonomy: AutonomyLevel::OBSERVE,
        resources: ResourceLimits {
            timeout_seconds: 180,
            ..Default::default()
        },
        harness_config: serde_json::json!({
            "model": model,
            "autoApprove": true,
            "standalone": true
        }),
        extra_env: Default::default(),
        session: orbit_contracts::SessionRequest {
            session_ref: None,
            mode: ExecutionMode::OnDemand,
        },
        workspace: orbit_contracts::WorkspaceSpec {
            root: std::env::temp_dir()
                .join(format!("orbit-opencode-e2e-{}", uuid::Uuid::now_v7()))
                .to_string_lossy()
                .to_string(),
            read_only: false,
        },
        events: Some(events),
        cancel: tokio_util::sync::CancellationToken::new(),
        pause: Default::default(),
    }
}

#[tokio::test]
#[ignore = "requires OpenCode CLI + provider auth"]
async fn opencode_executes_a_task_and_streams_events() {
    if std::env::var("ORBIT_OPENCODE_E2E").ok().as_deref() != Some("1") {
        eprintln!("set ORBIT_OPENCODE_E2E=1 to run");
        return;
    }
    let model = std::env::var("ORBIT_OPENCODE_TEST_MODEL")
        .unwrap_or_else(|_| "opencode-go/deepseek-v4-pro".to_string());
    let adapter = OpenCodeHarnessAdapter;

    let config = HarnessConfig {
        kind: HarnessKind::OpenCode,
        settings: serde_json::json!({ "model": model }),
    };
    let health = adapter.health(&config).await.expect("health");
    assert!(health.healthy, "opencode CLI missing: {health:?}");

    let (events, mut receiver) = tokio::sync::mpsc::channel(256);
    let collected = tokio::spawn(async move {
        let mut events = Vec::new();
        while let Some(event) = receiver.recv().await {
            events.push(event);
        }
        events
    });
    let mut req = request(&model);
    req.events = Some(events);

    let outcome = tokio::time::timeout(Duration::from_secs(240), adapter.execute(req))
        .await
        .expect("adapter must respect its own timeout")
        .expect("execution");

    let events = collected.await.expect("collect events");
    eprintln!(
        "status={:?} events={} output={:?}",
        outcome.status,
        events.len(),
        outcome.output
    );

    assert_eq!(
        outcome.status,
        ExecutionStatus::Succeeded,
        "error: {:?}",
        outcome.error
    );
    assert!(outcome.output.is_some());
    let text = outcome
        .output
        .as_ref()
        .and_then(|value| value.get("text"))
        .and_then(|value| value.as_str())
        .unwrap_or("");
    assert!(
        text.contains("ORBIT_OPENCODE_E2E_OK"),
        "unexpected output: {text}"
    );
    assert!(outcome.session_ref.is_some(), "session id must be captured");
    assert!(!events.is_empty(), "harness events must be streamed");
    assert!(
        events
            .iter()
            .any(|event| event.kind == orbit_contracts::HarnessEventKind::OutputDelta),
        "expected output events"
    );
    assert!(
        outcome.usage.input_tokens.unwrap_or(0) > 0,
        "usage tokens expected"
    );
}
