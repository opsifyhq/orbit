use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use orbit_domain::{is_valid_slug, ValidationIssue};
use orbit_security::{AuthContext, Permission};
use orbit_store::Skill;

use crate::audit;
use crate::error::ApiError;
use crate::state::AppState;

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateSkillRequest {
    pub name: String,
    pub slug: String,
    #[serde(default)]
    pub description: String,
    pub content: String,
    #[serde(default)]
    pub files: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSkillRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub files: Option<serde_json::Value>,
}

#[utoipa::path(get, path = "/api/v1/skills", tag = "skills", responses((status = 200, body = [Skill])))]
pub async fn list_skills(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> Result<Json<Vec<Skill>>, ApiError> {
    auth.require(Permission::AgentsRead)?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    Ok(Json(
        state
            .store
            .list_skills(auth.org_id, Some(&access.store_access()))
            .await?,
    ))
}

#[utoipa::path(post, path = "/api/v1/skills", tag = "skills", request_body = CreateSkillRequest, responses((status = 201, body = Skill)))]
pub async fn create_skill(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    crate::extract::ApiJson(body): crate::extract::ApiJson<CreateSkillRequest>,
) -> Result<(StatusCode, Json<Skill>), ApiError> {
    auth.require(Permission::AgentsWrite)?;
    validate_skill(&body.slug, &body.name, &body.content, &body.files)?;
    let skill = state
        .store
        .create_skill(
            auth.org_id,
            body.slug.trim(),
            body.name.trim(),
            body.description.trim(),
            &body.content,
            &body.files,
        )
        .await?;
    state
        .store
        .assign_resource_owner(auth.org_id, "skills", skill.id, auth.user_id)
        .await?;
    audit::record(
        &state,
        &auth,
        "skill.created",
        Some("skill"),
        Some(skill.id),
        serde_json::json!({ "slug": skill.slug }),
    )
    .await;
    Ok((StatusCode::CREATED, Json(skill)))
}

#[utoipa::path(get, path = "/api/v1/skills/{skill_id}", tag = "skills", responses((status = 200, body = Skill)))]
pub async fn get_skill(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(skill_id): Path<Uuid>,
) -> Result<Json<Skill>, ApiError> {
    auth.require(Permission::AgentsRead)?;
    let skill = state.store.get_skill(auth.org_id, skill_id).await?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    access.require_view(
        "skill",
        skill.owner_user_id,
        skill.owner_team_id,
        &skill.visibility,
    )?;
    Ok(Json(skill))
}

#[utoipa::path(patch, path = "/api/v1/skills/{skill_id}", tag = "skills", request_body = UpdateSkillRequest, responses((status = 200, body = Skill)))]
pub async fn update_skill(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(skill_id): Path<Uuid>,
    crate::extract::ApiJson(body): crate::extract::ApiJson<UpdateSkillRequest>,
) -> Result<Json<Skill>, ApiError> {
    auth.require(Permission::AgentsWrite)?;
    let existing = state.store.get_skill(auth.org_id, skill_id).await?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    access.require_manage(
        "skill",
        existing.owner_user_id,
        existing.owner_team_id,
        &existing.visibility,
    )?;
    if let Some(content) = &body.content {
        if content.trim().is_empty() {
            return Err(ApiError::validation(vec![ValidationIssue::new(
                "content",
                "SKILL.md content is required",
            )]));
        }
    }
    if let Some(files) = &body.files {
        validate_files(files).map_err(ApiError::validation)?;
    }
    let skill = state
        .store
        .update_skill(
            auth.org_id,
            skill_id,
            body.name.as_deref().map(str::trim),
            body.description.as_deref(),
            body.content.as_deref(),
            body.files.as_ref(),
        )
        .await?;
    audit::record(
        &state,
        &auth,
        "skill.updated",
        Some("skill"),
        Some(skill.id),
        serde_json::json!({ "version": skill.version }),
    )
    .await;
    Ok(Json(skill))
}

#[utoipa::path(delete, path = "/api/v1/skills/{skill_id}", tag = "skills", responses((status = 204)))]
pub async fn delete_skill(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(skill_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::AgentsWrite)?;
    let existing = state.store.get_skill(auth.org_id, skill_id).await?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    access.require_manage(
        "skill",
        existing.owner_user_id,
        existing.owner_team_id,
        &existing.visibility,
    )?;
    state.store.delete_skill(auth.org_id, skill_id).await?;
    audit::record(
        &state,
        &auth,
        "skill.deleted",
        Some("skill"),
        Some(skill_id),
        serde_json::json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) fn validate_skill(
    slug: &str,
    name: &str,
    content: &str,
    files: &serde_json::Value,
) -> Result<(), ApiError> {
    let mut issues = Vec::new();
    if !is_valid_slug(slug) {
        issues.push(ValidationIssue::new(
            "slug",
            "lowercase letters, digits, - or _",
        ));
    }
    if name.trim().is_empty() || name.trim().chars().count() > orbit_domain::MAX_NAME_CHARS {
        issues.push(ValidationIssue::new(
            "name",
            format!(
                "name must be 1..={} characters",
                orbit_domain::MAX_NAME_CHARS
            ),
        ));
    }
    if content.trim().is_empty() {
        issues.push(ValidationIssue::new(
            "content",
            "SKILL.md content is required",
        ));
    }
    if let Err(report) = validate_files(files) {
        issues.extend(report);
    }
    if issues.is_empty() {
        Ok(())
    } else {
        Err(ApiError::validation(issues))
    }
}

fn validate_files(files: &serde_json::Value) -> Result<(), Vec<ValidationIssue>> {
    let mut issues = Vec::new();
    if files.is_null() {
        return Ok(());
    }
    let Some(map) = files.as_object() else {
        return Err(vec![ValidationIssue::new(
            "files",
            "files must be an object of path → content",
        )]);
    };
    if map.len() > 50 {
        issues.push(ValidationIssue::new("files", "at most 50 files per skill"));
    }
    let allowed_script_exts = [".sh", ".bash", ".py", ".js", ".ts", ".rb", ".pl", ".go"];
    let mut script_bytes = 0usize;
    for (path, value) in map {
        let safe = !path.starts_with('/')
            && !path.contains("..")
            && !path.contains('\\')
            && path.len() <= 200
            && value.as_str().is_some();
        if !safe {
            issues.push(ValidationIssue::new(
                format!("files.{path}"),
                "use a relative file path with string content",
            ));
            continue;
        }
        let is_script = path.starts_with("scripts/") || path.contains("/scripts/");
        if is_script {
            let has_allowed_ext = allowed_script_exts.iter().any(|ext| path.ends_with(ext));
            if !has_allowed_ext {
                issues.push(ValidationIssue::new(
                    format!("files.{path}"),
                    format!(
                        "scripts must have an allowed extension: {}",
                        allowed_script_exts.join(", ")
                    ),
                ));
            }
            if let Some(content) = value.as_str() {
                let bytes = content.len();
                if bytes > 100_000 {
                    issues.push(ValidationIssue::new(
                        format!("files.{path}"),
                        "script must be <= 100KB",
                    ));
                }
                script_bytes += bytes;
            }
        }
    }
    if script_bytes > 1_000_000 {
        issues.push(ValidationIssue::new(
            "files",
            "total script content must be <= 1MB",
        ));
    }
    if issues.is_empty() {
        Ok(())
    } else {
        Err(issues)
    }
}
