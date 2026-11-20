use std::sync::Arc;

use orbit_domain::HarnessKind;
use orbit_harness::registry::HarnessRegistry;
use orbit_harness::testing::{assert_adapter_contract, assert_fails_closed};

#[tokio::test]
async fn mock_adapter_satisfies_contract() {
    let registry = HarnessRegistry::builtin();
    let adapter = registry.require(HarnessKind::Mock).unwrap();
    assert_adapter_contract(adapter).await;
}

#[tokio::test]
async fn unimplemented_adapters_fail_closed() {
    let registry = HarnessRegistry::builtin();
    for kind in [HarnessKind::ClaudeCode, HarnessKind::Generic] {
        let adapter = registry.require(kind).unwrap();
        assert_fails_closed(adapter).await;
    }
}

#[tokio::test]
async fn registry_exposes_all_harness_families() {
    let registry = HarnessRegistry::builtin();
    let kinds = registry.kinds();
    for kind in HarnessKind::ALL {
        assert!(kinds.contains(&kind), "missing harness {kind}");
    }
    let descriptors = registry.descriptors();
    assert_eq!(descriptors.len(), HarnessKind::ALL.len());
    let opencode = descriptors
        .iter()
        .find(|d| d.kind == HarnessKind::OpenCode)
        .expect("opencode descriptor");
    assert!(opencode.capabilities.harness_managed_providers);
    assert_eq!(
        opencode.implementation_status.as_str(),
        "available",
        "OpenCode execution is covered by tests/opencode_e2e.rs against v2.3.0"
    );
}

#[tokio::test]
async fn opencode_preflight_reports_unavailable_binary() {
    let registry = HarnessRegistry::builtin();
    let adapter: Arc<dyn orbit_harness::HarnessAdapter> =
        registry.require(HarnessKind::OpenCode).unwrap();
    let health = adapter
        .health(&orbit_contracts::HarnessConfig {
            kind: HarnessKind::OpenCode,
            settings: serde_json::json!({ "binary": "definitely-not-installed-opencode" }),
        })
        .await
        .expect("health returns degraded rather than failing");
    assert!(!health.healthy);
}

/// A continuous runtime resumes from the session of its previous run, so a run that
/// is cancelled must still say which session it started.
#[tokio::test]
async fn cancelled_mock_run_keeps_its_session() {
    let registry = HarnessRegistry::builtin();
    let adapter = registry.require(HarnessKind::Mock).unwrap();
    let mut request = orbit_harness::testing::execution_request(adapter.as_ref(), "slow task");
    // Long enough that the cancel below always lands mid-run.
    request.harness_config = serde_json::json!({ "stepDelayMs": 2000 });
    let cancel = request.cancel.clone();
    let (events, mut received) = tokio::sync::mpsc::channel(64);
    request.events = Some(events);

    let run = tokio::spawn(async move { adapter.execute(request).await });
    // The first event is `started`: the session exists from here on.
    let first = tokio::time::timeout(std::time::Duration::from_secs(5), received.recv())
        .await
        .expect("the mock reports that it started")
        .expect("an event");
    cancel.cancel();
    let outcome = run.await.unwrap().unwrap();

    assert_eq!(outcome.status, orbit_domain::ExecutionStatus::Cancelled);
    let session = outcome
        .session_ref
        .expect("a cancelled run keeps its session");
    assert!(
        serde_json::to_string(&first).unwrap().contains(&session),
        "the outcome names the session the run announced"
    );
}
