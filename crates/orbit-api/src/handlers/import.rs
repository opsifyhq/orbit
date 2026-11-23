//! Import OpenCode-style Markdown agent and skill definitions.

use axum::extract::State;
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use orbit_domain::{AgentDefinition, ValidationIssue};
use orbit_security::{AuthContext, Permission};

use crate::audit;
use crate::error::ApiError;
use crate::state::AppState;

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImportRequest {
    /// `agent` or `skill`.
    pub kind: String,
    #[serde(default)]
    pub filename: String,
    pub content: String,
    /// Activate a newly imported agent immediately.
    #[serde(default)]
    pub activate: bool,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImportResponse {
    pub kind: String,
    pub id: Uuid,
    pub name: String,
    pub warnings: Vec<String>,
}

fn split_frontmatter(content: &str) -> (serde_yaml::Value, String) {
    let normalized = content.replace("\r\n", "\n");
    if let Some(rest) = normalized.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---") {
            let (yaml, body) = rest.split_at(end);
            let body = body.trim_start_matches("\n---").trim_start_matches('\n');
            let value = serde_yaml::from_str(yaml).unwrap_or(serde_yaml::Value::Null);
            return (value, body.trim().to_string());
        }
    }
    (serde_yaml::Value::Null, normalized.trim().to_string())
}

fn field<'a>(yaml: &'a serde_yaml::Value, key: &str) -> Option<&'a str> {
    yaml.get(key).and_then(|value| value.as_str())
}

fn slugify(value: &str) -> String {
    value
        .to_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn name_from_filename(filename: &str) -> Option<String> {
    let stem = filename
        .rsplit('/')
        .next()?
        .trim_end_matches(".md")
        .trim_end_matches(".markdown");
    if stem.is_empty() || stem.eq_ignore_ascii_case("skill") {
        None
    } else {
        Some(stem.replace(['-', '_'], " "))
    }
}

#[utoipa::path(post, path = "/api/v1/import/opencode", tag = "import", request_body = ImportRequest, responses((status = 201, body = ImportResponse)))]
pub async fn import_markdown(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    crate::extract::ApiJson(body): crate::extract::ApiJson<ImportRequest>,
) -> Result<(StatusCode, Json<ImportResponse>), ApiError> {
    auth.require(Permission::AgentsWrite)?;
    let (frontmatter, instructions) = split_frontmatter(&body.content);
    if instructions.trim().is_empty() {
        return Err(ApiError::validation(vec![ValidationIssue::new(
            "content",
            "Markdown body is empty",
        )]));
    }
    let mut warnings = Vec::new();
    match body.kind.as_str() {
        "agent" => {
            let name = field(&frontmatter, "name")
                .map(str::to_string)
                .or_else(|| name_from_filename(&body.filename))
                .ok_or_else(|| {
                    ApiError::validation(vec![ValidationIssue::new(
                        "name",
                        "frontmatter name or filename is required",
                    )])
                })?;
            let description = field(&frontmatter, "description").unwrap_or("");
            let model = field(&frontmatter, "model").unwrap_or("").trim();
            let model = if model.is_empty() {
                serde_json::json!({ "management": "harness", "params": {} })
            } else if let Some((provider, id)) = model.split_once('/') {
                serde_json::json!({
                    "management": "harness",
                    "provider": provider,
                    "model": id,
                    "params": {}
                })
            } else {
                warnings.push("model has no provider/ prefix; using the harness default".into());
                serde_json::json!({ "management": "harness", "model": model, "params": {} })
            };
            for key in ["tools", "permission", "mode", "temperature"] {
                if frontmatter.get(key).is_some() {
                    warnings.push(format!(
                        "frontmatter '{key}' is not applied to the agent; configure tool permissions on the worker profile or add tool policies"
                    ));
                }
            }
            let definition: AgentDefinition = serde_json::from_value(serde_json::json!({
                "apiVersion": orbit_domain::AGENT_API_VERSION,
                "kind": orbit_domain::AGENT_KIND,
                "metadata": { "name": name, "description": description, "labels": {} },
                "spec": {
                    "model": model,
                    "instructions": { "system": instructions, "constraints": [] }
                }
            }))
            .map_err(|error| {
                ApiError::validation(vec![ValidationIssue::new("content", error.to_string())])
            })?;
            let report = state
                .store
                .validate_agent_references(auth.org_id, &definition)
                .await?;
            if !report.is_valid() {
                return Err(ApiError::validation(report.issues));
            }
            let access = crate::access::AccessScope::load(&state, &auth).await?;
            crate::preflight::require_spec_use(&state, auth.org_id, &access, &definition.spec)
                .await?;
            // OpenCode import creates the agent directly; apply the same installation-wide
            // agent limit as `POST /api/v1/agents` before the insert.
            let _agent_limit = state.agent_limit_guard().await?;
            let (agent, _) = state
                .store
                .create_agent(auth.org_id, Some(auth.user_id), &definition)
                .await?;
            drop(_agent_limit);
            state
                .store
                .assign_resource_owner(auth.org_id, "agents", agent.id, auth.user_id)
                .await?;
            if body.activate {
                state
                    .store
                    .set_agent_status(auth.org_id, agent.id, orbit_domain::AgentStatus::Active)
                    .await?;
            }
            audit::record(
                &state,
                &auth,
                "agent.imported",
                Some("agent"),
                Some(agent.id),
                serde_json::json!({ "source": body.filename, "format": "opencode_markdown" }),
            )
            .await;
            Ok((
                StatusCode::CREATED,
                Json(ImportResponse {
                    kind: "agent".into(),
                    id: agent.id,
                    name: agent.name,
                    warnings,
                }),
            ))
        }
        "skill" => {
            let name = field(&frontmatter, "name")
                .map(str::to_string)
                .or_else(|| name_from_filename(&body.filename))
                .ok_or_else(|| {
                    ApiError::validation(vec![ValidationIssue::new(
                        "name",
                        "SKILL.md frontmatter name is required",
                    )])
                })?;
            let slug = field(&frontmatter, "slug")
                .map(slugify)
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| slugify(&name));
            if slug.is_empty() {
                return Err(ApiError::validation(vec![ValidationIssue::new(
                    "slug",
                    "skill name must contain letters or digits",
                )]));
            }
            let description = field(&frontmatter, "description").unwrap_or("");
            let skill = state
                .store
                .create_skill(
                    auth.org_id,
                    &slug,
                    &name,
                    description,
                    &instructions,
                    &serde_json::json!({}),
                )
                .await?;
            state
                .store
                .assign_resource_owner(auth.org_id, "skills", skill.id, auth.user_id)
                .await?;
            audit::record(
                &state,
                &auth,
                "skill.imported",
                Some("skill"),
                Some(skill.id),
                serde_json::json!({ "source": body.filename, "format": "opencode_markdown" }),
            )
            .await;
            Ok((
                StatusCode::CREATED,
                Json(ImportResponse {
                    kind: "skill".into(),
                    id: skill.id,
                    name: skill.name,
                    warnings,
                }),
            ))
        }
        other => Err(ApiError::bad_request(format!(
            "unknown import kind '{other}'"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_frontmatter_and_body() {
        let (yaml, body) = split_frontmatter(
            "---\nname: Docs\ndescription: Writes docs\n---\nYou are a docs agent.",
        );
        assert_eq!(field(&yaml, "name"), Some("Docs"));
        assert_eq!(body, "You are a docs agent.");
    }

    #[test]
    fn derives_name_and_slug() {
        assert_eq!(
            name_from_filename("docs-agent.md").as_deref(),
            Some("docs agent")
        );
        assert_eq!(name_from_filename("SKILL.md"), None);
        assert_eq!(slugify("Docs Agent!"), "docs-agent");
    }
}
