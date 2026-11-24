//! Agent Team work sources: configuration, validation and a small read/write
//! connector client used by the control plane dispatcher and close-the-loop watcher.
//!
//! This intentionally mirrors the worker connector surface but stays in `orbit-api`
//! because issue discovery and queue bookkeeping are control-plane responsibilities.
//! Credentials are read from the encrypted store per invocation and are never logged.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use orbit_domain::ValidationIssue;

fn default_claim_label() -> String {
    "agent-in-progress".into()
}

fn default_done_label() -> String {
    "agent-done".into()
}

fn default_failed_label() -> String {
    "agent-failed".into()
}

fn default_max_items() -> i64 {
    5
}

fn default_max_attempts() -> i64 {
    1
}

fn default_poll_seconds() -> i64 {
    60
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkSourceDefinition {
    pub provider: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub connection_id: Option<Uuid>,

    // GitHub target.
    #[serde(default)]
    pub repository: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default = "default_claim_label")]
    pub claim_label: String,
    #[serde(default = "default_done_label")]
    pub done_label: String,
    #[serde(default = "default_failed_label")]
    pub failed_label: String,
    #[serde(default = "default_max_items")]
    pub max_items: i64,
    #[serde(default = "default_max_attempts")]
    pub max_attempts: i64,
    #[serde(default = "default_poll_seconds")]
    pub poll_seconds: i64,
    #[serde(default)]
    pub auto_run: bool,

    // GitLab target.
    #[serde(default)]
    pub group_id: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub board_id: Option<i64>,
}

impl Default for WorkSourceDefinition {
    fn default() -> Self {
        Self {
            provider: "github".into(),
            enabled: false,
            connection_id: None,
            repository: None,
            labels: vec![],
            claim_label: default_claim_label(),
            done_label: default_done_label(),
            failed_label: default_failed_label(),
            max_items: default_max_items(),
            max_attempts: default_max_attempts(),
            poll_seconds: default_poll_seconds(),
            auto_run: false,
            group_id: None,
            project: None,
            board_id: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RepositorySpec {
    pub connection_id: Option<Uuid>,
    pub repository: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub primary: bool,
    #[serde(default)]
    pub branch: Option<String>,
}

pub fn validate_work_source(source: &WorkSourceDefinition) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    if !matches!(source.provider.as_str(), "github" | "gitlab") {
        issues.push(ValidationIssue::new(
            "workSource.provider",
            "provider must be github or gitlab",
        ));
    }
    if !source.enabled {
        return issues;
    }
    if source.connection_id.is_none() {
        issues.push(ValidationIssue::new(
            "workSource.connectionId",
            "an enabled work source requires a connection",
        ));
    }
    match source.provider.as_str() {
        "github" => {
            if source
                .repository
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .is_none()
            {
                issues.push(ValidationIssue::new(
                    "workSource.repository",
                    "GitHub work source requires repository (owner/repo)",
                ));
            } else if let Some(repository) = source
                .repository
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
            {
                if !repository.contains('/') || repository.chars().any(char::is_whitespace) {
                    issues.push(ValidationIssue::new(
                        "workSource.repository",
                        "repository must be owner/repo",
                    ));
                }
            }
        }
        "gitlab" => {
            let has_project = source
                .project
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .is_some();
            let has_group = source
                .group_id
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .is_some();
            if !has_project && !has_group {
                issues.push(ValidationIssue::new(
                    "workSource",
                    "GitLab work source requires project or groupId",
                ));
            }
        }
        _ => {}
    }
    if !(1..=100).contains(&source.max_items) {
        issues.push(ValidationIssue::new(
            "workSource.maxItems",
            "maxItems must be between 1 and 100",
        ));
    }
    if !(1..=10).contains(&source.max_attempts) {
        issues.push(ValidationIssue::new(
            "workSource.maxAttempts",
            "maxAttempts must be between 1 and 10",
        ));
    }
    if source.poll_seconds < 5 {
        issues.push(ValidationIssue::new(
            "workSource.pollSeconds",
            "pollSeconds must be at least 5",
        ));
    }
    issues
}

pub fn validate_repositories(repositories: &[RepositorySpec]) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for (index, repository) in repositories.iter().enumerate() {
        let name = repository.repository.trim();
        if name.is_empty() || name.chars().any(char::is_whitespace) || !name.contains('/') {
            issues.push(ValidationIssue::new(
                "repositories.repository",
                format!("repository {} must be owner/repo", index + 1),
            ));
        }
        if repository
            .path
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
        {
            issues.push(ValidationIssue::new(
                "repositories.path",
                format!("repository {} has an unsafe workspace path", index + 1),
            ));
        }
        if !seen.insert(name.to_lowercase()) {
            issues.push(ValidationIssue::new(
                "repositories.repository",
                format!("duplicate repository '{name}'"),
            ));
        }
    }
    issues
}

#[derive(Debug, Clone)]
pub struct ExternalIssue {
    pub key: String,
    pub repository: Option<String>,
    pub project: Option<String>,
    pub number: u64,
    pub title: String,
    pub labels: Vec<String>,
    pub url: Option<String>,
}

fn github_repo_segments(repository: &str) -> anyhow::Result<(&str, &str)> {
    repository
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("repository must be owner/repo"))
}

fn encode_gitlab_segment(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

fn labels_to_string(labels: &[String]) -> String {
    labels.join(",")
}

async fn http_json(
    client: &reqwest::Client,
    method: reqwest::Method,
    url: &str,
    token: Option<&str>,
    body: Option<serde_json::Value>,
    provider: &str,
) -> anyhow::Result<serde_json::Value> {
    let mut request = client.request(method, url);
    if let Some(token) = token {
        if provider == "gitlab" {
            request = request.header("PRIVATE-TOKEN", token);
        } else {
            request = request
                .header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"))
                .header(reqwest::header::USER_AGENT, "orbit-agent-platform")
                .header(reqwest::header::ACCEPT, "application/vnd.github+json");
        }
    }
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request
        .send()
        .await
        .map_err(|error| anyhow::anyhow!("connector request failed: {error}"))?;
    let status = response.status();
    let payload: serde_json::Value = response
        .json()
        .await
        .unwrap_or_else(|_| serde_json::json!({ "message": "empty response" }));
    if !status.is_success() {
        let message = payload
            .get("message")
            .or_else(|| payload.get("error"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("connector API error");
        anyhow::bail!("connector returned {status}: {message}");
    }
    Ok(payload)
}

/// Whether a GitLab group work source covers `project` (#384). A group given by path
/// covers the projects under that path. A group given by numeric id cannot be matched
/// without an API call, so only `lenient` callers (items already in the queue, which the
/// dispatcher discovered in that group) accept its projects; the connection's
/// `allowedProjects` still applies to every write.
pub(crate) fn group_includes_project(
    source: &WorkSourceDefinition,
    project: &str,
    lenient: bool,
) -> bool {
    if source.provider != "gitlab"
        || source
            .project
            .as_deref()
            .is_some_and(|project| !project.trim().is_empty())
    {
        return false;
    }
    let Some(group) = source
        .group_id
        .as_deref()
        .map(|group| group.trim().trim_matches('/'))
        .filter(|group| !group.is_empty())
    else {
        return false;
    };
    if group.chars().all(|c| c.is_ascii_digit()) {
        return lenient;
    }
    let project = project.trim().trim_matches('/').to_ascii_lowercase();
    project.starts_with(&format!("{}/", group.to_ascii_lowercase()))
}

/// Whether a work source may write back to `target` (a GitHub `owner/repo` or a GitLab
/// project path or id): it must be the source's own repository or project (or, for a
/// GitLab group source, a project of that group), and it must be in the connection's
/// `allowedRepositories` / `allowedProjects` when that list is set, the rule the worker
/// applies to checkouts (#384).
pub(crate) fn work_source_allows_target(
    source: &WorkSourceDefinition,
    connection_config: &serde_json::Value,
    target: &str,
) -> bool {
    let target = target.trim();
    if target.is_empty() {
        return false;
    }
    let same = |candidate: &Option<String>| {
        candidate
            .as_deref()
            .is_some_and(|candidate| candidate.trim().eq_ignore_ascii_case(target))
    };
    let in_source = match source.provider.as_str() {
        "github" => same(&source.repository),
        "gitlab" => same(&source.project) || group_includes_project(source, target, true),
        _ => false,
    };
    in_source && connection_allows_target(connection_config, target)
}

/// The connection allowlist rule shared with the worker (`allowedRepositories`, or
/// `allowedProjects` for GitLab): an empty or missing list allows every target.
pub(crate) fn connection_allows_target(
    connection_config: &serde_json::Value,
    target: &str,
) -> bool {
    let list = connection_config
        .get("allowedRepositories")
        .and_then(serde_json::Value::as_array)
        .filter(|list| !list.is_empty())
        .or_else(|| {
            connection_config
                .get("allowedProjects")
                .and_then(serde_json::Value::as_array)
        });
    match list {
        Some(list) if !list.is_empty() => list
            .iter()
            .filter_map(serde_json::Value::as_str)
            .any(|allowed| allowed.trim().eq_ignore_ascii_case(target.trim())),
        _ => true,
    }
}

/// A work-source write was refused because its target is outside the source or the
/// connection allowlist (#384). Not retried: the target never becomes valid by waiting.
#[derive(Debug)]
pub(crate) struct WorkSourceTargetRefused {
    pub target: String,
}

impl std::fmt::Display for WorkSourceTargetRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "'{}' is not the work source's repository or is not allowed by its connection",
            self.target
        )
    }
}

impl std::error::Error for WorkSourceTargetRefused {}

/// The team's owner can no longer use the connection its work source names (#322).
#[derive(Debug)]
pub(crate) struct WorkSourceAccessLost {
    pub connection: String,
}

impl std::fmt::Display for WorkSourceAccessLost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the team's owner can no longer use work source connection '{}'",
            self.connection
        )
    }
}

impl std::error::Error for WorkSourceAccessLost {}

/// Load a work source's connection for background polling and outcome writes, which run
/// without a caller. The team's owner stands in for the caller: the credential is only
/// used while that user may still use the connection, the same stored-identity rule run
/// paths follow (#140, #193). Teams without an owner keep platform authority.
pub(crate) async fn work_source_connection(
    state: &crate::state::AppState,
    team: &orbit_store::AgentTeam,
    connection_id: Uuid,
) -> anyhow::Result<orbit_store::Connection> {
    let connection = state
        .store
        .get_connection(team.org_id, connection_id)
        .await?;
    if let Some(owner) = team.owner_user_id {
        let access = crate::access::AccessScope::load_for_user(state, team.org_id, owner)
            .await
            .map_err(|error| {
                anyhow::anyhow!("could not load the team owner's access: {}", error.message)
            })?;
        if !access.can_use(
            connection.owner_user_id,
            connection.owner_team_id,
            &connection.visibility,
        ) {
            return Err(WorkSourceAccessLost {
                connection: connection.name.clone(),
            }
            .into());
        }
    }
    Ok(connection)
}

pub async fn list_github_issues(
    client: &reqwest::Client,
    base_url: &str,
    token: Option<&str>,
    source: &WorkSourceDefinition,
) -> anyhow::Result<Vec<ExternalIssue>> {
    let repository = source
        .repository
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("GitHub work source has no repository"))?;
    let (owner, repo) = github_repo_segments(repository)?;
    let base = base_url.trim_end_matches('/');
    let mut url = format!(
        "{base}/repos/{owner}/{repo}/issues?state=open&per_page={}",
        source.max_items.clamp(1, 100)
    );
    if !source.labels.is_empty() {
        url.push_str("&labels=");
        url.push_str(
            &source
                .labels
                .iter()
                .map(|label| label.replace(' ', "%20"))
                .collect::<Vec<_>>()
                .join(","),
        );
    }

    let payload = http_json(client, reqwest::Method::GET, &url, token, None, "github").await?;
    let items = payload
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|item| item.get("pull_request").is_none())
        .filter_map(|item| {
            let number = item.get("number").and_then(serde_json::Value::as_u64)?;
            Some(ExternalIssue {
                key: format!("{repository}#{number}"),
                repository: Some(repository.to_string()),
                project: None,
                number,
                title: item
                    .get("title")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                labels: item
                    .get("labels")
                    .and_then(serde_json::Value::as_array)
                    .map(|labels| {
                        labels
                            .iter()
                            .filter_map(|label| {
                                label
                                    .get("name")
                                    .and_then(serde_json::Value::as_str)
                                    .map(str::to_string)
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                url: item
                    .get("html_url")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
            })
        })
        .collect();
    Ok(items)
}

pub async fn list_gitlab_issues(
    client: &reqwest::Client,
    base_url: &str,
    token: Option<&str>,
    source: &WorkSourceDefinition,
) -> anyhow::Result<Vec<ExternalIssue>> {
    let base = base_url.trim_end_matches('/');
    let per_page = source.max_items.clamp(1, 100);
    let labels = labels_to_string(&source.labels);
    let (url, default_project) = if let Some(project) = source
        .project
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        (
            format!(
                "{base}/projects/{}/issues?state=opened&per_page={per_page}{}",
                encode_gitlab_segment(project),
                if labels.is_empty() {
                    String::new()
                } else {
                    format!("&labels={}", encode_gitlab_segment(&labels))
                }
            ),
            Some(project.to_string()),
        )
    } else {
        let group = source
            .group_id
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("GitLab work source has no groupId or project"))?;
        (
            format!(
                "{base}/groups/{}/issues?state=opened&per_page={per_page}{}",
                encode_gitlab_segment(group),
                if labels.is_empty() {
                    String::new()
                } else {
                    format!("&labels={}", encode_gitlab_segment(&labels))
                }
            ),
            None,
        )
    };

    let payload = http_json(client, reqwest::Method::GET, &url, token, None, "gitlab").await?;
    let items = payload
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|item| {
            let iid = item.get("iid").and_then(serde_json::Value::as_u64)?;
            let project = default_project.clone().or_else(|| {
                item.get("references")
                    .and_then(|references| references.get("full"))
                    .and_then(serde_json::Value::as_str)
                    .and_then(|reference| reference.split('#').next())
                    .map(str::to_string)
            });
            let key = format!("{}#{iid}", project.as_deref().unwrap_or("gitlab-issue"));
            Some(ExternalIssue {
                key,
                repository: project.clone(),
                project,
                number: iid,
                title: item
                    .get("title")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                labels: item
                    .get("labels")
                    .and_then(serde_json::Value::as_array)
                    .map(|labels| {
                        labels
                            .iter()
                            .filter_map(serde_json::Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default(),
                url: item
                    .get("web_url")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
            })
        })
        .collect();
    Ok(items)
}

pub async fn get_github_issue(
    client: &reqwest::Client,
    base_url: &str,
    token: Option<&str>,
    repository: &str,
    number: u64,
) -> anyhow::Result<ExternalIssue> {
    let (owner, repo) = github_repo_segments(repository)?;
    let url = format!(
        "{}/repos/{owner}/{repo}/issues/{number}",
        base_url.trim_end_matches('/')
    );
    let item = http_json(client, reqwest::Method::GET, &url, token, None, "github").await?;
    Ok(ExternalIssue {
        key: format!("{repository}#{number}"),
        repository: Some(repository.to_string()),
        project: None,
        number,
        title: item
            .get("title")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        labels: item
            .get("labels")
            .and_then(serde_json::Value::as_array)
            .map(|labels| {
                labels
                    .iter()
                    .filter_map(|label| {
                        label
                            .get("name")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string)
                    })
                    .collect()
            })
            .unwrap_or_default(),
        url: item
            .get("html_url")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
    })
}

pub async fn get_gitlab_issue(
    client: &reqwest::Client,
    base_url: &str,
    token: Option<&str>,
    project: &str,
    iid: u64,
) -> anyhow::Result<ExternalIssue> {
    let url = format!(
        "{}/projects/{}/issues/{iid}",
        base_url.trim_end_matches('/'),
        encode_gitlab_segment(project)
    );
    let item = http_json(client, reqwest::Method::GET, &url, token, None, "gitlab").await?;
    Ok(ExternalIssue {
        key: format!("{project}#{iid}"),
        repository: Some(project.to_string()),
        project: Some(project.to_string()),
        number: iid,
        title: item
            .get("title")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        labels: item
            .get("labels")
            .and_then(serde_json::Value::as_array)
            .map(|labels| {
                labels
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        url: item
            .get("web_url")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
    })
}

pub async fn update_github_labels(
    client: &reqwest::Client,
    base_url: &str,
    token: Option<&str>,
    repository: &str,
    number: u64,
    labels: &[String],
) -> anyhow::Result<()> {
    let (owner, repo) = github_repo_segments(repository)?;
    let url = format!(
        "{}/repos/{owner}/{repo}/issues/{number}",
        base_url.trim_end_matches('/')
    );
    http_json(
        client,
        reqwest::Method::PATCH,
        &url,
        token,
        Some(serde_json::json!({ "labels": labels })),
        "github",
    )
    .await?;
    Ok(())
}

pub async fn comment_github(
    client: &reqwest::Client,
    base_url: &str,
    token: Option<&str>,
    repository: &str,
    number: u64,
    body: &str,
) -> anyhow::Result<()> {
    let (owner, repo) = github_repo_segments(repository)?;
    let url = format!(
        "{}/repos/{owner}/{repo}/issues/{number}/comments",
        base_url.trim_end_matches('/')
    );
    http_json(
        client,
        reqwest::Method::POST,
        &url,
        token,
        Some(serde_json::json!({ "body": body })),
        "github",
    )
    .await?;
    Ok(())
}

pub async fn update_gitlab_labels(
    client: &reqwest::Client,
    base_url: &str,
    token: Option<&str>,
    project: &str,
    iid: u64,
    add: &[String],
    remove: &[String],
) -> anyhow::Result<()> {
    if add.is_empty() && remove.is_empty() {
        return Ok(());
    }
    let url = format!(
        "{}/projects/{}/issues/{iid}",
        base_url.trim_end_matches('/'),
        encode_gitlab_segment(project)
    );
    let mut body = serde_json::Map::new();
    if !add.is_empty() {
        body.insert(
            "add_labels".into(),
            serde_json::json!(labels_to_string(add)),
        );
    }
    if !remove.is_empty() {
        body.insert(
            "remove_labels".into(),
            serde_json::json!(labels_to_string(remove)),
        );
    }
    http_json(
        client,
        reqwest::Method::PUT,
        &url,
        token,
        Some(serde_json::Value::Object(body)),
        "gitlab",
    )
    .await?;
    Ok(())
}

pub async fn comment_gitlab(
    client: &reqwest::Client,
    base_url: &str,
    token: Option<&str>,
    project: &str,
    iid: u64,
    body: &str,
) -> anyhow::Result<()> {
    let url = format!(
        "{}/projects/{}/issues/{iid}/notes",
        base_url.trim_end_matches('/'),
        encode_gitlab_segment(project)
    );
    http_json(
        client,
        reqwest::Method::POST,
        &url,
        token,
        Some(serde_json::json!({ "body": body })),
        "gitlab",
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_source_validation_requires_connection_and_target() {
        let mut source = WorkSourceDefinition {
            provider: "github".into(),
            enabled: true,
            ..Default::default()
        };
        let issues = validate_work_source(&source);
        assert!(issues
            .iter()
            .any(|issue| issue.path == "workSource.connectionId"));
        assert!(issues
            .iter()
            .any(|issue| issue.path == "workSource.repository"));

        source.connection_id = Some(Uuid::now_v7());
        source.repository = Some("acme/orbit".into());
        assert!(validate_work_source(&source).is_empty());
    }

    #[test]
    fn repository_paths_cannot_escape() {
        let repositories = vec![RepositorySpec {
            connection_id: None,
            repository: "acme/orbit".into(),
            path: "../escape".into(),
            primary: true,
            branch: None,
        }];
        assert!(!validate_repositories(&repositories).is_empty());
    }

    fn source(provider: &str, value: serde_json::Value) -> WorkSourceDefinition {
        let mut value = value;
        value["provider"] = serde_json::json!(provider);
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn work_source_targets_stay_inside_the_source_and_the_connection_allowlist() {
        let github = source("github", serde_json::json!({ "repository": "Acme/App" }));
        let open = serde_json::json!({});
        assert!(work_source_allows_target(&github, &open, "acme/app"));
        assert!(!work_source_allows_target(&github, &open, "acme/other"));
        assert!(!work_source_allows_target(&github, &open, ""));
        let listed = serde_json::json!({ "allowedRepositories": ["acme/other"] });
        assert!(!work_source_allows_target(&github, &listed, "acme/app"));
        let listed = serde_json::json!({ "allowedRepositories": ["ACME/app"] });
        assert!(work_source_allows_target(&github, &listed, "acme/app"));

        let project = source("gitlab", serde_json::json!({ "project": "group/app" }));
        assert!(work_source_allows_target(&project, &open, "group/app"));
        assert!(!work_source_allows_target(&project, &open, "group/other"));
        let gitlab_listed = serde_json::json!({ "allowedProjects": ["group/other"] });
        assert!(!work_source_allows_target(
            &project,
            &gitlab_listed,
            "group/app"
        ));
    }

    #[test]
    fn gitlab_group_sources_cover_their_own_projects() {
        let by_path = source("gitlab", serde_json::json!({ "groupId": "Acme/Team" }));
        assert!(group_includes_project(&by_path, "acme/team/app", false));
        assert!(group_includes_project(&by_path, "acme/team/sub/app", false));
        assert!(!group_includes_project(&by_path, "acme/teamx/app", false));
        assert!(!group_includes_project(&by_path, "other/app", true));
        // A numeric group cannot be matched by path: only already-stored items pass.
        let by_id = source("gitlab", serde_json::json!({ "groupId": "42" }));
        assert!(!group_includes_project(&by_id, "acme/app", false));
        assert!(group_includes_project(&by_id, "acme/app", true));
        // A project source never widens to a group, and GitHub has no groups.
        let project = source(
            "gitlab",
            serde_json::json!({ "project": "acme/app", "groupId": "acme" }),
        );
        assert!(!group_includes_project(&project, "acme/other", true));
        let github = source("github", serde_json::json!({ "groupId": "acme" }));
        assert!(!group_includes_project(&github, "acme/app", true));
    }

    #[test]
    fn encodes_gitlab_segments() {
        assert_eq!(encode_gitlab_segment("group/app"), "group%2Fapp");
    }
}
