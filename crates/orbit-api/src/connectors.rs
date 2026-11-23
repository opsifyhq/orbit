//! Built-in connector manifests.
//!
//! Phase 0 ships manifests and connection management. Capability invocation, OAuth
//! flows and webhook verification are implemented in Phase 2; manifests are already the
//! contract the platform and UI consume.

use orbit_contracts::{
    x_posts_create_input_schema, AuthMethod, CapabilityDescriptor, ConnectorManifest,
};
use orbit_domain::CapabilityEffect;

fn capability(
    id: &str,
    name: &str,
    description: &str,
    effect: CapabilityEffect,
) -> CapabilityDescriptor {
    CapabilityDescriptor {
        id: id.into(),
        name: name.into(),
        description: description.into(),
        effect,
        input_schema: serde_json::json!({ "type": "object" }),
        output_schema: serde_json::json!({ "type": "object" }),
    }
}

fn capability_with_input_schema(
    id: &str,
    name: &str,
    description: &str,
    effect: CapabilityEffect,
    input_schema: serde_json::Value,
) -> CapabilityDescriptor {
    CapabilityDescriptor {
        input_schema,
        ..capability(id, name, description, effect)
    }
}

pub fn builtin_manifests() -> Vec<ConnectorManifest> {
    vec![
        ConnectorManifest {
            slug: "mock".into(),
            name: "Mock connector".into(),
            version: "0.1.0".into(),
            description: "Deterministic connector for local development and tests.".into(),
            auth: vec![AuthMethod::None],
            webhooks: false,
            capabilities: vec![
                capability(
                    "notes.list",
                    "List notes",
                    "Returns demo notes.",
                    CapabilityEffect::Read,
                ),
                capability(
                    "notes.create",
                    "Create note",
                    "Creates a demo note.",
                    CapabilityEffect::Write,
                ),
            ],
            config_schema: serde_json::json!({ "type": "object", "properties": {} }),
            docs_url: None,
            builtin: true,
        },
        ConnectorManifest {
            slug: "telegram".into(),
            name: "Telegram".into(),
            version: "0.1.0".into(),
            description: "Telegram bot messaging (Phase 2 invocation).".into(),
            auth: vec![AuthMethod::ApiToken],
            webhooks: true,
            capabilities: vec![
                capability(
                    "messages.list",
                    "List updates",
                    "Reads recent bot updates.",
                    CapabilityEffect::Read,
                ),
                capability(
                    "messages.send",
                    "Send message",
                    "Sends a Telegram message.",
                    CapabilityEffect::ExternalSideEffect,
                ),
            ],
            config_schema: serde_json::json!({
                "type": "object",
                "properties": { "chatId": { "type": "string" } }
            }),
            docs_url: Some("https://core.telegram.org/bots/api".into()),
            builtin: true,
        },
        ConnectorManifest {
            slug: "github".into(),
            name: "GitHub".into(),
            version: "0.1.0".into(),
            description: "GitHub issues and repository actions, executed through ORBIT's policy-enforcing tool broker.".into(),
            auth: vec![AuthMethod::ApiToken],
            webhooks: false,
            capabilities: vec![
                capability("issues.list", "List issues", "Lists issues in a repository.", CapabilityEffect::Read),
                capability("issues.get", "Get issue", "Retrieves one issue with comments metadata.", CapabilityEffect::Read),
                capability("issues.comment", "Comment on issue", "Posts a comment to an issue.", CapabilityEffect::Write),
                capability("issues.create", "Create issue", "Creates a new issue.", CapabilityEffect::Write),
                capability("issues.update", "Update issue", "Updates title, body, state, labels or assignees.", CapabilityEffect::Write),
                capability("pulls.list", "List pull requests", "Lists pull requests in a repository.", CapabilityEffect::Read),
                capability("pulls.get", "Get pull request", "Retrieves one pull request.", CapabilityEffect::Read),
                capability("pulls.create", "Create pull request", "Creates a pull request from a branch.", CapabilityEffect::Write),
                capability("checks.list", "List checks", "Lists check runs for a commit or branch.", CapabilityEffect::Read),
                capability("repo.checkout", "Checkout repository", "Clones the configured repository into the execution workspace.", CapabilityEffect::Read),
                capability("repo.commit", "Commit changes", "Commits workspace changes on behalf of the agent.", CapabilityEffect::Write),
                capability("repo.push", "Push branch", "Pushes a workspace branch to the remote repository.", CapabilityEffect::ExternalSideEffect),
            ],
            config_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "baseUrl": { "type": "string", "default": "https://api.github.com" },
                    "allowedRepositories": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional owner/repo allowlist. Empty means any repository the token can access."
                    },
                    "defaultRepository": { "type": "string", "description": "owner/repo used when tool args omit it" }
                }
            }),
            docs_url: Some("https://docs.github.com/en/rest".into()),
            builtin: true,
        },
        ConnectorManifest {
            slug: "gitlab".into(),
            name: "GitLab".into(),
            version: "0.3.0".into(),
            description: "GitLab projects, group issue boards, issues, merge requests and pipelines.".into(),
            auth: vec![AuthMethod::OAuth2, AuthMethod::ApiToken],
            webhooks: true,
            capabilities: vec![
                capability("projects.list", "List projects", "Lists accessible projects.", CapabilityEffect::Read),
                capability("group_issues.list", "List group issues", "Lists issues across a GitLab group.", CapabilityEffect::Read),
                capability("boards.list", "List group boards", "Lists issue boards for a GitLab group.", CapabilityEffect::Read),
                capability("issues.list", "List issues", "Lists project issues.", CapabilityEffect::Read),
                capability("issues.get", "Get issue", "Retrieves one issue.", CapabilityEffect::Read),
                capability("issues.create", "Create issue", "Creates an issue.", CapabilityEffect::Write),
                capability("issues.update", "Update issue", "Updates issue metadata, labels or state.", CapabilityEffect::Write),
                capability("issues.comment", "Comment on issue", "Posts a note on an issue.", CapabilityEffect::Write),
                capability("merge_requests.list", "List merge requests", "Lists merge requests.", CapabilityEffect::Read),
                capability("merge_requests.get", "Get merge request", "Reads one merge request.", CapabilityEffect::Read),
                capability("merge_requests.create", "Create merge request", "Opens a merge request.", CapabilityEffect::ExternalSideEffect),
                capability("merge_requests.comment", "Comment on merge request", "Posts a note on a merge request.", CapabilityEffect::Write),
                capability("merge_requests.approve", "Approve merge request", "Approves a merge request.", CapabilityEffect::ExternalSideEffect),
                capability("merge_requests.merge", "Merge merge request", "Merges a merge request.", CapabilityEffect::ExternalSideEffect),
                capability("pipelines.status", "Pipeline status", "Reads pipeline status.", CapabilityEffect::Read),
                capability("repo.checkout", "Checkout repository", "Clones the configured GitLab project.", CapabilityEffect::Read),
                capability("repo.commit", "Commit changes", "Commits workspace changes.", CapabilityEffect::Write),
                capability("repo.push", "Push branch", "Pushes a branch to the GitLab project.", CapabilityEffect::ExternalSideEffect),
            ],
            config_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "baseUrl": { "type": "string", "default": "https://gitlab.com/api/v4" },
                    "cloneBaseUrl": { "type": "string", "default": "https://gitlab.com" },
                    "allowedProjects": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional project path (or id) allowlist for project tools and projects.list. Empty means any project the token can access."
                    },
                    "defaultProject": { "type": "string", "description": "Project path used when tool args omit it" },
                    "groupId": { "type": "string", "description": "Group path for group issues and boards; group tools may only use this group and allowedGroups" },
                    "allowedGroups": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional further group paths that group tools may name. Without groupId or allowedGroups, group tools are refused."
                    },
                    "boardId": { "type": "integer", "description": "Optional group board id" }
                }
            }),
            docs_url: Some("https://docs.gitlab.com/ee/api/".into()),
            builtin: true,
        },
        ConnectorManifest {
            slug: "kubernetes".into(),
            name: "Kubernetes".into(),
            version: "0.2.0".into(),
            description: "Read-only Kubernetes inspection: namespaces, pods, logs, deployments and events.".into(),
            auth: vec![AuthMethod::ApiToken],
            webhooks: false,
            capabilities: vec![
                capability("namespaces.list", "List namespaces", "Lists namespaces allowed on the connection.", CapabilityEffect::Read),
                capability("pods.list", "List pods", "Lists pods in a namespace with optional selectors.", CapabilityEffect::Read),
                capability("pods.get", "Get pod", "Reads one pod.", CapabilityEffect::Read),
                capability("pods.logs", "Pod logs", "Reads bounded pod logs (current or previous).", CapabilityEffect::Read),
                capability("deployments.list", "List deployments", "Lists deployments in a namespace.", CapabilityEffect::Read),
                capability("deployments.get", "Get deployment", "Reads one deployment.", CapabilityEffect::Read),
                capability(
                    "deployments.restart",
                    "Restart deployment",
                    "Triggers a rolling restart by updating the pod template's restartedAt annotation.",
                    CapabilityEffect::ExternalSideEffect,
                ),
                capability(
                    "pods.delete",
                    "Delete pod",
                    "Deletes one pod; its Deployment/ReplicaSet recreates it. Use for a stuck pod.",
                    CapabilityEffect::ExternalSideEffect,
                ),
                capability("events.list", "List events", "Lists namespace events.", CapabilityEffect::Read),
            ],
            config_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "authMode": { "type": "string", "default": "service_account", "description": "service_account (token) or kubeconfig" },
                    "apiServer": { "type": "string", "default": "https://kubernetes.default.svc", "description": "Required for service_account; ignored for kubeconfig" },
                    "context": { "type": "string", "description": "Kubeconfig context (defaults to current-context)" },
                    "namespaces": { "type": "array", "items": { "type": "string" }, "description": "Namespace allowlist. Empty allows only defaultNamespace; without either, namespaced tools are refused" },
                    "defaultNamespace": { "type": "string", "description": "Namespace used when a tool call omits it" },
                    "allowedResources": { "type": "array", "items": { "type": "string" }, "description": "Resource allowlist: pods, pods/log, deployments, events, namespaces, or * for all. Empty denies every tool" },
                    "caCert": { "type": "string", "description": "Public CA certificate PEM (optional)" },
                    "timeoutSeconds": { "type": "integer", "default": 20 },
                    "maxLogBytes": { "type": "integer", "default": 200000 }
                }
            }),
            docs_url: Some("https://kubernetes.io/docs/reference/kubernetes-api/".into()),
            builtin: true,
        },
        ConnectorManifest {
            slug: "x".into(),
            name: "X".into(),
            version: "0.2.0".into(),
            description: "X/Twitter: search recent posts and publish posts with a user-context token.".into(),
            auth: vec![AuthMethod::OAuth2, AuthMethod::ApiToken],
            webhooks: false,
            capabilities: vec![
                capability(
                    "posts.search",
                    "Search posts",
                    "Searches recent posts during context hydration. An app-only bearer token is sufficient.",
                    CapabilityEffect::Read,
                ),
                capability_with_input_schema(
                    "posts.create",
                    "Publish a post",
                    "Publishes a post or reply. Requires a user-context access token with tweet.write; app-only bearer tokens cannot post.",
                    CapabilityEffect::ExternalSideEffect,
                    x_posts_create_input_schema(),
                ),
            ],
            config_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "baseUrl": {
                        "type": "string",
                        "default": "https://api.x.com/2",
                        "description": "X API v2 base URL. The stored credential is sent as a user-context bearer token."
                    },
                    "query": {
                        "type": "string",
                        "description": "Default query for posts.search hydration."
                    }
                }
            }),
            docs_url: Some("https://developer.x.com/en/docs".into()),
            builtin: true,
        },
        ConnectorManifest {
            slug: "webhook".into(),
            name: "Generic webhook".into(),
            version: "0.1.0".into(),
            description: "Send and receive signed JSON webhooks.".into(),
            auth: vec![AuthMethod::HmacWebhook, AuthMethod::None],
            webhooks: true,
            capabilities: vec![capability(
                "request.send",
                "Send request",
                "Sends an outbound HTTP request.",
                CapabilityEffect::ExternalSideEffect,
            )],
            config_schema: serde_json::json!({
                "type": "object",
                "properties": { "url": { "type": "string" } }
            }),
            docs_url: None,
            builtin: true,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x_advertises_search_and_user_context_posting() {
        let x = builtin_manifests()
            .into_iter()
            .find(|manifest| manifest.slug == "x")
            .expect("X manifest");
        assert_eq!(
            x.capability("posts.search").unwrap().effect,
            CapabilityEffect::Read
        );
        let create = x
            .capability("posts.create")
            .expect("posts.create is implemented");
        assert_eq!(create.effect, CapabilityEffect::ExternalSideEffect);
        assert_eq!(create.input_schema, x_posts_create_input_schema());
        assert!(create.description.contains("user-context"));
        assert!(create.description.contains("tweet.write"));
    }
}
