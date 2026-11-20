//! Real Pi integration test (tested with Pi 1.5.2).
//!
//!   ORBIT_PI_E2E=1 \
//!   ORBIT_PI_TEST_MODEL=opencode-go/deepseek-v4-pro \
//!   cargo test -p orbit-harness --test pi_e2e -- --ignored --nocapture
use std::time::Duration;

use orbit_contracts::{ExecutionRequest, HarnessAdapter, InputProvenance, InstructionsBlock};
use orbit_domain::{AutonomyLevel, ExecutionMode, ExecutionStatus, ResourceLimits};
use orbit_harness::PiHarnessAdapter;

#[tokio::test]
#[ignore = "requires Pi CLI + provider auth"]
async fn pi_executes_a_task_and_streams_events() {
    if std::env::var("ORBIT_PI_E2E").ok().as_deref() != Some("1") {
        eprintln!("set ORBIT_PI_E2E=1 to run");
        return;
    }
    let model = std::env::var("ORBIT_PI_TEST_MODEL")
        .unwrap_or_else(|_| "opencode-go/deepseek-v4-pro".to_string());
    let adapter = PiHarnessAdapter;
    let config = orbit_contracts::HarnessConfig {
        kind: orbit_domain::HarnessKind::Pi,
        settings: serde_json::json!({ "model": model, "thinking": "low" }),
    };
    let health = adapter.health(&config).await.expect("health");
    assert!(health.healthy, "pi CLI missing: {health:?}");

    let (events, mut receiver) = tokio::sync::mpsc::channel(256);
    let collected = tokio::spawn(async move {
        let mut events = Vec::new();
        while let Some(event) = receiver.recv().await {
            events.push(event);
        }
        events
    });
    let request = ExecutionRequest {
        execution_id: uuid::Uuid::now_v7(),
        agent_id: uuid::Uuid::now_v7(),
        agent_version_id: uuid::Uuid::now_v7(),
        agent_name: "pi-e2e".into(),
        instructions: InstructionsBlock {
            system: "You are a terse test agent. Follow the task exactly.".into(),
            constraints: vec!["Do not modify files.".into()],
        },
        input: orbit_contracts::ExecutionInput {
            task: "Reply with exactly: PI_ORBIT_E2E_OK".into(),
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
        harness_config: serde_json::json!({ "model": model, "thinking": "low" }),
        extra_env: Default::default(),
        session: orbit_contracts::SessionRequest {
            session_ref: None,
            mode: ExecutionMode::OnDemand,
        },
        workspace: orbit_contracts::WorkspaceSpec {
            root: std::env::temp_dir()
                .join(format!("orbit-pi-e2e-{}", uuid::Uuid::now_v7()))
                .to_string_lossy()
                .to_string(),
            read_only: false,
        },
        events: Some(events),
        cancel: tokio_util::sync::CancellationToken::new(),
        pause: Default::default(),
    };
    let outcome = tokio::time::timeout(Duration::from_secs(240), adapter.execute(request))
        .await
        .expect("adapter timeout")
        .expect("execution");
    let events = collected.await.expect("collect");
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
    let text = outcome
        .output
        .as_ref()
        .and_then(|v| v.get("text"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(
        text.contains("PI_ORBIT_E2E_OK"),
        "unexpected output: {text}"
    );
    assert!(outcome.session_ref.is_some());
    assert!(outcome.usage.input_tokens.unwrap_or(0) > 0);
    assert!(events
        .iter()
        .any(|e| e.kind == orbit_contracts::HarnessEventKind::OutputDelta));
}
