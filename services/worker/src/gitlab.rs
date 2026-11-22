//! GitLab connector invoked through the ORBIT tool broker.
//!
//! Supports project issues, group issue boards, merge requests, pipelines and the
//! project-scoped git workflow. Credentials stay in the worker.

use serde_json::{json, Value};

#[derive(Clone, Debug)]
pub struct GitLabClientConfig {
    pub base_url: String,
    pub token: Option<String>,
    pub allowed_projects: Vec<String>,
    pub default_project: Option<String>,
    pub group_id: Option<String>,
    pub allowed_groups: Vec<String>,
}

fn path_list(config: &Value, key: &str) -> Vec<String> {
    config
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(normalize_path)
                .filter(|value| !value.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn normalize_path(value: &str) -> String {
    value.trim().trim_matches('/').to_lowercase()
}

impl GitLabClientConfig {
    pub fn from_connection(config: &Value, token: Option<String>) -> Self {
        let allowed_projects = path_list(config, "allowedProjects");
        Self {
            base_url: config
                .get("baseUrl")
                .and_then(Value::as_str)
                .unwrap_or("https://gitlab.com/api/v4")
                .trim_end_matches('/')
                .to_string(),
            token,
            allowed_projects,
            default_project: config
                .get("defaultProject")
                .and_then(Value::as_str)
                .map(str::to_string),
            group_id: config
                .get("groupId")
                .and_then(Value::as_str)
                .map(str::to_string),
            allowed_groups: path_list(config, "allowedGroups"),
        }
    }

    fn token(&self, capability: &str) -> Result<&str, String> {
        self.token.as_deref().ok_or_else(|| {
            format!("GitLab capability '{capability}' requires a token on the connection")
        })
    }

    fn project(&self, args: &Value) -> Result<String, String> {
        let project = args
            .get("project")
            .and_then(Value::as_str)
            .or_else(|| args.get("projectId").and_then(Value::as_str))
            .or(self.default_project.as_deref())
            .ok_or_else(|| "project is required (or set config.defaultProject)".to_string())?
            .trim()
            .trim_matches('/')
            .to_string();
        if project.is_empty() || project.chars().any(char::is_whitespace) {
            return Err("project must be a GitLab project path or id".into());
        }
        if !self.allowed_projects.is_empty()
            && !self.allowed_projects.contains(&project.to_lowercase())
        {
            return Err(format!(
                "project {project} is not in the connection's allowedProjects"
            ));
        }
        Ok(project)
    }

    /// Resolves the group for a group tool. Only `config.groupId` and the optional
    /// `config.allowedGroups` list may be used: a token usually reaches far more groups
    /// than the connection is configured for.
    fn group(&self, args: &Value) -> Result<String, String> {
        let group = args
            .get("group")
            .and_then(Value::as_str)
            .or(self.group_id.as_deref())
            .ok_or_else(|| "group is required (or set config.groupId)".to_string())?
            .trim()
            .trim_matches('/')
            .to_string();
        if group.is_empty() || group.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err("group must be a GitLab group path or id".into());
        }
        let wanted = group.to_lowercase();
        let allowed = self
            .group_id
            .as_deref()
            .map(normalize_path)
            .into_iter()
            .chain(self.allowed_groups.iter().cloned())
            .any(|item| item == wanted);
        if !allowed {
            return Err(format!(
                "group {group} is not the connection's groupId or in its allowedGroups"
            ));
        }
        Ok(group)
    }

    /// Whether a project returned by the API may be shown. Without `allowedProjects`, every
    /// project the token reaches is allowed, as for the project tools.
    fn project_visible(&self, project: &Value) -> bool {
        if self.allowed_projects.is_empty() {
            return true;
        }
        let path = project
            .get("path_with_namespace")
            .and_then(Value::as_str)
            .map(normalize_path);
        let id = project.get("id").map(|id| match id {
            Value::String(id) => id.clone(),
            other => other.to_string(),
        });
        self.allowed_projects
            .iter()
            .any(|allowed| Some(allowed) == path.as_ref() || Some(allowed) == id.as_ref())
    }
}

fn encode(value: &str) -> String {
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

fn iid(args: &Value) -> Result<u64, String> {
    args.get("iid")
        .or_else(|| args.get("number"))
        .and_then(Value::as_u64)
        .ok_or_else(|| "issue/merge request iid is required".to_string())
}

fn query_string(args: &Value, defaults: &[(&str, &str)]) -> String {
    let mut values = defaults
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect::<Vec<_>>();
    for key in ["state", "labels", "search", "ref", "scope"] {
        if let Some(value) = args.get(key).and_then(Value::as_str) {
            if !value.is_empty() {
                values.retain(|(existing, _)| existing != key);
                values.push((key.to_string(), value.to_string()));
            }
        }
    }
    values
        .into_iter()
        .map(|(key, value)| format!("{key}={}", encode(&value)))
        .collect::<Vec<_>>()
        .join("&")
}

pub async fn invoke(
    client: &reqwest::Client,
    config: &GitLabClientConfig,
    capability: &str,
    args: &Value,
) -> Result<Value, String> {
    let token = config.token(capability)?;
    let base = &config.base_url;
    let (method, url, body) = match capability {
        "projects.list" => (
            reqwest::Method::GET,
            format!("{base}/projects?membership=true&per_page=20"),
            None,
        ),
        "group_issues.list" => {
            let group = config.group(args)?;
            (
                reqwest::Method::GET,
                format!(
                    "{base}/groups/{}/issues?{}",
                    encode(&group),
                    query_string(args, &[("state", "opened"), ("per_page", "20")])
                ),
                None,
            )
        }
        "boards.list" => {
            let group = config.group(args)?;
            (
                reqwest::Method::GET,
                format!("{base}/groups/{}/boards", encode(&group)),
                None,
            )
        }
        "issues.list" => {
            let project = config.project(args)?;
            (
                reqwest::Method::GET,
                format!(
                    "{base}/projects/{}/issues?{}",
                    encode(&project),
                    query_string(args, &[("state", "opened"), ("per_page", "20")])
                ),
                None,
            )
        }
        "issues.get" => {
            let project = config.project(args)?;
            (
                reqwest::Method::GET,
                format!("{base}/projects/{}/issues/{}", encode(&project), iid(args)?),
                None,
            )
        }
        "issues.create" => {
            let project = config.project(args)?;
            let mut payload = json!({
                "title": args
                    .get("title")
                    .and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| "issue title is required".to_string())?,
            });
            for key in ["description", "confidential"] {
                if let Some(value) = args.get(key) {
                    payload[key] = value.clone();
                }
            }
            if let Some(labels) = args.get("labels") {
                payload["labels"] = match labels {
                    Value::Array(items) => json!(items
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(",")),
                    value => value.clone(),
                };
            }
            (
                reqwest::Method::POST,
                format!("{base}/projects/{}/issues", encode(&project)),
                Some(payload),
            )
        }
        "issues.update" => {
            let project = config.project(args)?;
            let mut payload = serde_json::Map::new();
            for key in ["title", "description", "state_event"] {
                if let Some(value) = args.get(key) {
                    payload.insert(key.into(), value.clone());
                }
            }
            if let Some(labels) = args.get("labels") {
                payload.insert(
                    "labels".into(),
                    match labels {
                        Value::Array(items) => json!(items
                            .iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(",")),
                        value => value.clone(),
                    },
                );
            }
            if let Some(assignee) = args.get("assigneeIds") {
                payload.insert("assignee_ids".into(), assignee.clone());
            }
            (
                reqwest::Method::PUT,
                format!("{base}/projects/{}/issues/{}", encode(&project), iid(args)?),
                Some(Value::Object(payload)),
            )
        }
        "issues.comment" => {
            let project = config.project(args)?;
            (
                reqwest::Method::POST,
                format!(
                    "{base}/projects/{}/issues/{}/notes",
                    encode(&project),
                    iid(args)?
                ),
                Some(json!({
                    "body": args
                        .get("body")
                        .and_then(Value::as_str)
                        .filter(|value| !value.trim().is_empty())
                        .ok_or_else(|| "comment body is required".to_string())?,
                })),
            )
        }
        "merge_requests.list" => {
            let project = config.project(args)?;
            (
                reqwest::Method::GET,
                format!(
                    "{base}/projects/{}/merge_requests?{}",
                    encode(&project),
                    query_string(args, &[("state", "opened"), ("per_page", "20")])
                ),
                None,
            )
        }
        "merge_requests.get" => {
            let project = config.project(args)?;
            (
                reqwest::Method::GET,
                format!(
                    "{base}/projects/{}/merge_requests/{}",
                    encode(&project),
                    iid(args)?
                ),
                None,
            )
        }
        "merge_requests.create" => {
            let project = config.project(args)?;
            let title = args
                .get("title")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| "merge request title is required".to_string())?;
            let source = args
                .get("sourceBranch")
                .or_else(|| args.get("source_branch"))
                .and_then(Value::as_str)
                .ok_or_else(|| "source branch is required".to_string())?;
            let target = args
                .get("targetBranch")
                .or_else(|| args.get("target_branch"))
                .and_then(Value::as_str)
                .unwrap_or("main");
            let mut payload = json!({
                "title": title,
                "source_branch": source,
                "target_branch": target,
            });
            if let Some(description) = args
                .get("description")
                .or_else(|| args.get("body"))
                .and_then(Value::as_str)
            {
                payload["description"] = json!(description);
            }
            if let Some(draft) = args.get("draft") {
                payload["draft"] = draft.clone();
            }
            (
                reqwest::Method::POST,
                format!("{base}/projects/{}/merge_requests", encode(&project)),
                Some(payload),
            )
        }
        "merge_requests.comment" => {
            let project = config.project(args)?;
            (
                reqwest::Method::POST,
                format!(
                    "{base}/projects/{}/merge_requests/{}/notes",
                    encode(&project),
                    iid(args)?
                ),
                Some(json!({
                    "body": args
                        .get("body")
                        .and_then(Value::as_str)
                        .filter(|value| !value.trim().is_empty())
                        .ok_or_else(|| "comment body is required".to_string())?,
                })),
            )
        }
        "merge_requests.approve" => {
            let project = config.project(args)?;
            (
                reqwest::Method::POST,
                format!(
                    "{base}/projects/{}/merge_requests/{}/approve",
                    encode(&project),
                    iid(args)?
                ),
                Some(json!({})),
            )
        }
        "merge_requests.merge" => {
            let project = config.project(args)?;
            let mut payload = json!({});
            if let Some(sha) = args.get("sha") {
                payload["sha"] = sha.clone();
            }
            if let Some(message) = args
                .get("mergeCommitMessage")
                .or_else(|| args.get("merge_commit_message"))
                .and_then(Value::as_str)
            {
                payload["merge_commit_message"] = json!(message);
            }
            (
                reqwest::Method::PUT,
                format!(
                    "{base}/projects/{}/merge_requests/{}/merge",
                    encode(&project),
                    iid(args)?
                ),
                Some(payload),
            )
        }
        "pipelines.status" => {
            let project = config.project(args)?;
            let mut query = vec![("per_page", "1".to_string())];
            if let Some(reference) = args
                .get("ref")
                .or_else(|| args.get("branch"))
                .and_then(Value::as_str)
            {
                query.push(("ref", reference.to_string()));
            }
            let query = query
                .into_iter()
                .map(|(key, value)| format!("{key}={}", encode(&value)))
                .collect::<Vec<_>>()
                .join("&");
            (
                reqwest::Method::GET,
                format!("{base}/projects/{}/pipelines?{query}", encode(&project)),
                None,
            )
        }
        other => return Err(format!("unsupported GitLab capability: {other}")),
    };

    let mut request = client.request(method, url).header("PRIVATE-TOKEN", token);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("GitLab request failed: {error}"))?;
    let status = response.status();
    let body: Value = response
        .json()
        .await
        .unwrap_or_else(|_| json!({ "message": "empty GitLab response" }));
    if !status.is_success() {
        let message = body
            .get("message")
            .and_then(Value::as_str)
            .or_else(|| body.get("error").and_then(Value::as_str))
            .unwrap_or("GitLab API error");
        return Err(format!("GitLab API returned {status}: {message}"));
    }

    Ok(match capability {
        "projects.list" => {
            let projects = body
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter(|project| config.project_visible(project))
                        .map(summarize_project)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            json!({ "count": projects.len(), "projects": projects })
        }
        "issues.list" | "group_issues.list" => json!({
            "count": body.as_array().map(Vec::len).unwrap_or(0),
            "issues": body.as_array().map(|items| items.iter().map(summarize_issue).collect::<Vec<_>>()).unwrap_or_default(),
        }),
        "merge_requests.list" => json!({
            "count": body.as_array().map(Vec::len).unwrap_or(0),
            "mergeRequests": body.as_array().map(|items| items.iter().map(summarize_mr).collect::<Vec<_>>()).unwrap_or_default(),
        }),
        "merge_requests.get" | "merge_requests.create" | "merge_requests.merge" => {
            summarize_mr(&body)
        }
        "merge_requests.approve" | "merge_requests.comment" => body,
        "pipelines.status" => {
            let latest = body
                .as_array()
                .and_then(|items| items.first())
                .cloned()
                .unwrap_or(body);
            summarize_pipeline(&latest)
        }
        "boards.list" => json!({
            "count": body.as_array().map(Vec::len).unwrap_or(0),
            "boards": body.as_array().map(|items| items.iter().map(|board| json!({
                "id": board.get("id"),
                "name": board.get("name"),
                "group": board.get("group").and_then(|group| group.get("full_path")),
            })).collect::<Vec<_>>()).unwrap_or_default(),
        }),
        _ => summarize_issue(&body),
    })
}

fn summarize_project(project: &Value) -> Value {
    json!({
        "id": project.get("id"),
        "path": project.get("path_with_namespace"),
        "name": project.get("name"),
        "url": project.get("web_url"),
        "defaultBranch": project.get("default_branch"),
    })
}

fn summarize_issue(issue: &Value) -> Value {
    json!({
        "id": issue.get("id"),
        "iid": issue.get("iid"),
        "title": issue.get("title"),
        "state": issue.get("state"),
        "description": issue.get("description").and_then(Value::as_str).map(|value| value.chars().take(4000).collect::<String>()),
        "url": issue.get("web_url"),
        "projectId": issue.get("project_id"),
        "references": issue.get("references"),
        "labels": issue.get("labels"),
        "author": issue.get("author").and_then(|value| value.get("username")),
        "assignees": issue.get("assignees").and_then(Value::as_array).map(|items| items.iter().filter_map(|value| value.get("username").and_then(Value::as_str)).collect::<Vec<_>>()).unwrap_or_default(),
        "createdAt": issue.get("created_at"),
        "updatedAt": issue.get("updated_at"),
    })
}

fn summarize_mr(mr: &Value) -> Value {
    json!({
        "id": mr.get("id"),
        "iid": mr.get("iid"),
        "title": mr.get("title"),
        "state": mr.get("state"),
        "draft": mr.get("draft"),
        "url": mr.get("web_url"),
        "sourceBranch": mr.get("source_branch"),
        "targetBranch": mr.get("target_branch"),
        "author": mr.get("author").and_then(|value| value.get("username")),
        "mergeStatus": mr.get("detailed_merge_status").or_else(|| mr.get("merge_status")),
        "createdAt": mr.get("created_at"),
        "updatedAt": mr.get("updated_at"),
    })
}

fn summarize_pipeline(pipeline: &Value) -> Value {
    json!({
        "id": pipeline.get("id"),
        "iid": pipeline.get("iid"),
        "status": pipeline.get("status"),
        "ref": pipeline.get("ref"),
        "sha": pipeline.get("sha"),
        "url": pipeline.get("web_url"),
        "createdAt": pipeline.get("created_at"),
        "updatedAt": pipeline.get("updated_at"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_project_paths() {
        assert_eq!(encode("group/app"), "group%2Fapp");
    }

    #[test]
    fn validates_allowed_projects() {
        let config = GitLabClientConfig {
            base_url: "https://gitlab.com/api/v4".into(),
            token: Some("t".into()),
            allowed_projects: vec!["group/app".into()],
            default_project: None,
            group_id: None,
            allowed_groups: vec![],
        };
        assert!(config.project(&json!({"project":"group/app"})).is_ok());
        assert!(config.project(&json!({"project":"group/other"})).is_err());
    }

    fn connection(config: Value) -> GitLabClientConfig {
        GitLabClientConfig::from_connection(&config, Some("t".into()))
    }

    #[test]
    fn group_tools_are_limited_to_configured_groups() {
        let config = connection(json!({
            "groupId": "Platform",
            "allowedGroups": ["platform/infra", "/tools/"]
        }));
        assert_eq!(config.group(&json!({})).unwrap(), "Platform");
        assert!(config.group(&json!({"group": "platform"})).is_ok());
        assert!(config.group(&json!({"group": "platform/infra"})).is_ok());
        assert!(config.group(&json!({"group": "tools"})).is_ok());
        assert!(config.group(&json!({"group": "other"})).is_err());
        assert!(config
            .group(&json!({"group": "platform/infra/../x"}))
            .is_err());
        assert!(config.group(&json!({"group": "12345"})).is_err());

        let unconfigured = connection(json!({}));
        assert!(unconfigured.group(&json!({})).is_err());
        assert!(unconfigured.group(&json!({"group": "anything"})).is_err());
    }

    #[tokio::test]
    async fn group_tools_reject_other_groups_before_any_request() {
        // Port 9 (discard) is never contacted: the call must fail on the allowlist.
        let config = connection(json!({ "baseUrl": "http://127.0.0.1:9", "groupId": "platform" }));
        let client = reqwest::Client::new();
        for capability in ["group_issues.list", "boards.list"] {
            let error = invoke(&client, &config, capability, &json!({"group": "finance"}))
                .await
                .unwrap_err();
            assert!(error.contains("not the connection's groupId"), "{error}");
        }
    }

    #[test]
    fn project_listing_honours_allowed_projects() {
        let config = connection(json!({ "allowedProjects": ["Group/App", "42"] }));
        assert!(config.project_visible(&json!({"id": 1, "path_with_namespace": "group/app"})));
        assert!(config.project_visible(&json!({"id": 42, "path_with_namespace": "group/x"})));
        assert!(!config.project_visible(&json!({"id": 7, "path_with_namespace": "group/secret"})));
        let open = connection(json!({}));
        assert!(open.project_visible(&json!({"id": 7, "path_with_namespace": "group/secret"})));
    }
}
