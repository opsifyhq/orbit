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
