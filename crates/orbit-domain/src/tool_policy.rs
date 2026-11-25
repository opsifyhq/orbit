//! Tool policy rules: per-call allow / deny / require-approval decisions.
//!
//! Rules can only make the platform decision stricter. A matching `deny` always wins,
//! then `require_approval`, then `allow`; an `allow` rule can never turn a platform
//! `require_approval` or `deny` into an execution. This keeps autonomy and capability
//! grants authoritative while allowing argument-level restrictions (paths, shell
//! commands, connections) on top.

use regex::Regex;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::autonomy::{CapabilityEffect, PolicyDecision};

/// What a matching rule asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolPolicyAction {
    Allow,
    Deny,
    RequireApproval,
}

impl ToolPolicyAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
            Self::RequireApproval => "require_approval",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "allow" => Some(Self::Allow),
            "deny" => Some(Self::Deny),
            "require_approval" => Some(Self::RequireApproval),
            _ => None,
        }
    }
}

/// Fields a rule can match on. Every configured field must match (AND). An empty matcher
/// matches every tool call and is useful for a default `require_approval` policy.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolPolicyMatcher {
    /// Glob against the tool name (`*` and `?` are wildcards).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// Glob against the connector capability, e.g. `issues.*`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effect: Option<CapabilityEffect>,
    /// Glob against the connection name/slug.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection: Option<String>,
    /// Glob against path-like arguments (`path`, `file_path`, `paths`, …). Deny and
    /// require-approval rules also match when no path can be extracted (fail closed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path_glob: Option<String>,
    /// Regular expression against shell commands (`command`, `cmd`). Best-effort, not a
    /// sandbox; deny and require-approval rules match when no command can be extracted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command_regex: Option<String>,
}

impl ToolPolicyMatcher {
    pub fn is_empty(&self) -> bool {
        self.tool.is_none()
            && self.capability.is_none()
            && self.effect.is_none()
            && self.connection.is_none()
            && self.path_glob.is_none()
            && self.command_regex.is_none()
    }

    /// Compile the command regex, if configured. Validation uses this to reject bad rules
    /// before they are stored.
    pub fn compile(&self) -> Result<Option<Regex>, regex::Error> {
        self.command_regex
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(Regex::new)
            .transpose()
    }
}

/// A stored rule. `priority` orders evaluation for reporting only; the deny >
/// require_approval > allow precedence decides the outcome.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolPolicyRule {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub priority: i32,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    pub action: ToolPolicyAction,
    #[serde(default)]
    pub matcher: ToolPolicyMatcher,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Human-readable impact shown in the approval UI, e.g. "deletes files under /repo".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impact: Option<String>,
}

fn default_enabled() -> bool {
    true
}

/// The concrete tool call being evaluated.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    pub tool: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<CapabilityEffect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection: Option<String>,
    #[serde(default)]
    pub arguments: serde_json::Value,
    /// Working directory of the call, used to resolve relative paths. When absent,
    /// relative paths that escape upwards (`..`) are treated as unresolvable and match
    /// absolute-path restrictions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

/// Final decision plus provenance for audit/UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolPolicyOutcome {
    pub decision: PolicyDecision,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub impact: Option<String>,
}

impl ToolPolicyOutcome {
    pub fn from_platform(decision: PolicyDecision) -> Self {
        Self {
            decision,
            rule_id: None,
            rule_name: None,
            reason: None,
            impact: None,
        }
    }

    pub fn is_allowed(&self) -> bool {
        self.decision == PolicyDecision::Allow
    }
}

/// Combine the platform decision with matching tool-policy rules.
///
/// `platform` is the hard floor and can never be weakened. `default_decision` applies
/// when no rule matches; it may be stricter than `platform` (for example native harness
/// tools default to `require_approval`) and an explicit `allow` rule may resolve it.
///
/// Precedence: platform `deny` wins, then any matching rule `deny`, then
/// `require_approval`, then `allow`. An `allow` rule never weakens `platform`.
pub fn evaluate_tool_call(
    platform: PolicyDecision,
    default_decision: PolicyDecision,
    rules: &[ToolPolicyRule],
    call: &ToolCall,
) -> ToolPolicyOutcome {
    if platform == PolicyDecision::Deny {
        return ToolPolicyOutcome::from_platform(PolicyDecision::Deny);
    }
    let strictest = |left: PolicyDecision, right: PolicyDecision| -> PolicyDecision {
        use PolicyDecision::*;
        match (left, right) {
            (Deny, _) | (_, Deny) => Deny,
            (RequireApproval, _) | (_, RequireApproval) => RequireApproval,
            _ => Allow,
        }
    };

    let mut matched: Vec<&ToolPolicyRule> = rules
        .iter()
        .filter(|rule| rule.enabled && matches(rule, call))
        .collect();
    // Stable ordering only affects which reason is reported for the same decision.
    matched.sort_by_key(|rule| (rule.priority, rule.name.clone()));

    let deny = matched
        .iter()
        .find(|rule| rule.action == ToolPolicyAction::Deny);
    let approval = matched
        .iter()
        .find(|rule| rule.action == ToolPolicyAction::RequireApproval);
    let allow = matched
        .iter()
        .find(|rule| rule.action == ToolPolicyAction::Allow);

    let (rule_decision, rule) = match (deny, approval, allow) {
        (Some(rule), _, _) => (PolicyDecision::Deny, Some(*rule)),
        (None, Some(rule), _) => (PolicyDecision::RequireApproval, Some(*rule)),
        (None, None, Some(rule)) => (PolicyDecision::Allow, Some(*rule)),
        (None, None, None) => (default_decision, None),
    };
    let decision = strictest(platform, rule_decision);
    let outcome = ToolPolicyOutcome {
        decision,
        rule_id: rule.map(|rule| rule.id),
        rule_name: rule.map(|rule| rule.name.clone()),
        reason: rule.and_then(|rule| rule.reason.clone()),
        impact: rule.and_then(|rule| rule.impact.clone()),
    };
    if decision == PolicyDecision::RequireApproval && outcome.reason.is_none() {
        ToolPolicyOutcome {
            reason: Some("platform requires approval for this effect".into()),
            ..outcome
        }
    } else {
        outcome
    }
}

/// Validate a rule before storing it. Returns a human-readable issue list.
pub fn validate_tool_policy(rule: &ToolPolicyRule) -> Vec<String> {
    let mut issues = Vec::new();
    if rule.name.trim().is_empty() {
        issues.push("name is required".into());
    }
    if let Some(issue) = crate::name_length_issue("name", &rule.name) {
        issues.push(issue.message);
    }
    for (field, value) in [
        ("matcher.tool", rule.matcher.tool.as_deref()),
        ("matcher.capability", rule.matcher.capability.as_deref()),
        ("matcher.connection", rule.matcher.connection.as_deref()),
        ("matcher.pathGlob", rule.matcher.path_glob.as_deref()),
    ] {
        if let Some(value) = value {
            if value.trim().is_empty() {
                issues.push(format!("{field} must not be empty"));
            }
        }
    }
    if let Err(error) = rule.matcher.compile() {
        issues.push(format!(
            "matcher.commandRegex is not a valid regex: {error}"
        ));
    }
    issues
}

/// Whether a rule applies to a call.
///
/// Matchers fail closed for restrictive rules: when a `deny` or `require_approval` rule
/// configures `pathGlob` / `commandRegex` but the call carries nothing the matcher can
/// inspect (an unexpected argument shape, an unlisted key), the rule MATCHES. An `allow`
/// rule in the same situation does not match, so unknown shapes never gain permission.
fn matches(rule: &ToolPolicyRule, call: &ToolCall) -> bool {
    let matcher = &rule.matcher;
    let fail_closed = rule.action != ToolPolicyAction::Allow;
    if let Some(pattern) = &matcher.tool {
        // Tool names differ in case between harnesses (`bash` vs `Bash`).
        if !glob_match(&pattern.to_lowercase(), &call.tool.to_lowercase()) {
            return false;
        }
    }
    if let Some(pattern) = &matcher.capability {
        let Some(capability) = call.capability.as_deref() else {
            return false;
        };
        if !glob_match(pattern, capability) {
            return false;
        }
    }
    if let Some(effect) = matcher.effect {
        if call.effect != Some(effect) {
            return false;
        }
    }
    if let Some(pattern) = &matcher.connection {
        let Some(connection) = call.connection.as_deref() else {
            return false;
        };
        if !glob_match(pattern, connection) {
            return false;
        }
    }
    if let Some(pattern) = &matcher.path_glob {
        let paths = argument_paths(&call.tool, &call.arguments, fail_closed);
        let hit = if paths.is_empty() {
            fail_closed
        } else {
            paths
                .iter()
                .any(|path| path_matches(pattern, path, call.cwd.as_deref(), fail_closed))
        };
        if !hit {
            return false;
        }
    }
    if let Some(pattern) = &matcher.command_regex {
        let pattern = pattern.trim();
        let hit = if pattern.is_empty() {
            false
        } else {
            match Regex::new(pattern) {
                Ok(regex) => match argument_command(&call.arguments) {
                    Some(command) => regex.is_match(&command),
                    None => fail_closed,
                },
                // A rule that cannot be evaluated must not silently stop restricting.
                Err(_) => fail_closed,
            }
        };
        if !hit {
            return false;
        }
    }
    true
}

const COMMAND_KEYS: [&str; 7] = [
    "command",
    "cmd",
    "script",
    "commandLine",
    "command_line",
    "cmdline",
    "argv",
];

fn argument_command(arguments: &serde_json::Value) -> Option<String> {
    for key in COMMAND_KEYS {
        let Some(value) = arguments.get(key) else {
            continue;
        };
        match value {
            serde_json::Value::String(command) => return Some(command.clone()),
            serde_json::Value::Array(parts) => {
                let parts: Vec<&str> = parts.iter().filter_map(|part| part.as_str()).collect();
                if !parts.is_empty() {
                    return Some(parts.join(" "));
                }
            }
            _ => {}
        }
    }
    None
}

/// Argument keys treated as filesystem paths. `pattern` is deliberately absent: for
/// `grep` it is a content regex, not a path (see [`PATTERN_PATH_TOOLS`]).
const PATH_KEYS: [&str; 23] = [
    "path",
    "file_path",
    "filePath",
    "paths",
    "files",
    "file",
    "filename",
    "fileName",
    "target",
    "destination",
    "directory",
    "dir",
    "cwd",
    "workdir",
    "workingDirectory",
    "old_path",
    "oldPath",
    "new_path",
    "newPath",
    "src",
    "dst",
    "source",
    "dest",
];

/// Tools whose `pattern` argument is a path glob rather than a content regex.
const PATTERN_PATH_TOOLS: [&str; 4] = ["glob", "find", "ls", "list"];

fn push_path_value(value: &serde_json::Value, paths: &mut Vec<String>) {
    match value {
        serde_json::Value::String(path) => paths.push(path.clone()),
        serde_json::Value::Array(items) => {
            for item in items {
                push_path_value(item, paths);
            }
        }
        serde_json::Value::Object(object) => {
            if let Some(path) = object.get("path").and_then(|v| v.as_str()) {
                paths.push(path.to_string());
            }
        }
        _ => {}
    }
}

/// Recursively collect string leaves that look like paths, from keys we do not know.
fn collect_pathlike(value: &serde_json::Value, paths: &mut Vec<String>) {
    match value {
        serde_json::Value::String(text) => {
            let text = text.trim();
            if text.len() <= 4096
                && !text.contains('\n')
                && (text.contains('/') || text.starts_with('~') || text.starts_with(".."))
            {
                paths.push(text.to_string());
            }
        }
        serde_json::Value::Array(items) => items.iter().for_each(|i| collect_pathlike(i, paths)),
        serde_json::Value::Object(map) => map.values().for_each(|v| collect_pathlike(v, paths)),
        _ => {}
    }
}

/// Paths a call refers to. With `deep` (restrictive rules), path-like strings under
/// unrecognised keys are inspected too, so renaming the key does not hide a path.
fn argument_paths(tool: &str, arguments: &serde_json::Value, deep: bool) -> Vec<String> {
    let mut paths = Vec::new();
    for key in PATH_KEYS {
        if let Some(value) = arguments.get(key) {
            push_path_value(value, &mut paths);
        }
    }
    if PATTERN_PATH_TOOLS.contains(&tool.to_lowercase().as_str()) {
        if let Some(value) = arguments.get("pattern") {
            push_path_value(value, &mut paths);
        }
    }
    if deep {
        collect_pathlike(arguments, &mut paths);
    }
    paths
}

/// Glob match with `*` (any sequence, including `/`) and `?` (one character).
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    let (mut p, mut t) = (0usize, 0usize);
    let (mut star, mut mark) = (None, 0usize);
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            mark = t;
            p += 1;
        } else if let Some(star_position) = star {
            p = star_position + 1;
            mark += 1;
            t = mark;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == '*' {
        p += 1;
    }
    p == pattern.len()
}

fn path_matches(pattern: &str, path: &str, cwd: Option<&str>, fail_closed: bool) -> bool {
    let resolved = resolve_path(path, cwd);
    let pattern = pattern.trim();
    if glob_match(pattern, &resolved.path) {
        return true;
    }
    if pattern.starts_with('/') {
        // A path we cannot anchor (`../../etc/shadow` without a known cwd, `~/x`) may
        // land anywhere; restrictive rules must assume it hits an absolute pattern.
        return fail_closed && resolved.unresolved;
    }
    // Relative patterns also match any component-aligned suffix, so `src/**` matches
    // `/workspace/src/lib.rs` and `*.env` matches `/repo/.env`.
    let components: Vec<&str> = resolved
        .path
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    for index in 0..components.len() {
        let suffix = components[index..].join("/");
        if glob_match(pattern, &suffix) {
            return true;
        }
    }
    false
}

struct ResolvedPath {
    path: String,
    /// Relative path that escapes its base (`..` first) or `~`-rooted: its real
    /// location is unknown without a cwd / home directory.
    unresolved: bool,
}

/// Lexically resolve a path against `cwd` (when absolute) without touching the
/// filesystem. Leading `..` of an unanchored relative path is preserved.
fn resolve_path(path: &str, cwd: Option<&str>) -> ResolvedPath {
    let path = path.trim().replace('\\', "/");
    if path.starts_with('~') {
        return ResolvedPath {
            path,
            unresolved: true,
        };
    }
    if path.starts_with('/') {
        return ResolvedPath {
            path: normalize_path(&path),
            unresolved: false,
        };
    }
    if let Some(cwd) = cwd.map(|c| c.trim().replace('\\', "/")) {
        if cwd.starts_with('/') {
            return ResolvedPath {
                path: normalize_path(&format!("{cwd}/{path}")),
                unresolved: false,
            };
        }
    }
    let normalized = normalize_path(&path);
    let unresolved = normalized == ".." || normalized.starts_with("../");
    ResolvedPath {
        path: normalized,
        unresolved,
    }
}

/// Lexically normalize a path: collapse `.`/`..` and duplicate separators. `..` above
/// the root of an absolute path is dropped; on a relative path it is preserved.
fn normalize_path(path: &str) -> String {
    let absolute = path.starts_with('/');
    let mut stack: Vec<&str> = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => match stack.last() {
                Some(&last) if last != ".." => {
                    stack.pop();
                }
                _ if absolute => {}
                _ => stack.push(".."),
            },
            other => stack.push(other),
        }
    }
    let joined = stack.join("/");
    if absolute {
        format!("/{joined}")
    } else if joined.is_empty() {
        ".".into()
    } else {
        joined
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::autonomy::PolicyDecision;

    fn rule(name: &str, action: ToolPolicyAction, matcher: ToolPolicyMatcher) -> ToolPolicyRule {
        ToolPolicyRule {
            id: Uuid::now_v7(),
            name: name.into(),
            priority: 100,
            enabled: true,
            action,
            matcher,
            reason: None,
            impact: None,
        }
    }

    fn call(tool: &str, arguments: serde_json::Value) -> ToolCall {
        ToolCall {
            tool: tool.into(),
            capability: None,
            effect: None,
            connection: None,
            arguments,
            cwd: None,
        }
    }

    #[test]
    fn denies_win_over_allow_and_approval() {
        let rules = vec![
            rule(
                "allow read",
                ToolPolicyAction::Allow,
                ToolPolicyMatcher {
                    tool: Some("Read".into()),
                    ..Default::default()
                },
            ),
            rule(
                "ask shell",
                ToolPolicyAction::RequireApproval,
                ToolPolicyMatcher {
                    tool: Some("Bash".into()),
                    ..Default::default()
                },
            ),
            rule(
                "deny rm",
                ToolPolicyAction::Deny,
                ToolPolicyMatcher {
                    command_regex: Some("rm\\s+-rf".into()),
                    ..Default::default()
                },
            ),
        ];
        let outcome = evaluate_tool_call(
            PolicyDecision::Allow,
            PolicyDecision::Allow,
            &rules,
            &call("Bash", serde_json::json!({ "command": "rm -rf /" })),
        );
        assert_eq!(outcome.decision, PolicyDecision::Deny);
        assert_eq!(outcome.rule_name.as_deref(), Some("deny rm"));
    }

    #[test]
    fn allow_rules_cannot_weaken_platform_approval() {
        let rules = vec![rule(
            "allow everything",
            ToolPolicyAction::Allow,
            ToolPolicyMatcher::default(),
        )];
        let outcome = evaluate_tool_call(
            PolicyDecision::RequireApproval,
            PolicyDecision::RequireApproval,
            &rules,
            &call("Write", serde_json::json!({})),
        );
        assert_eq!(outcome.decision, PolicyDecision::RequireApproval);
    }

    #[test]
    fn default_approval_rule_matches_all() {
        let rules = vec![rule(
            "ask all tools",
            ToolPolicyAction::RequireApproval,
            ToolPolicyMatcher::default(),
        )];
        let outcome = evaluate_tool_call(
            PolicyDecision::Allow,
            PolicyDecision::Allow,
            &rules,
            &call("Bash", serde_json::json!({ "command": "git status" })),
        );
        assert_eq!(outcome.decision, PolicyDecision::RequireApproval);
        assert_eq!(outcome.rule_name.as_deref(), Some("ask all tools"));
    }

    #[test]
    fn path_globs_normalize_traversal() {
        let matcher = ToolPolicyMatcher {
            path_glob: Some("secrets/**".into()),
            ..Default::default()
        };
        assert!(matches(
            &rule("p", ToolPolicyAction::Deny, matcher.clone()),
            &call(
                "Read",
                serde_json::json!({ "path": "/workspace/secrets/key.txt" })
            )
        ));
        assert!(matches(
            &rule("p", ToolPolicyAction::Deny, matcher.clone()),
            &call(
                "Read",
                serde_json::json!({ "path": "a/../secrets/../secrets/key.txt" })
            )
        ));
        assert!(!matches(
            &rule("p", ToolPolicyAction::Deny, matcher),
            &call("Read", serde_json::json!({ "path": "src/main.rs" }))
        ));
    }

    #[test]
    fn command_arrays_are_joined() {
        let matcher = ToolPolicyMatcher {
            command_regex: Some("^git push".into()),
            ..Default::default()
        };
        assert!(matches(
            &rule("push", ToolPolicyAction::RequireApproval, matcher),
            &call("Bash", serde_json::json!({ "command": ["git", "push"] }))
        ));
    }

    #[test]
    fn effects_and_capabilities_must_match_when_configured() {
        let matcher = ToolPolicyMatcher {
            capability: Some("issues.*".into()),
            effect: Some(CapabilityEffect::Write),
            ..Default::default()
        };
        let mut write = call("issues.create", serde_json::json!({}));
        write.capability = Some("issues.create".into());
        write.effect = Some(CapabilityEffect::Write);
        assert!(matches(
            &rule("w", ToolPolicyAction::Deny, matcher.clone()),
            &write
        ));
        write.effect = Some(CapabilityEffect::Read);
        assert!(!matches(
            &rule("w", ToolPolicyAction::Deny, matcher),
            &write
        ));
    }

    #[test]
    fn allow_rule_resolves_a_stricter_default() {
        let rules = vec![ToolPolicyRule {
            reason: Some("read-only command".into()),
            ..rule(
                "allow git status",
                ToolPolicyAction::Allow,
                ToolPolicyMatcher {
                    command_regex: Some("^git status$".into()),
                    ..Default::default()
                },
            )
        }];
        let bash_call = call("Bash", serde_json::json!({ "command": "git status" }));
        // Native tools default to require_approval; the explicit allow resolves it.
        let outcome = evaluate_tool_call(
            PolicyDecision::Allow,
            PolicyDecision::RequireApproval,
            &rules,
            &bash_call,
        );
        assert_eq!(outcome.decision, PolicyDecision::Allow);
        assert_eq!(outcome.rule_name.as_deref(), Some("allow git status"));
        assert_eq!(outcome.reason.as_deref(), Some("read-only command"));

        // A different command still requires approval.
        let outcome = evaluate_tool_call(
            PolicyDecision::Allow,
            PolicyDecision::RequireApproval,
            &rules,
            &call("Bash", serde_json::json!({ "command": "git push" })),
        );
        assert_eq!(outcome.decision, PolicyDecision::RequireApproval);
    }

    #[test]
    fn invalid_regex_is_rejected_by_validation() {
        let bad = rule(
            "bad",
            ToolPolicyAction::Deny,
            ToolPolicyMatcher {
                command_regex: Some("([".into()),
                ..Default::default()
            },
        );
        assert!(!validate_tool_policy(&bad).is_empty());
    }

    fn path_rule(action: ToolPolicyAction, glob: &str) -> ToolPolicyRule {
        rule(
            "p",
            action,
            ToolPolicyMatcher {
                path_glob: Some(glob.into()),
                ..Default::default()
            },
        )
    }

    #[test]
    fn deny_path_glob_fails_closed_when_nothing_extractable() {
        let deny = path_rule(ToolPolicyAction::Deny, "/etc/**");
        assert!(matches(
            &deny,
            &call("Read", serde_json::json!({ "weird": 1 }))
        ));
        assert!(matches(&deny, &call("Read", serde_json::json!({}))));
        let ask = path_rule(ToolPolicyAction::RequireApproval, "/etc/**");
        assert!(matches(
            &ask,
            &call("Read", serde_json::json!({ "weird": 1 }))
        ));
    }

    #[test]
    fn allow_path_glob_does_not_match_when_nothing_extractable() {
        let allow = path_rule(ToolPolicyAction::Allow, "/work/**");
        assert!(!matches(
            &allow,
            &call("Read", serde_json::json!({ "weird": 1 }))
        ));
        assert!(!matches(&allow, &call("Read", serde_json::json!({}))));
    }

    #[test]
    fn deny_path_glob_sees_paths_under_unlisted_keys() {
        let deny = path_rule(ToolPolicyAction::Deny, "/etc/**");
        assert!(matches(
            &deny,
            &call(
                "Read",
                serde_json::json!({ "location": { "p": "/etc/shadow" } })
            )
        ));
        // An allow rule only trusts recognised keys.
        let allow = path_rule(ToolPolicyAction::Allow, "/etc/**");
        assert!(!matches(
            &allow,
            &call("Read", serde_json::json!({ "location": "/etc/shadow" }))
        ));
        // Extended keys are recognised.
        assert!(matches(
            &deny,
            &call("Read", serde_json::json!({ "oldPath": "/etc/shadow" }))
        ));
    }

    #[test]
    fn traversal_is_resolved_against_cwd() {
        let deny = path_rule(ToolPolicyAction::Deny, "/etc/**");
        let mut traversal = call("Read", serde_json::json!({ "path": "../../etc/x" }));
        traversal.cwd = Some("/work".into());
        assert!(matches(&deny, &traversal));
        // Stays inside cwd: not a match.
        let mut inside = call("Read", serde_json::json!({ "path": "../work/src/x" }));
        inside.cwd = Some("/work".into());
        assert!(!matches(&deny, &inside));
    }

    #[test]
    fn traversal_without_cwd_matches_absolute_deny_but_not_allow() {
        let traversal = call("Read", serde_json::json!({ "path": "../../etc/x" }));
        assert!(matches(
            &path_rule(ToolPolicyAction::Deny, "/etc/**"),
            &traversal
        ));
        assert!(!matches(
            &path_rule(ToolPolicyAction::Allow, "/etc/**"),
            &traversal
        ));
        // Plain relative paths without `..` stay unanchored and do not match.
        assert!(!matches(
            &path_rule(ToolPolicyAction::Deny, "/etc/**"),
            &call("Read", serde_json::json!({ "path": "src/x" }))
        ));
        assert_eq!(normalize_path("../../etc/x"), "../../etc/x");
        assert_eq!(normalize_path("/../etc/x"), "/etc/x");
    }

    #[test]
    fn tool_name_match_is_case_insensitive() {
        let r = rule(
            "t",
            ToolPolicyAction::Deny,
            ToolPolicyMatcher {
                tool: Some("bash".into()),
                ..Default::default()
            },
        );
        assert!(matches(&r, &call("Bash", serde_json::json!({}))));
        assert!(matches(&r, &call("BASH", serde_json::json!({}))));
    }

    #[test]
    fn command_regex_fails_closed_for_restrictive_rules_only() {
        let matcher = ToolPolicyMatcher {
            command_regex: Some("rm\\s+-rf".into()),
            ..Default::default()
        };
        let odd = call("Bash", serde_json::json!({ "line": "rm -rf /" }));
        assert!(matches(
            &rule("d", ToolPolicyAction::Deny, matcher.clone()),
            &odd
        ));
        assert!(!matches(&rule("a", ToolPolicyAction::Allow, matcher), &odd));
    }

    #[test]
    fn grep_pattern_is_not_a_path_but_glob_pattern_is() {
        let allow = path_rule(ToolPolicyAction::Allow, "/etc/**");
        assert!(!matches(
            &allow,
            &call("grep", serde_json::json!({ "pattern": "/etc/shadow" }))
        ));
        assert!(matches(
            &allow,
            &call("glob", serde_json::json!({ "pattern": "/etc/*" }))
        ));
    }
}
