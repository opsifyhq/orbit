use serde::{Deserialize, Serialize};

use orbit_domain::CapabilityEffect;

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
