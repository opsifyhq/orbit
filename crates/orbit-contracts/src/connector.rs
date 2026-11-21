use serde::{Deserialize, Serialize};
use uuid::Uuid;

use orbit_domain::CapabilityEffect;

use crate::provider::SecretLease;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    OAuth2,
    ApiToken,
    Basic,
    HmacWebhook,
    None,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityDescriptor {
    /// Stable capability id: `resource.action`, e.g. `issues.create`.
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub effect: CapabilityEffect,
    #[serde(default)]
    pub input_schema: serde_json::Value,
    #[serde(default)]
    pub output_schema: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorManifest {
    pub slug: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub auth: Vec<AuthMethod>,
    #[serde(default)]
    pub webhooks: bool,
    #[serde(default)]
    pub capabilities: Vec<CapabilityDescriptor>,
    /// JSON schema for connection configuration.
    #[serde(default)]
    pub config_schema: serde_json::Value,
    #[serde(default)]
    pub docs_url: Option<String>,
    /// Built-in connectors ship with ORBIT and cannot be deleted.
    #[serde(default)]
    pub builtin: bool,
}

impl ConnectorManifest {
    pub fn capability(&self, id: &str) -> Option<&CapabilityDescriptor> {
        self.capabilities.iter().find(|c| c.id == id)
    }

    pub fn read_capabilities(&self) -> impl Iterator<Item = &CapabilityDescriptor> {
        self.capabilities
            .iter()
            .filter(|c| c.effect == CapabilityEffect::Read)
    }
}

/// JSON Schema for the arguments of the built-in X `posts.create` capability.
///
/// Both the connector manifest (what the UI advertises) and the tool broker (what the
/// harness may actually call) use this schema so the two cannot drift. Posting needs a
/// user-context token; app-only bearer tokens can search but cannot create posts.
/// A reply may be expressed as `replyTo` or as `reply.in_reply_to_tweet_id`.
pub fn x_posts_create_input_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "text": {
                "type": "string",
                "minLength": 1,
                "maxLength": 280,
                "description": "Post text, at most 280 X-weighted characters (URLs count as 23)."
            },
            "replyTo": {
                "type": "string",
                "pattern": "^[0-9]{1,19}$",
                "description": "Tweet ID to reply to; alternative to reply.in_reply_to_tweet_id."
            },
            "reply": {
                "type": "object",
                "properties": {
                    "in_reply_to_tweet_id": {
                        "type": "string",
                        "pattern": "^[0-9]{1,19}$"
                    }
                }
            }
        },
        "required": ["text"]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x_posts_create_schema_requires_text_and_allows_replies() {
        let schema = x_posts_create_input_schema();
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["required"], serde_json::json!(["text"]));
        assert_eq!(schema["properties"]["text"]["maxLength"], 280);
        assert!(schema["properties"]["replyTo"].is_object());
        assert!(schema["properties"]["reply"]["properties"]["in_reply_to_tweet_id"].is_object());
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityCall {
    pub capability: String,
    #[serde(default)]
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityResult {
    pub data: serde_json::Value,
    #[serde(default)]
    pub external_refs: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ConnectorContext {
    pub connection_id: Uuid,
    pub organization_id: Uuid,
    pub config: serde_json::Value,
    pub credential: Option<SecretLease>,
    /// Platform-approved capabilities for this invocation.
    pub grants: Vec<String>,
}

impl ConnectorContext {
    pub fn is_granted(&self, capability: &str) -> bool {
        self.grants.iter().any(|c| c == capability)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorEvent {
    pub connector: String,
    pub connection_id: Uuid,
    pub event_type: String,
    #[serde(default)]
    pub subject: Option<String>,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorError {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub retryable: bool,
}

impl ConnectorError {
    pub fn denied(capability: &str) -> Self {
        Self {
            code: "capability_not_granted".into(),
            message: format!("capability {capability} is not granted to this connection"),
            retryable: false,
        }
    }

    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable: false,
        }
    }
}

impl std::fmt::Display for ConnectorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ConnectorError {}

/// Connector implementation contract. Connectors never see raw model output directly;
/// they receive platform-approved capability calls.
#[async_trait::async_trait]
pub trait Connector: Send + Sync {
    fn manifest(&self) -> &ConnectorManifest;

    async fn validate_connection(&self, context: &ConnectorContext) -> Result<(), ConnectorError>;

    async fn invoke(
        &self,
        context: ConnectorContext,
        call: CapabilityCall,
    ) -> Result<CapabilityResult, ConnectorError>;

    /// Verify an inbound webhook and normalize it. Returning `None` means the payload
    /// was accepted but no interesting event was produced.
    async fn verify_webhook(
        &self,
        headers: &http::HeaderMap,
        body: &[u8],
        secret: Option<&SecretLease>,
    ) -> Result<Option<ConnectorEvent>, ConnectorError> {
        let _ = (headers, body, secret);
        Err(ConnectorError::new(
            "unsupported",
            "this connector does not accept webhooks",
        ))
    }
}
