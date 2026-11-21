use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use orbit_domain::{HarnessKind, ValidationReport};

/// Provider families. New kinds extend this enum plus a config struct; unrelated
/// modules are unaffected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// Any endpoint implementing the OpenAI chat-completions shape.
    #[serde(rename = "openai_compatible")]
    OpenAiCompatible,
    /// Anthropic's native Messages API.
    Anthropic,
    /// Provider inventory and credentials owned by the harness (OpenCode Go, Claude Code).
    HarnessManaged,
}

impl ProviderKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::OpenAiCompatible => "openai_compatible",
            Self::Anthropic => "anthropic",
            Self::HarnessManaged => "harness_managed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "openai_compatible" => Some(Self::OpenAiCompatible),
            "anthropic" => Some(Self::Anthropic),
            "harness_managed" => Some(Self::HarnessManaged),
            _ => None,
        }
    }
}

/// Explicit endpoint feature flags. "OpenAI-compatible" is not a guarantee.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCompat {
    #[serde(default = "default_true")]
    pub chat_completions: bool,
    #[serde(default)]
    pub responses_api: bool,
    #[serde(default)]
    pub tools: bool,
    #[serde(default = "default_true")]
    pub streaming: bool,
    #[serde(default)]
    pub json_mode: bool,
    #[serde(default)]
    pub vision: bool,
    #[serde(default)]
    pub embeddings: bool,
    #[serde(default = "default_true")]
    pub models_endpoint: bool,
}

fn default_true() -> bool {
    true
}

impl Default for ProviderCompat {
    fn default() -> Self {
        Self {
            chat_completions: true,
            responses_api: false,
            tools: false,
            streaming: true,
            json_mode: false,
            vision: false,
            embeddings: false,
            models_endpoint: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpenAiCompatibleConfig {
    pub base_url: String,
    pub model: String,
    /// Handle resolved by the backend secret store. Never a raw key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_ref: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub request_params: serde_json::Value,
    #[serde(default)]
    pub context_window: Option<u32>,
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
    #[serde(default)]
    pub compat: ProviderCompat,
    /// ORBIT metadata that is never sent to the endpoint, such as the model catalog
    /// (`settings.models`) that Test stores and model pickers read.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub settings: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnthropicConfig {
    #[serde(default = "default_anthropic_base_url")]
    pub base_url: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_ref: Option<String>,
    #[serde(default)]
    pub context_window: Option<u32>,
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
    /// ORBIT metadata that is never sent to the endpoint, such as the model catalog
    /// (`settings.models`) that Test stores and model pickers read.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub settings: serde_json::Value,
}

fn default_anthropic_base_url() -> String {
    "https://api.anthropic.com/v1".into()
}

/// Wire protocol a platform-resolved endpoint speaks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModelProtocol {
    #[default]
    OpenAiCompatible,
    AnthropicMessages,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HarnessManagedConfig {
    pub harness: HarnessKind,
    /// Provider name as understood by the harness (e.g. `opencode-go`). Not validated
    /// against a hardcoded inventory.
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub secret_ref: Option<String>,
    #[serde(default)]
    pub settings: serde_json::Value,
}

/// Validate a provider configuration document for the given kind.
pub fn validate_provider_config(
    kind: ProviderKind,
    config: &serde_json::Value,
) -> ValidationReport {
    let mut report = ValidationReport::default();
    match kind {
        ProviderKind::OpenAiCompatible => {
            match serde_json::from_value::<OpenAiCompatibleConfig>(config.clone()) {
                Ok(parsed) => {
                    if !(parsed.base_url.starts_with("http://")
                        || parsed.base_url.starts_with("https://"))
                    {
                        report.push("baseUrl", "must be an http(s) URL");
                    }
                    if parsed.base_url.ends_with('/') {
                        report.push("baseUrl", "must not end with a slash");
                    }
                    if parsed.model.trim().is_empty() {
                        report.push("model", "model id is required");
                    }
                    if let Some(window) = parsed.context_window {
                        if window < 128 {
                            report.push("contextWindow", "must be >= 128");
                        }
                    }
                    for name in parsed.headers.keys() {
                        if name.eq_ignore_ascii_case("authorization")
                            || name.eq_ignore_ascii_case("x-api-key")
                        {
                            report.push(
                                "headers",
                                format!("{name} must be configured as a credential, not a header"),
                            );
                        }
                    }
                }
                Err(err) => {
                    report.push("config", format!("invalid OpenAI-compatible config: {err}"))
                }
            }
        }
        ProviderKind::Anthropic => {
            match serde_json::from_value::<AnthropicConfig>(config.clone()) {
                Ok(parsed) => {
                    if !(parsed.base_url.starts_with("http://")
                        || parsed.base_url.starts_with("https://"))
                    {
                        report.push("baseUrl", "must be an http(s) URL");
                    }
                    if parsed.base_url.ends_with('/') {
                        report.push("baseUrl", "must not end with a slash");
                    }
                    if parsed.model.trim().is_empty() {
                        report.push("model", "model id is required");
                    }
                    if parsed.context_window.is_some_and(|window| window < 128) {
                        report.push("contextWindow", "must be >= 128");
                    }
                }
                Err(err) => report.push("config", format!("invalid Anthropic config: {err}")),
            }
        }
        ProviderKind::HarnessManaged => {
            match serde_json::from_value::<HarnessManagedConfig>(config.clone()) {
                Ok(parsed) => {
                    if let Some(provider) = &parsed.provider {
                        if provider.trim().is_empty() {
                            report.push("provider", "provider name must not be empty");
                        }
                    }
                }
                Err(err) => report.push("config", format!("invalid harness-managed config: {err}")),
            }
        }
    }
    report
}

/// How the agent selected its model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "management", rename_all = "snake_case")]
pub enum ModelSelection {
    /// Harness owns the model inventory and credentials.
    Harness {
        provider: Option<String>,
        model: Option<String>,
    },
    /// ORBIT resolves a configured provider into an endpoint.
    Platform {
        provider_id: Uuid,
        provider_name: String,
        model: String,
    },
}

/// Plaintext credential lease. `Debug` is redacted, serialization is refused.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretLease {
    pub secret_ref: String,
    value: String,
}

impl SecretLease {
    pub fn new(secret_ref: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            secret_ref: secret_ref.into(),
            value: value.into(),
        }
    }

    /// Exposes the credential. Only for injection into an isolated harness process.
    pub fn expose(&self) -> &str {
        &self.value
    }
}

impl fmt::Debug for SecretLease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretLease")
            .field("secret_ref", &self.secret_ref)
            .field("value", &"<redacted>")
            .finish()
    }
}

impl serde::Serialize for SecretLease {
    fn serialize<S: serde::Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom(
            "SecretLease must never be serialized",
        ))
    }
}

/// Endpoint handed to a harness that can consume an OpenAI-compatible API.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModelEndpoint {
    pub base_url: String,
    pub model: String,
    #[serde(default)]
    pub protocol: ModelProtocol,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub request_params: serde_json::Value,
    #[serde(default)]
    pub compat: ProviderCompat,
    #[serde(default)]
    pub context_window: Option<u32>,
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
}

/// Fully resolved model selection for one execution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedModel {
    pub selection: ModelSelection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<ModelEndpoint>,
    #[serde(skip, default)]
    pub credential: Option<SecretLease>,
}

impl ResolvedModel {
    pub fn harness_managed(provider: Option<String>, model: Option<String>) -> Self {
        Self {
            selection: ModelSelection::Harness { provider, model },
            endpoint: None,
            credential: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_configs_keep_the_model_catalog() {
        // Test on a saved provider stores the discovered models here; an edit that sends
        // the stored config back must still validate.
        for (kind, config) in [
            (
                ProviderKind::OpenAiCompatible,
                serde_json::json!({
                    "baseUrl": "https://gateway.example.com/v1",
                    "model": "qwen",
                    "settings": { "models": ["qwen", "llama"] }
                }),
            ),
            (
                ProviderKind::Anthropic,
                serde_json::json!({
                    "model": "claude-sonnet-4-5",
                    "settings": { "models": ["claude-sonnet-4-5"] }
                }),
            ),
        ] {
            let report = validate_provider_config(kind, &config);
            assert!(report.is_valid(), "{kind:?}: {report:?}");
        }
        let unknown =
            serde_json::json!({ "baseUrl": "https://g.example/v1", "model": "q", "extra": 1 });
        assert!(!validate_provider_config(ProviderKind::OpenAiCompatible, &unknown).is_valid());
    }

    #[test]
    fn rejects_secret_headers() {
        let config = serde_json::json!({
            "baseUrl": "https://gateway.example.com/v1",
            "model": "qwen",
            "headers": { "Authorization": "Bearer leaked" }
        });
        let report = validate_provider_config(ProviderKind::OpenAiCompatible, &config);
        assert!(!report.is_valid());
    }

    #[test]
    fn accepts_custom_endpoint() {
        let config = serde_json::json!({
            "baseUrl": "http://vllm.local:8000/v1",
            "model": "qwen3-coder",
            "contextWindow": 32768,
            "compat": { "tools": false }
        });
        let report = validate_provider_config(ProviderKind::OpenAiCompatible, &config);
        assert!(report.is_valid(), "{report:?}");
    }

    #[test]
    fn accepts_anthropic_native_provider() {
        let config = serde_json::json!({
            "baseUrl": "https://api.anthropic.com/v1",
            "model": "claude-sonnet-4-5"
        });
        let report = validate_provider_config(ProviderKind::Anthropic, &config);
        assert!(report.is_valid(), "{report:?}");
        assert_eq!(
            ProviderKind::parse("anthropic"),
            Some(ProviderKind::Anthropic)
        );
    }

    #[test]
    fn anthropic_defaults_base_url_and_requires_model() {
        let report =
            validate_provider_config(ProviderKind::Anthropic, &serde_json::json!({ "model": "" }));
        assert!(!report.is_valid());
    }

    #[test]
    fn secret_lease_is_redacted() {
        let lease = SecretLease::new("provider/abc", "sk-secret");
        assert!(!format!("{lease:?}").contains("sk-secret"));
    }
}
