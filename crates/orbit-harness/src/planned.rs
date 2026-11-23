use async_trait::async_trait;

use orbit_contracts::{
    ExecutionOutcome, ExecutionRequest, HarnessAdapter, HarnessCapabilities, HarnessConfig,
    HarnessDescriptor, HarnessError, HarnessErrorKind, HarnessHealth,
};
use orbit_domain::{ExecutionMode, HarnessKind, ImplementationStatus, ValidationReport};

/// Adapter for harnesses that are registered (so agents can reference them and the UI can
/// show capabilities) but are not implemented in this build. Execution fails with a
/// precise, non-retryable error instead of pretending to work.
#[derive(Debug, Clone)]
pub struct DescriptorOnlyAdapter {
    descriptor: HarnessDescriptor,
}

impl DescriptorOnlyAdapter {
    pub fn new(descriptor: HarnessDescriptor) -> Self {
        Self { descriptor }
    }
}

pub fn claude_code() -> DescriptorOnlyAdapter {
    DescriptorOnlyAdapter::new(HarnessDescriptor {
        kind: HarnessKind::ClaudeCode,
        name: "Claude Code".into(),
        description: "Claude Code CLI harness (headless/programmatic). Adapter lands in Phase 2; \
             authentication and licensing are harness-managed."
            .into(),
        capabilities: HarnessCapabilities {
            supports_sessions: true,
            supports_resume: true,
            supports_streaming: true,
            supports_cancel: true,
            supports_instances: false,
            supports_tool_events: true,
            supports_usage_reporting: true,
            harness_managed_providers: true,
            supports_platform_providers: false,
            execution_modes: vec![ExecutionMode::OnDemand],
            max_timeout_seconds: 86_400,
            max_memory_mb: 16_384,
        },
        implementation_status: ImplementationStatus::Planned,
        config_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "binary": { "type": "string" },
                "permissionMode": { "type": "string", "enum": ["default", "acceptEdits", "plan", "bypassPermissions"] },
                "allowedTools": { "type": "array", "items": { "type": "string" } },
                "extraArgs": { "type": "array", "items": { "type": "string" } }
            }
        }),
        docs_url: Some("https://docs.anthropic.com/en/docs/claude-code".into()),
    })
}

pub fn generic() -> DescriptorOnlyAdapter {
    DescriptorOnlyAdapter::new(HarnessDescriptor {
        kind: HarnessKind::Generic,
        name: "Generic harness".into(),
        description: "Bring-your-own harness speaking the ORBIT harness protocol (Phase 3+). \
             Requires an explicit protocol endpoint in harness config."
            .into(),
        capabilities: HarnessCapabilities {
            supports_sessions: true,
            supports_resume: false,
            supports_streaming: true,
            supports_cancel: true,
            supports_instances: true,
            supports_tool_events: true,
            supports_usage_reporting: false,
            harness_managed_providers: false,
            supports_platform_providers: true,
            execution_modes: vec![ExecutionMode::OnDemand, ExecutionMode::LongRunning],
            max_timeout_seconds: 86_400,
            max_memory_mb: 32_768,
        },
        implementation_status: ImplementationStatus::Planned,
        config_schema: serde_json::json!({
            "type": "object",
            "required": ["endpoint"],
            "properties": {
                "endpoint": { "type": "string" },
                "protocol": { "type": "string", "enum": ["http", "grpc"] }
            }
        }),
        docs_url: None,
    })
}

#[async_trait]
impl HarnessAdapter for DescriptorOnlyAdapter {
    fn kind(&self) -> HarnessKind {
        self.descriptor.kind
    }

    fn descriptor(&self) -> HarnessDescriptor {
        self.descriptor.clone()
    }

    async fn validate_config(&self, config: &HarnessConfig) -> ValidationReport {
        let mut report = ValidationReport::default();
        if config.kind != self.descriptor.kind {
            report.push("type", "harness kind does not match this adapter");
        }
        report
    }

    async fn health(&self, _config: &HarnessConfig) -> Result<HarnessHealth, HarnessError> {
        Ok(HarnessHealth::degraded(format!(
            "{} adapter is not implemented in this build",
            self.descriptor.name
        )))
    }

    async fn execute(&self, _request: ExecutionRequest) -> Result<ExecutionOutcome, HarnessError> {
        Err(not_implemented(self.descriptor.kind))
    }
}

fn not_implemented(kind: HarnessKind) -> HarnessError {
    HarnessError::new(
        HarnessErrorKind::Unsupported,
        format!(
            "the '{kind}' harness adapter is registered but not implemented in this build; \
             use the mock harness or implement the adapter in crates/orbit-harness"
        ),
    )
}
