//! Portable, secret-free resource format shared by export and import.
//!
//! Every shareable ORBIT resource is exported as a YAML (or JSON) *envelope*:
//!
//! ```yaml
//! apiVersion: orbit.io/v1
//! kind: Connection
//! metadata:
//!   name: team-github
//! spec: { ... }
//! credentials:
//!   required: true
//!   note: Credentials are never exported; set them after import.
//! ```
//!
//! Rules enforced here (and relied upon by `orbit-api`):
//!
//! * References between resources use **names/slugs**, never database ids, so a
//!   document is meaningful on another installation.
//! * Secrets and credential references are never part of the format. [`redact`]
//!   removes them recursively on export and [`strip_secrets`] drops them again on
//!   import, so a hand-edited file cannot smuggle a `secretRef` into the platform.
//! * Skills additionally have a Markdown form (`SKILL.md`): YAML frontmatter plus
//!   the instruction body.
//! * A *bundle* is a multi-document YAML stream (documents separated by `---`) or a
//!   JSON array of envelopes.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use orbit_domain::ValidationIssue;

/// Envelope `apiVersion` accepted and produced by this build.
pub const PORTABLE_API_VERSION: &str = orbit_domain::AGENT_API_VERSION;
/// Value written for environment variables whose value must be supplied on import.
pub const VALUE_PLACEHOLDER: &str = "<set-on-import>";
/// Human readable note stored in the `credentials` block of exported documents.
pub const CREDENTIALS_NOTE: &str =
    "Credentials are never exported. Set them on the imported resource after import.";
/// Maximum number of documents accepted in one bundle.
pub const MAX_BUNDLE_DOCUMENTS: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum PortableKind {
    Provider,
    Skill,
    Connection,
    WorkerProfile,
    Agent,
    AgentTeam,
    Trigger,
    ToolPolicy,
}

impl PortableKind {
    /// In dependency order: referenced kinds come before the kinds that reference them.
    pub const ALL: [PortableKind; 8] = [
        PortableKind::Provider,
        PortableKind::Skill,
        PortableKind::Connection,
        PortableKind::Agent,
        PortableKind::WorkerProfile,
        PortableKind::AgentTeam,
        PortableKind::Trigger,
        PortableKind::ToolPolicy,
    ];

    /// Envelope `kind` value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Provider => "Provider",
            Self::Skill => "Skill",
            Self::Connection => "Connection",
            Self::WorkerProfile => "WorkerProfile",
            Self::Agent => "Agent",
            Self::AgentTeam => "AgentTeam",
            Self::Trigger => "Trigger",
            Self::ToolPolicy => "ToolPolicy",
        }
    }

    /// Resource collection name used by the export route and ownership tables.
    pub fn resource(self) -> &'static str {
        match self {
            Self::Provider => "providers",
            Self::Skill => "skills",
            Self::Connection => "connections",
            Self::WorkerProfile => "worker_profiles",
            Self::Agent => "agents",
            Self::AgentTeam => "agent_teams",
            Self::Trigger => "triggers",
            Self::ToolPolicy => "tool_policies",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str().eq_ignore_ascii_case(value.trim()))
    }

    pub fn from_resource(resource: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.resource() == resource)
    }

    /// Position in [`Self::ALL`]; bundles are imported in this order.
    pub fn import_rank(self) -> usize {
        Self::ALL
            .iter()
            .position(|kind| *kind == self)
            .unwrap_or(usize::MAX)
    }
}

impl std::fmt::Display for PortableKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PortableMetadata {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CredentialsNotice {
    pub required: bool,
    pub note: String,
}

/// The generic envelope. `spec` is validated against the typed spec of `kind`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PortableDocument {
    pub api_version: String,
    pub kind: String,
    pub metadata: PortableMetadata,
    #[serde(default)]
    pub spec: Value,
    /// Informational only: ignored on import. Credentials are set after import.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials: Option<CredentialsNotice>,
}

impl PortableDocument {
    pub fn new(kind: PortableKind, name: impl Into<String>, spec: Value) -> Self {
        Self {
            api_version: PORTABLE_API_VERSION.to_string(),
            kind: kind.as_str().to_string(),
            metadata: PortableMetadata {
                name: name.into(),
                ..Default::default()
            },
            spec,
            credentials: None,
        }
    }

    pub fn with_credentials(mut self, required: bool) -> Self {
        self.credentials = Some(CredentialsNotice {
            required,
            note: CREDENTIALS_NOTE.to_string(),
        });
        self
    }

    pub fn portable_kind(&self) -> Option<PortableKind> {
        PortableKind::parse(&self.kind)
    }

    /// Render as YAML with a leading comment that repeats the credential notice.
    pub fn to_yaml(&self) -> Result<String, String> {
        let body = serde_yaml::to_string(self).map_err(|e| format!("could not serialize: {e}"))?;
        let header = format!(
            "# ORBIT portable {kind} ({version}). Secret-free export.\n# {note}\n",
            kind = self.kind,
            version = self.api_version,
            note = CREDENTIALS_NOTE
        );
        Ok(format!("{header}{body}"))
    }
}

// ----------------------------------------------------------------------- specs

fn default_true() -> bool {
    true
}

fn default_priority() -> i32 {
    100
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillSpec {
    pub slug: String,
    pub content: String,
    /// Relative path -> file content (scripts, references).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub files: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectionSpec {
    /// Connector slug (for example `github`).
    pub connector: String,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default)]
    pub config: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderSpec {
    pub kind: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub config: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerProfileSpec {
    pub harness_kind: String,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub execution_driver: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub image_tag: Option<String>,
    #[serde(default)]
    pub agent_scope: Option<String>,
    /// Agent names the profile is restricted to.
    #[serde(default)]
    pub agents: Vec<String>,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// Environment variable names. Values are never exported: they hold
    /// [`VALUE_PLACEHOLDER`] and are imported as empty strings.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Names of reusable environment mappings attached to the profile.
    #[serde(default)]
    pub env_mappings: Vec<String>,
    #[serde(default)]
    pub resources: Value,
    #[serde(default)]
    pub allow_web_search: bool,
    #[serde(default)]
    pub allow_web_fetch: bool,
    #[serde(default)]
    pub allow_glab: bool,
    #[serde(default)]
    pub allow_gh: bool,
    #[serde(default)]
    pub native_tool_permissions: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentTeamStepSpec {
    /// Agent name.
    pub agent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentTeamSpec {
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub instructions: String,
    #[serde(default)]
    pub repository: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub model: Option<Value>,
    /// Work source with `connection: <connector>/<name>` instead of `connectionId`.
    #[serde(default)]
    pub work_source: Option<Value>,
    /// Repositories with `connection: <connector>/<name>` instead of `connectionId`.
    #[serde(default)]
    pub repositories: Vec<Value>,
    /// Worker profile name used for automatic launch.
    #[serde(default)]
    pub worker_profile: Option<String>,
    #[serde(default)]
    pub auto_launch: bool,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub harness: Option<String>,
    /// Connections as `<connector>/<name>`.
    #[serde(default)]
    pub connections: Vec<String>,
    #[serde(default)]
    pub env_mappings: Vec<String>,
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    /// Optional explicit team autonomy policy.
    #[serde(default)]
    pub autonomy: Option<Value>,
    pub steps: Vec<AgentTeamStepSpec>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TriggerPortableSpec {
    /// Agent name.
    pub agent: String,
    /// `schedule`, `event`, `webhook` or `manual`.
    pub kind: String,
    #[serde(default)]
    pub harness: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub config: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolPolicySpec {
    /// Agent name; omitted for organisation-wide rules.
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default = "default_priority")]
    pub priority: i32,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub action: String,
    #[serde(default)]
    pub matcher: Value,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub impact: Option<String>,
}

// ---------------------------------------------------------------------- import

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ImportMode {
    /// Only create. A name conflict is an error (409). Default: no silent overwrite.
    #[default]
    Create,
    /// Only update an existing resource; a missing one is an error.
    Update,
    /// Create when missing, update when present.
    Upsert,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ImportAction {
    Create,
    Update,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportItemStatus {
    Ok,
    Conflict,
    Invalid,
    UnresolvedReferences,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UnresolvedReference {
    /// Referenced kind, for example `skill`, `agent`, `connection`, `connector`.
    pub kind: String,
    /// The name/slug as written in the document.
    pub reference: String,
    /// Where it was found, for example `spec.skills[1]`.
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImportItemReport {
    pub kind: String,
    pub name: String,
    pub status: ImportItemStatus,
    /// What an apply would do (or did). Absent when the item cannot be applied.
    pub action: Option<ImportAction>,
    /// Id of the created/updated resource. Only set after a real import.
    pub id: Option<uuid::Uuid>,
    pub unresolved_references: Vec<UnresolvedReference>,
    pub issues: Vec<ValidationIssue>,
    /// Non-blocking notes: ignored fields, credentials to set, dropped secrets.
    pub warnings: Vec<String>,
    pub credentials_required: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub dry_run: bool,
    pub mode: ImportMode,
    /// True when every item can be (or was) applied.
    pub valid: bool,
    /// True when changes were written.
    pub applied: bool,
    pub items: Vec<ImportItemReport>,
}

// -------------------------------------------------------------------- redaction

/// Keys (normalised: lowercase, no `-`/`_`) that never leave the platform.
const SENSITIVE_KEYS: [&str; 21] = [
    "token",
    "secret",
    "password",
    "passphrase",
    "credential",
    "apikey",
    "authorization",
    "privatekey",
    "signingkey",
    "clientsecret",
    "webhooksecret",
    "webhookurl",
    "authsecretname",
    "secretref",
    "bearer",
    "cookie",
    "sessionkey",
    "dsn",
    "connectionstring",
    "accesskey",
    "secretkey",
];

pub fn sensitive_key(key: &str) -> bool {
    let mut key = key.to_ascii_lowercase().replace(['-', '_'], "");
    // Token *budgets* are model parameters, not credentials (`maxTokens`, ...).
    for benign in ["tokens", "tokenlimit", "tokenbudget", "tokencount"] {
        key = key.replace(benign, "");
    }
    SENSITIVE_KEYS.iter().any(|needle| key.contains(needle))
}

/// True for string values that look like credentials regardless of the key they
/// are stored under.
pub fn secret_like_value(text: &str) -> bool {
    let trimmed = text.trim();
    const PREFIXES: [&str; 12] = [
        "orbit_sk_",
        "orbit_wk_",
        "ghp_",
        "gho_",
        "ghs_",
        "github_pat_",
        "glpat-",
        "xoxb-",
        "xoxp-",
        "sk-",
        "-----BEGIN",
        "https://hooks.slack.com/",
    ];
    if PREFIXES.iter().any(|prefix| trimmed.starts_with(prefix)) {
        return true;
    }
    // Basic-auth credentials embedded in a URL: scheme://user:pass@host
    if let Some((_, rest)) = trimmed.split_once("://") {
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        if let Some((userinfo, _)) = authority.rsplit_once('@') {
            if userinfo.contains(':') {
                return true;
            }
        }
    }
    false
}

/// Remove sensitive keys and secret-looking values from `value`, recording the
/// JSON path of everything removed in `removed`.
pub fn strip_secrets(value: &mut Value, path: &str, removed: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            let keys: Vec<String> = map.keys().cloned().collect();
            for key in keys {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                if sensitive_key(&key) {
                    map.remove(&key);
                    removed.push(child_path);
                } else if let Some(child) = map.get_mut(&key) {
                    strip_secrets(child, &child_path, removed);
                }
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter_mut().enumerate() {
                strip_secrets(item, &format!("{path}[{index}]"), removed);
            }
        }
        Value::String(text) if secret_like_value(text) => {
            *text = "[redacted]".into();
            removed.push(path.to_string());
        }
        _ => {}
    }
}

/// Recursively remove secrets and credential references.
pub fn redact(value: &mut Value) {
    strip_secrets(value, "", &mut Vec::new());
}

// ---------------------------------------------------------------------- parsing

/// Result of parsing an upload.
#[derive(Debug, Clone)]
pub struct ParsedBundle {
    pub documents: Vec<PortableDocument>,
}

fn issue(path: impl Into<String>, message: impl Into<String>) -> ValidationIssue {
    ValidationIssue::new(path, message)
}

/// Split `SKILL.md` style content into (frontmatter, body). The body is returned
/// untouched apart from newline normalisation.
pub fn split_frontmatter(content: &str) -> Option<(serde_yaml::Value, String)> {
    let normalized = content.replace("\r\n", "\n");
    let rest = normalized.strip_prefix("---\n")?;
    let end = rest.find("\n---")?;
    let (yaml, tail) = rest.split_at(end);
    let tail = tail.strip_prefix("\n---")?;
    // The closing fence must end its line.
    let body = match tail.strip_prefix('\n') {
        Some(body) => body,
        None if tail.is_empty() => "",
        None => return None,
    };
    let frontmatter = serde_yaml::from_str(yaml).ok()?;
    Some((frontmatter, body.to_string()))
}

/// Render a skill as `SKILL.md`.
pub fn skill_to_markdown(slug: &str, name: &str, description: &str, content: &str) -> String {
    let mut front = serde_yaml::Mapping::new();
    front.insert("name".into(), name.into());
    front.insert("slug".into(), slug.into());
    front.insert("description".into(), description.into());
    let yaml = serde_yaml::to_string(&front).unwrap_or_default();
    format!("---\n{yaml}---\n{content}")
}

fn markdown_to_document(frontmatter: &serde_yaml::Value, body: String) -> PortableDocument {
    let text = |key: &str| {
        frontmatter
            .get(key)
            .and_then(|value| value.as_str())
            .map(str::to_string)
    };
    let mut doc = PortableDocument::new(
        PortableKind::Skill,
        text("name").unwrap_or_default(),
        serde_json::json!({ "slug": text("slug").unwrap_or_default(), "content": body }),
    );
    doc.metadata.description = text("description").unwrap_or_default();
    doc
}

fn document_from_value(
    index: usize,
    value: Value,
) -> Result<PortableDocument, Vec<ValidationIssue>> {
    let prefix = format!("documents[{index}]");
    if !value.is_object() {
        return Err(vec![issue(prefix, "each document must be a mapping")]);
    }
    let kind = value.get("kind").and_then(Value::as_str).unwrap_or("");
    if PortableKind::parse(kind).is_none() {
        return Err(vec![issue(
            format!("{prefix}.kind"),
            format!(
                "unsupported kind '{kind}', expected one of: {}",
                PortableKind::ALL.map(|k| k.as_str()).join(", ")
            ),
        )]);
    }
    let document: PortableDocument = serde_json::from_value(value)
        .map_err(|error| vec![issue(prefix.clone(), error.to_string())])?;
    let mut issues = Vec::new();
    if document.api_version != PORTABLE_API_VERSION {
        issues.push(issue(
            format!("{prefix}.apiVersion"),
            format!("unsupported apiVersion, expected {PORTABLE_API_VERSION}"),
        ));
    }
    if document.metadata.name.trim().is_empty() {
        issues.push(issue(format!("{prefix}.metadata.name"), "name is required"));
    }
    if issues.is_empty() {
        Ok(document)
    } else {
        Err(issues)
    }
}

/// Parse an upload: a `SKILL.md`, a single YAML/JSON envelope or a bundle.
pub fn parse_upload(text: &str, filename: &str) -> Result<ParsedBundle, Vec<ValidationIssue>> {
    let trimmed = text.trim_start_matches('\u{feff}').trim_start();
    if trimmed.is_empty() {
        return Err(vec![issue("content", "file is empty")]);
    }
    let lower = filename.to_ascii_lowercase();
    let looks_markdown = lower.ends_with(".md") || lower.ends_with(".markdown");

    // Markdown skill: frontmatter without an envelope `kind`.
    if let Some((frontmatter, body)) = split_frontmatter(trimmed) {
        let has_envelope =
            frontmatter.get("apiVersion").is_some() || frontmatter.get("kind").is_some();
        if !has_envelope && (looks_markdown || frontmatter.get("name").is_some()) {
            if body.trim().is_empty() {
                return Err(vec![issue("content", "SKILL.md body is empty")]);
            }
            let mut document = markdown_to_document(&frontmatter, body);
            if document.metadata.name.trim().is_empty() {
                let stem = lower
                    .rsplit('/')
                    .next()
                    .map(|file| file.trim_end_matches(".markdown").trim_end_matches(".md"))
                    .filter(|stem| !stem.is_empty() && *stem != "skill");
                document.metadata.name = stem.unwrap_or_default().replace(['-', '_'], " ");
            }
            if document.metadata.name.trim().is_empty() {
                return Err(vec![issue(
                    "metadata.name",
                    "SKILL.md frontmatter `name` is required",
                )]);
            }
            let slug_missing = document
                .spec
                .get("slug")
                .and_then(Value::as_str)
                .unwrap_or("")
                .is_empty();
            if slug_missing {
                document.spec["slug"] = Value::String(slugify(&document.metadata.name));
            }
            return Ok(ParsedBundle {
                documents: vec![document],
            });
        }
    }
    if looks_markdown {
        return Err(vec![issue(
            "content",
            "Markdown skills need YAML frontmatter with at least `name`",
        )]);
    }

    let mut values: Vec<Value> = Vec::new();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        let parsed: Value = serde_json::from_str(trimmed)
            .map_err(|error| vec![issue("content", format!("invalid JSON: {error}"))])?;
        match parsed {
            Value::Array(items) => values.extend(items),
            other => values.push(other),
        }
    } else {
        for (index, document) in serde_yaml::Deserializer::from_str(trimmed).enumerate() {
            let value: serde_yaml::Value =
                serde::Deserialize::deserialize(document).map_err(|error| {
                    vec![issue(
                        format!("documents[{index}]"),
                        format!("invalid YAML: {error}"),
                    )]
                })?;
            if value.is_null() {
                continue;
            }
            let json = serde_json::to_value(value)
                .map_err(|error| vec![issue(format!("documents[{index}]"), error.to_string())])?;
            values.push(json);
        }
    }
    if values.is_empty() {
        return Err(vec![issue("content", "no documents found")]);
    }
    if values.len() > MAX_BUNDLE_DOCUMENTS {
        return Err(vec![issue(
            "content",
            format!("a bundle may contain at most {MAX_BUNDLE_DOCUMENTS} documents"),
        )]);
    }
    let mut documents = Vec::with_capacity(values.len());
    let mut issues = Vec::new();
    for (index, value) in values.into_iter().enumerate() {
        match document_from_value(index, value) {
            Ok(document) => documents.push(document),
            Err(found) => issues.extend(found),
        }
    }
    if issues.is_empty() {
        Ok(ParsedBundle { documents })
    } else {
        Err(issues)
    }
}

pub fn slugify(value: &str) -> String {
    let mut slug = String::new();
    for ch in value.to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_sensitive_keys_and_secret_values_recursively() {
        let mut value = serde_json::json!({
            "name": "safe",
            "apiKey": "abc",
            "nested": { "password": "x", "tokenValue": "y", "keep": 1, "secretRef": "enc:1" },
            "list": [{ "authSecretName": "k8s", "value": "orbit_sk_123" }],
            "url": "https://user:hunter2@example.com/x",
            "hook": "https://hooks.slack.com/services/T/B/X",
            "ok": "https://example.com/x"
        });
        let mut removed = Vec::new();
        strip_secrets(&mut value, "", &mut removed);
        assert_eq!(value["name"], "safe");
        assert_eq!(value["nested"]["keep"], 1);
        assert_eq!(value["ok"], "https://example.com/x");
        assert!(value.get("apiKey").is_none());
        assert!(value["nested"].get("secretRef").is_none());
        assert!(value["list"][0].get("authSecretName").is_none());
        assert_eq!(value["list"][0]["value"], "[redacted]");
        assert_eq!(value["url"], "[redacted]");
        assert_eq!(value["hook"], "[redacted]");
        assert!(removed.contains(&"nested.password".to_string()));
        assert!(removed.contains(&"list[0].value".to_string()));
    }

    #[test]
    fn skill_markdown_round_trips() {
        let md = skill_to_markdown("docs", "Docs: writer", "Writes docs", "# Steps\n\nDo it.\n");
        let parsed = parse_upload(&md, "SKILL.md").unwrap();
        let doc = &parsed.documents[0];
        assert_eq!(doc.kind, "Skill");
        assert_eq!(doc.metadata.name, "Docs: writer");
        assert_eq!(doc.metadata.description, "Writes docs");
        assert_eq!(doc.spec["slug"], "docs");
        assert_eq!(doc.spec["content"], "# Steps\n\nDo it.\n");
    }

    #[test]
    fn parses_multi_document_bundle_and_rejects_bad_kind() {
        let bundle = "apiVersion: orbit.io/v1\nkind: Provider\nmetadata:\n  name: p\nspec:\n  kind: runtime_managed\n---\napiVersion: orbit.io/v1\nkind: Skill\nmetadata:\n  name: s\nspec:\n  slug: s\n  content: hi\n";
        assert_eq!(parse_upload(bundle, "b.yaml").unwrap().documents.len(), 2);
        let bad = "apiVersion: orbit.io/v1\nkind: Nope\nmetadata:\n  name: x\n";
        let err = parse_upload(bad, "x.yaml").unwrap_err();
        assert!(err[0].message.contains("unsupported kind"));
        let wrong_version = "apiVersion: v9\nkind: Skill\nmetadata:\n  name: x\nspec: {}\n";
        assert!(parse_upload(wrong_version, "x.yaml").is_err());
    }

    #[test]
    fn rejects_unknown_envelope_fields() {
        let doc =
            "apiVersion: orbit.io/v1\nkind: Skill\nmetadata:\n  name: x\nspec: {}\nextra: 1\n";
        assert!(parse_upload(doc, "x.yaml").is_err());
    }

    #[test]
    fn kind_resource_mapping_is_consistent() {
        for kind in PortableKind::ALL {
            assert_eq!(PortableKind::from_resource(kind.resource()), Some(kind));
            assert_eq!(PortableKind::parse(kind.as_str()), Some(kind));
        }
    }
}
