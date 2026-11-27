use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use orbit_domain::{HarnessKind, ValidationIssue};
use orbit_security::{issue_worker_token, AuthContext, Permission};
use orbit_store::WorkerProfile;

use crate::audit;
use crate::error::ApiError;
use crate::paging::{Page, PageQuery};
use crate::patch::{merge_patch, parse_patch};
use crate::state::AppState;
use crate::worker_tokens::{issue_profile_token, IssuedWorkerToken};

#[derive(Debug, Clone, Deserialize, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateWorkerProfileRequest {
    pub name: String,
    pub harness_kind: String,
    #[serde(default = "default_target")]
    pub target: String,
    #[serde(default = "default_driver")]
    pub execution_driver: String,
    #[serde(default)]
    pub image: String,
    #[serde(default = "default_tag")]
    pub image_tag: String,
    /// `all` (dynamic) or `selected`; omitted derives from agentIds for compatibility.
    #[serde(default)]
    pub agent_scope: Option<String>,
    #[serde(default)]
    pub agent_ids: Vec<Uuid>,
    #[serde(default)]
    pub skill_slugs: Vec<String>,
    #[serde(default)]
    pub auth_secret_name: Option<String>,
    /// Variable name -> value. A string sets or replaces the value; `null` keeps the
    /// stored value (422 when none is stored); a name left out is removed. Responses never
    /// contain values, only `{ "NAME": { "hasValue": bool } }`.
    #[serde(default)]
    #[schema(value_type = std::collections::BTreeMap<String, Option<String>>)]
    pub env: serde_json::Value,
    /// Inline references only: `envVar` plus `connectionId`, `secretKey` or `secretRef`.
    #[serde(default)]
    pub env_mappings: serde_json::Value,
    /// Reusable environment mapping IDs attached to this profile.
    #[serde(default)]
    pub env_mapping_ids: Vec<Uuid>,
    #[serde(default)]
    pub resources: serde_json::Value,
    /// Allow OpenCode's built-in Web Search tool. Default false for headless runs.
    #[serde(default)]
    pub allow_web_search: bool,
    /// Allow OpenCode's built-in WebFetch tool. Default false for headless runs.
    #[serde(default)]
    pub allow_web_fetch: bool,
    #[serde(default)]
    pub allow_glab: bool,
    #[serde(default)]
    pub allow_gh: bool,
    /// Native harness tool name -> allow|ask|deny (OpenCode permission map).
    #[serde(default)]
    pub native_tool_permissions: serde_json::Value,
}

async fn require_profile_manage(
    state: &AppState,
    auth: &AuthContext,
    profile_id: Uuid,
) -> Result<WorkerProfile, ApiError> {
    let profile = state
        .store
        .get_worker_profile(auth.org_id, profile_id)
        .await?;
    let access = crate::access::AccessScope::load(state, auth).await?;
    access.require_manage(
        "worker profile",
        profile.owner_user_id,
        profile.owner_team_id,
        &profile.visibility,
    )?;
    Ok(profile)
}

/// Merge the requested `env` onto what is stored. Strings replace, `null` keeps the stored
/// value, names that are not sent are dropped. Errors name the variable, never a value.
pub(crate) fn merge_profile_env(
    existing: Option<&serde_json::Value>,
    requested: &serde_json::Value,
) -> Result<serde_json::Value, ApiError> {
    let empty = serde_json::Map::new();
    let requested = match requested {
        serde_json::Value::Null => &empty,
        serde_json::Value::Object(map) => map,
        _ => {
            return Err(ApiError::validation(vec![ValidationIssue::new(
                "env",
                "must be an object of variable name -> value (or null to keep the stored value)",
            )]))
        }
    };
    let stored = existing.and_then(|value| value.as_object());
    let mut merged = serde_json::Map::new();
    let mut issues = Vec::new();
    for (name, value) in requested {
        let field = format!("env.{name}");
        if name.trim().is_empty() {
            issues.push(ValidationIssue::new(
                "env",
                "variable names must not be empty",
            ));
            continue;
        }
        match value {
            serde_json::Value::String(text) => {
                merged.insert(name.clone(), serde_json::Value::String(text.clone()));
            }
            serde_json::Value::Null => match stored.and_then(|map| map.get(name)) {
                Some(kept) => {
                    merged.insert(name.clone(), kept.clone());
                }
                None => issues.push(ValidationIssue::new(
                    field,
                    "no stored value to keep; send a string value",
                )),
            },
            _ => issues.push(ValidationIssue::new(
                field,
                "must be a string, or null to keep the stored value",
            )),
        }
    }
    if issues.is_empty() {
        Ok(serde_json::Value::Object(merged))
    } else {
        Err(ApiError::validation(issues))
    }
}

/// Inline mappings carry references, never values: unknown keys are rejected so a value
/// cannot be stored (and later returned) under an extra key.
pub(crate) fn validate_inline_env_mappings(value: &serde_json::Value) -> Result<(), ApiError> {
    let items = match value {
        serde_json::Value::Null => return Ok(()),
        // Older installs stored `{}` for "none".
        serde_json::Value::Object(map) if map.is_empty() => return Ok(()),
        serde_json::Value::Array(items) => items,
        _ => {
            return Err(ApiError::validation(vec![ValidationIssue::new(
                "envMappings",
                "must be a list of mappings",
            )]))
        }
    };
    let mut issues = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let Some(map) = item.as_object() else {
            issues.push(ValidationIssue::new(
                "envMappings",
                format!("mapping {index} must be an object"),
            ));
            continue;
        };
        for (key, field) in map {
            if !orbit_store::INLINE_ENV_MAPPING_KEYS.contains(&key.as_str()) {
                issues.push(ValidationIssue::new(
                    "envMappings",
                    format!(
                        "mapping {index}: unknown field '{key}'; mappings reference a connection or secret and cannot hold a value"
                    ),
                ));
            } else if !(field.is_null() || field.is_string()) {
                issues.push(ValidationIssue::new(
                    "envMappings",
                    format!("mapping {index}: '{key}' must be a string"),
                ));
            }
        }
        if map
            .get("envVar")
            .and_then(|v| v.as_str())
            .is_none_or(|v| v.trim().is_empty())
        {
            issues.push(ValidationIssue::new(
                "envMappings",
                format!("mapping {index} is missing envVar"),
            ));
        }
    }
    if issues.is_empty() {
        Ok(())
    } else {
        Err(ApiError::validation(issues))
    }
}

/// `authSecretName` must be a valid name allowed by the installer's list
/// (`ORBIT_WORKER_AUTH_SECRETS`); the launcher enforces the same rule at launch.
pub(crate) fn validate_auth_secret_name(name: Option<&str>) -> Result<(), ApiError> {
    let Some(name) = name.map(str::trim).filter(|name| !name.is_empty()) else {
        return Ok(());
    };
    orbit_domain::AuthSecretPolicy::from_env()
        .check(name)
        .map_err(|message| {
            ApiError::validation(vec![ValidationIssue::new("authSecretName", message)])
        })
}

/// Bind a harness auth Secret to a profile (#325). The name rule above only says which
/// Secrets are harness auth at all; it does not say whose. A profile editor may keep the
/// profile's current Secret or clear it, and may reuse a Secret already bound to another
/// profile they manage (they can mount it through that profile anyway). Binding any
/// other Secret, for example another team's, takes an Owner or Admin.
pub(crate) async fn require_auth_secret_binding(
    state: &AppState,
    auth: &AuthContext,
    access: &crate::access::AccessScope,
    profile_id: Option<Uuid>,
    previous: Option<&str>,
    next: Option<&str>,
) -> Result<(), ApiError> {
    let normalize = |name: Option<&str>| {
        name.map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
    };
    let Some(next) = normalize(next) else {
        return Ok(());
    };
    if normalize(previous).as_deref() == Some(next.as_str()) || access.is_privileged() {
        return Ok(());
    }
    let bound_elsewhere = state
        .store
        .list_worker_profiles(auth.org_id, None)
        .await?
        .into_iter()
        .any(|profile| {
            Some(profile.id) != profile_id
                && profile.auth_secret_name.as_deref().map(str::trim) == Some(next.as_str())
                && access.can_manage(profile.owner_user_id, profile.owner_team_id)
        });
    if bound_elsewhere {
        Ok(())
    } else {
        Err(ApiError::forbidden(format!(
            "authSecretName '{next}' is not bound to a worker profile you manage; an Owner or Admin must bind a new harness auth Secret"
        )))
    }
}

fn default_target() -> String {
    "docker".into()
}
fn default_driver() -> String {
    "process".into()
}
fn default_tag() -> String {
    String::new()
}

fn resolve_agent_scope(requested: Option<&str>, agent_ids: &[Uuid]) -> Result<String, ApiError> {
    match requested.map(str::trim) {
        Some("all") => Ok("all".into()),
        Some("selected") => Ok("selected".into()),
        Some("") | None => Ok(if agent_ids.is_empty() {
            "all".into()
        } else {
            "selected".into()
        }),
        Some(other) => Err(ApiError::bad_request(format!(
            "agentScope must be 'all' or 'selected', got '{other}'"
        ))),
    }
}

pub(crate) fn validate_native_tool_permissions(value: &serde_json::Value) -> Result<(), ApiError> {
    let Some(map) = value.as_object() else {
        if value.is_null() {
            return Ok(());
        }
        return Err(ApiError::validation(vec![
            orbit_domain::ValidationIssue::new(
                "nativeToolPermissions",
                "must be an object of tool -> allow|ask|deny",
            ),
        ]));
    };
    for (tool, decision) in map {
        if tool.trim().is_empty() || !matches!(decision.as_str(), Some("allow" | "ask" | "deny")) {
            return Err(ApiError::validation(vec![
                orbit_domain::ValidationIssue::new(
                    "nativeToolPermissions",
                    format!("invalid permission for tool '{tool}'"),
                ),
            ]));
        }
    }
    Ok(())
}

pub(crate) fn resolve_worker_resources(
    value: &serde_json::Value,
) -> Result<serde_json::Value, ApiError> {
    let resources: orbit_domain::WorkerResources =
        if value.is_null() || value.as_object().is_some_and(|object| object.is_empty()) {
            orbit_domain::WorkerResources::defaults()
        } else {
            serde_json::from_value(value.clone()).map_err(|error| {
                ApiError::validation(vec![orbit_domain::ValidationIssue::new(
                    "resources",
                    format!("invalid worker resources: {error}"),
                )])
            })?
        }
        .with_defaults();
    let issues = resources.validate();
    if !issues.is_empty() {
        return Err(ApiError::validation(
            issues
                .into_iter()
                .map(|issue| orbit_domain::ValidationIssue::new("resources", issue))
                .collect(),
        ));
    }
    serde_json::to_value(resources).map_err(|error| ApiError::internal(error.to_string()))
}

#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkerBootstrap {
    pub docker: String,
    pub compose: String,
    pub kubernetes: String,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateWorkerTokenRequest {
    pub name: String,
    #[serde(default)]
    pub harnesses: Vec<String>,
    #[serde(default)]
    pub profile_id: Option<Uuid>,
}

#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkerTokenView {
    pub id: Uuid,
    pub name: String,
    pub profile_id: Option<Uuid>,
    pub harnesses: Vec<String>,
    pub token_prefix: String,
    pub last_used_at: Option<chrono::DateTime<chrono::Utc>>,
    pub revoked_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[utoipa::path(post, path = "/api/v1/worker-profiles/{profile_id}/token", tag = "workers", responses((status = 200, body = IssuedWorkerToken)))]
pub async fn rotate_token(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(profile_id): Path<Uuid>,
) -> Result<Json<IssuedWorkerToken>, ApiError> {
    auth.require(Permission::SettingsWrite)?;
    let profile = require_profile_manage(&state, &auth, profile_id).await?;
    Ok(Json(
        issue_profile_token(&state, auth.org_id, &profile, true).await?,
    ))
}

#[utoipa::path(post, path = "/api/v1/worker-tokens", tag = "workers", request_body = CreateWorkerTokenRequest, responses((status = 201, body = IssuedWorkerToken)))]
pub async fn create_token(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    crate::extract::ApiJson(body): crate::extract::ApiJson<CreateWorkerTokenRequest>,
) -> Result<(StatusCode, Json<IssuedWorkerToken>), ApiError> {
    auth.require(Permission::SettingsWrite)?;
    let profile = match body.profile_id {
        Some(profile_id) => Some(require_profile_manage(&state, &auth, profile_id).await?),
        None => {
            // An org-wide token (no profile) can claim any team's executions.
            if !crate::access::AccessScope::load(&state, &auth)
                .await?
                .is_privileged()
            {
                return Err(ApiError::forbidden(
                    "only an owner or admin can create an organisation-wide worker token",
                ));
            }
            None
        }
    };
    if let Some(issue) = orbit_domain::name_length_issue("name", &body.name) {
        return Err(ApiError::validation(vec![issue]));
    }
    let mut harnesses = body.harnesses;
    if let Some(profile) = &profile {
        harnesses = vec![profile.harness_kind.clone()];
    }
    harnesses.sort();
    harnesses.dedup();
    if harnesses.is_empty() {
        return Err(ApiError::validation(vec![ValidationIssue::new(
            "harnesses",
            "at least one harness is required",
        )]));
    }
    // A token for a harness kind that does not exist can never claim anything (#438).
    let unknown: Vec<ValidationIssue> = harnesses
        .iter()
        .filter(|kind| kind.parse::<orbit_domain::HarnessKind>().is_err())
        .map(|kind| {
            ValidationIssue::new(
                "harnesses",
                format!("unknown harness '{kind}'; use mock, opencode, pi, claude_code or generic"),
            )
        })
        .collect();
    if !unknown.is_empty() {
        return Err(ApiError::validation(unknown));
    }
    let issued = issue_worker_token();
    let record = state
        .store
        .create_worker_token(
            auth.org_id,
            body.profile_id,
            body.name.trim(),
            &issued.prefix,
            &issued.hash,
            &harnesses,
        )
        .await?;
    audit::record(
        &state,
        &auth,
        "worker_token.created",
        Some("worker_token"),
        Some(record.id),
        serde_json::json!({ "name": record.name, "harnesses": record.harnesses }),
    )
    .await;
    Ok((
        StatusCode::CREATED,
        Json(IssuedWorkerToken {
            token: issued.token,
            token_id: record.id,
            profile_id: record.profile_id,
            harnesses: record.harnesses,
            name: record.name,
        }),
    ))
}

#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct WorkerTokenListQuery {
    /// Include revoked tokens. Off by default; revoked tokens can be viewed when needed.
    pub include_revoked: Option<bool>,
    pub limit: Option<i64>,
    pub cursor: Option<String>,
}

impl WorkerTokenListQuery {
    fn page(&self) -> PageQuery {
        PageQuery {
            limit: self.limit,
            cursor: self.cursor.clone(),
        }
    }
}

#[utoipa::path(get, path = "/api/v1/worker-tokens", tag = "workers", params(WorkerTokenListQuery), responses((status = 200, body = Page<WorkerTokenView>)))]
pub async fn list_tokens(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<WorkerTokenListQuery>,
) -> Result<Json<Page<WorkerTokenView>>, ApiError> {
    auth.require(Permission::HarnessesRead)?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    // Tokens follow the visibility of their profile; org-wide tokens are privileged-only.
    let visible_profiles: std::collections::HashSet<Uuid> = state
        .store
        .list_worker_profiles(auth.org_id, Some(&access.store_access()))
        .await?
        .into_iter()
        .map(|profile| profile.id)
        .collect();
    let tokens = state
        .store
        .list_worker_tokens(auth.org_id)
        .await?
        .into_iter()
        .filter(|record| match record.profile_id {
            Some(id) => visible_profiles.contains(&id),
            None => access.is_privileged(),
        })
        .filter(|record| query.include_revoked.unwrap_or(false) || record.revoked_at.is_none())
        .map(|record| WorkerTokenView {
            id: record.id,
            name: record.name,
            profile_id: record.profile_id,
            harnesses: record.harnesses,
            token_prefix: record.token_prefix,
            last_used_at: record.last_used_at,
            revoked_at: record.revoked_at,
            created_at: record.created_at,
        })
        .collect();
    Ok(Json(Page::from_vec(tokens, &query.page(), 100)?))
}

#[utoipa::path(delete, path = "/api/v1/worker-tokens/{token_id}", tag = "workers", responses((status = 204)))]
pub async fn revoke_token(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(token_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::SettingsWrite)?;
    let token = state
        .store
        .list_worker_tokens(auth.org_id)
        .await?
        .into_iter()
        .find(|token| token.id == token_id)
        .ok_or_else(|| ApiError::not_found("worker token"))?;
    match token.profile_id {
        Some(profile_id) => {
            require_profile_manage(&state, &auth, profile_id).await?;
        }
        None => {
            if !crate::access::AccessScope::load(&state, &auth)
                .await?
                .is_privileged()
            {
                return Err(ApiError::not_found("worker token"));
            }
        }
    }
    state
        .store
        .revoke_worker_token(auth.org_id, token_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(get, path = "/api/v1/worker-profiles", tag = "workers", params(PageQuery), responses((status = 200, body = Page<WorkerProfile>)))]
pub async fn list_profiles(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<PageQuery>,
) -> Result<Json<Page<WorkerProfile>>, ApiError> {
    auth.require(Permission::HarnessesRead)?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    let profiles = state
        .store
        .list_worker_profiles(auth.org_id, Some(&access.store_access()))
        .await?;
    Ok(Json(Page::from_vec(profiles, &query, 100)?))
}

#[utoipa::path(post, path = "/api/v1/worker-profiles", tag = "workers", request_body = CreateWorkerProfileRequest, responses((status = 201, body = WorkerProfile)))]
pub async fn create_profile(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    crate::extract::ApiJson(body): crate::extract::ApiJson<CreateWorkerProfileRequest>,
) -> Result<(StatusCode, Json<WorkerProfile>), ApiError> {
    auth.require(Permission::SettingsWrite)?;
    validate_native_tool_permissions(&body.native_tool_permissions)?;
    validate_auth_secret_name(body.auth_secret_name.as_deref())?;
    validate_inline_env_mappings(&body.env_mappings)?;
    let env = merge_profile_env(None, &body.env)?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    require_auth_secret_binding(
        &state,
        &auth,
        &access,
        None,
        None,
        body.auth_secret_name.as_deref(),
    )
    .await?;
    crate::handlers::env_mappings::validate_profile_mapping_ids(
        &state,
        &auth,
        &body.env_mapping_ids,
    )
    .await?;
    // Worker seats are consumed by running launches, not by enabled profile definitions;
    // launch_profile_internal enforces the limit when a worker is actually started.
    let harness = HarnessKind::from_str_compat(&body.harness_kind)?;
    if !matches!(body.target.as_str(), "local" | "docker" | "kubernetes") {
        return Err(ApiError::validation(vec![ValidationIssue::new(
            "target",
            "must be local, docker or kubernetes",
        )]));
    }
    if !matches!(body.execution_driver.as_str(), "process" | "docker") {
        return Err(ApiError::validation(vec![ValidationIssue::new(
            "executionDriver",
            "must be process or docker",
        )]));
    }
    let mut issues = Vec::new();
    if body.name.trim().is_empty() {
        issues.push(ValidationIssue::new("name", "name is required"));
    }
    issues.extend(orbit_domain::name_length_issue("name", &body.name));
    for slug in &body.skill_slugs {
        match state.store.get_skill_by_slug(auth.org_id, slug).await? {
            Some(skill)
                if access.can_view(skill.owner_user_id, skill.owner_team_id, &skill.visibility) => {
            }
            Some(_) => issues.push(ValidationIssue::new(
                "skillSlugs",
                format!("skill '{slug}' is not visible to you"),
            )),
            None => issues.push(ValidationIssue::new(
                "skillSlugs",
                format!("unknown skill '{slug}'"),
            )),
        }
    }
    for agent_id in &body.agent_ids {
        match state.store.get_agent(auth.org_id, *agent_id).await {
            Ok(agent)
                if access.can_view(agent.owner_user_id, agent.owner_team_id, &agent.visibility) => {
            }
            Ok(_) => issues.push(ValidationIssue::new(
                "agentIds",
                format!("agent '{agent_id}' is not visible to you"),
            )),
            Err(_) => issues.push(ValidationIssue::new(
                "agentIds",
                format!("unknown agent '{agent_id}'"),
            )),
        }
    }
    if !issues.is_empty() {
        return Err(ApiError::validation(issues));
    }
    let image = if body.image.trim().is_empty() {
        crate::images::default_image(harness)
    } else {
        body.image.trim().to_string()
    };
    // Empty tag follows the current platform image tag at launch time.
    let image_tag = body.image_tag.trim().to_string();
    let agent_scope = resolve_agent_scope(body.agent_scope.as_deref(), &body.agent_ids)?;
    let resources = resolve_worker_resources(&body.resources)?;
    let profile = state
        .store
        .create_worker_profile(
            auth.org_id,
            body.name.trim(),
            harness.as_str(),
            &body.target,
            &body.execution_driver,
            &image,
            &image_tag,
            &agent_scope,
            &body.agent_ids,
            &body.skill_slugs,
            None,
            None,
            body.auth_secret_name.as_deref(),
            &env,
            &body.env_mappings,
            &resources,
            body.allow_web_search,
            body.allow_web_fetch,
            body.allow_glab,
            body.allow_gh,
            &body.native_tool_permissions,
        )
        .await?;
    state
        .store
        .set_worker_profile_env_mappings(auth.org_id, profile.id, &body.env_mapping_ids)
        .await?;
    state
        .store
        .assign_resource_owner(auth.org_id, "worker_profiles", profile.id, auth.user_id)
        .await?;
    audit::record(
        &state,
        &auth,
        "worker_profile.created",
        Some("worker_profile"),
        Some(profile.id),
        serde_json::json!({ "harness": profile.harness_kind, "target": profile.target }),
    )
    .await;
    Ok((StatusCode::CREATED, Json(profile)))
}

/// Stored profile as a request body. Env values are write-only, so each stored name is
/// represented by `null`: with no `env` in the PATCH body every stored value is kept.
fn stored_profile_request(profile: &WorkerProfile, env_mapping_ids: &[Uuid]) -> serde_json::Value {
    let env = profile
        .env
        .as_object()
        .map(|map| {
            map.keys()
                .map(|name| (name.clone(), serde_json::Value::Null))
                .collect::<serde_json::Map<_, _>>()
        })
        .unwrap_or_default();
    serde_json::json!({
        "name": profile.name,
        "harnessKind": profile.harness_kind,
        "target": profile.target,
        "executionDriver": profile.execution_driver,
        "image": profile.image,
        "imageTag": profile.image_tag,
        "agentScope": profile.agent_scope,
        "agentIds": profile.agent_ids,
        "skillSlugs": profile.skill_slugs,
        "authSecretName": profile.auth_secret_name,
        "env": env,
        "envMappings": profile.env_mappings,
        "envMappingIds": env_mapping_ids,
        "resources": profile.resources,
        "allowWebSearch": profile.allow_web_search,
        "allowWebFetch": profile.allow_web_fetch,
        "allowGlab": profile.allow_glab,
        "allowGh": profile.allow_gh,
        "nativeToolPermissions": profile.native_tool_permissions,
    })
}

#[utoipa::path(patch, path = "/api/v1/worker-profiles/{profile_id}", tag = "workers", request_body = CreateWorkerProfileRequest, responses((status = 200, body = WorkerProfile)))]
pub async fn update_profile(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(profile_id): Path<Uuid>,
    crate::extract::ApiJson(patch): crate::extract::ApiJson<serde_json::Value>,
) -> Result<Json<WorkerProfile>, ApiError> {
    auth.require(Permission::SettingsWrite)?;
    let existing = state
        .store
        .get_worker_profile(auth.org_id, profile_id)
        .await?;
    let mut attached_before: Vec<Uuid> = state
        .store
        .list_worker_profile_env_mappings(profile_id)
        .await?
        .into_iter()
        .map(|mapping| mapping.id)
        .collect();
    attached_before.sort();
    let merged = merge_patch(
        stored_profile_request(&existing, &attached_before),
        patch,
        &[],
    )?;
    let body: CreateWorkerProfileRequest = parse_patch(merged)?;
    validate_native_tool_permissions(&body.native_tool_permissions)?;
    validate_auth_secret_name(body.auth_secret_name.as_deref())?;
    validate_inline_env_mappings(&body.env_mappings)?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    access.require_manage(
        "worker profile",
        existing.owner_user_id,
        existing.owner_team_id,
        &existing.visibility,
    )?;
    require_auth_secret_binding(
        &state,
        &auth,
        &access,
        Some(profile_id),
        existing.auth_secret_name.as_deref(),
        body.auth_secret_name.as_deref(),
    )
    .await?;
    let env = merge_profile_env(Some(&existing.env), &body.env)?;
    crate::handlers::env_mappings::validate_profile_mapping_ids(
        &state,
        &auth,
        &body.env_mapping_ids,
    )
    .await?;
    // Worker seats are consumed by running launches, not by enabled profile definitions;
    // launch_profile_internal enforces the limit when a worker is actually started.
    let harness = HarnessKind::from_str_compat(&body.harness_kind)?;
    if !matches!(body.target.as_str(), "local" | "docker" | "kubernetes") {
        return Err(ApiError::validation(vec![ValidationIssue::new(
            "target",
            "must be local, docker or kubernetes",
        )]));
    }
    if !matches!(body.execution_driver.as_str(), "process" | "docker") {
        return Err(ApiError::validation(vec![ValidationIssue::new(
            "executionDriver",
            "must be process or docker",
        )]));
    }
    let mut issues = Vec::new();
    if body.name.trim().is_empty() {
        issues.push(ValidationIssue::new("name", "name is required"));
    }
    issues.extend(orbit_domain::name_length_issue("name", &body.name));
    for slug in &body.skill_slugs {
        match state.store.get_skill_by_slug(auth.org_id, slug).await? {
            Some(skill)
                if access.can_view(skill.owner_user_id, skill.owner_team_id, &skill.visibility) => {
            }
            Some(_) => issues.push(ValidationIssue::new(
                "skillSlugs",
                format!("skill '{slug}' is not visible to you"),
            )),
            None => issues.push(ValidationIssue::new(
                "skillSlugs",
                format!("unknown skill '{slug}'"),
            )),
        }
    }
    for agent_id in &body.agent_ids {
        match state.store.get_agent(auth.org_id, *agent_id).await {
            Ok(agent)
                if access.can_view(agent.owner_user_id, agent.owner_team_id, &agent.visibility) => {
            }
            Ok(_) => issues.push(ValidationIssue::new(
                "agentIds",
                format!("agent '{agent_id}' is not visible to you"),
            )),
            Err(_) => issues.push(ValidationIssue::new(
                "agentIds",
                format!("unknown agent '{agent_id}'"),
            )),
        }
    }
    if !issues.is_empty() {
        return Err(ApiError::validation(issues));
    }
    let image = if body.image.trim().is_empty() {
        crate::images::default_image(harness)
    } else {
        body.image.trim().to_string()
    };
    // Empty tag follows the current platform image tag at launch time.
    let image_tag = body.image_tag.trim().to_string();
    let agent_scope = resolve_agent_scope(body.agent_scope.as_deref(), &body.agent_ids)?;
    let resources = resolve_worker_resources(&body.resources)?;
    let profile = state
        .store
        .update_worker_profile(
            auth.org_id,
            profile_id,
            body.name.trim(),
            harness.as_str(),
            &body.target,
            &body.execution_driver,
            &image,
            &image_tag,
            &agent_scope,
            &body.agent_ids,
            &body.skill_slugs,
            None,
            None,
            body.auth_secret_name.as_deref(),
            &env,
            &body.env_mappings,
            &resources,
            body.allow_web_search,
            body.allow_web_fetch,
            body.allow_glab,
            body.allow_gh,
            &body.native_tool_permissions,
        )
        .await?;
    state
        .store
        .set_worker_profile_env_mappings(auth.org_id, profile.id, &body.env_mapping_ids)
        .await?;
    let mut attached_after: Vec<Uuid> = state
        .store
        .list_worker_profile_env_mappings(profile_id)
        .await?
        .into_iter()
        .map(|mapping| mapping.id)
        .collect();
    attached_after.sort();
    let restart_required = attached_before != attached_after
        || existing.harness_kind != profile.harness_kind
        || existing.target != profile.target
        || existing.execution_driver != profile.execution_driver
        || existing.image != profile.image
        || existing.image_tag != profile.image_tag
        || existing.agent_scope != profile.agent_scope
        || existing.agent_ids != profile.agent_ids
        || existing.skill_slugs != profile.skill_slugs
        || existing.auth_secret_name != profile.auth_secret_name
        || existing.env != profile.env
        || existing.env_mappings != profile.env_mappings
        || existing.resources != profile.resources;
    if restart_required {
        let marked = state
            .store
            .mark_profile_launches_relaunch_required(auth.org_id, profile.id)
            .await?;
        if marked > 0 {
            tracing::info!(
                profile_id = %profile.id,
                launches = marked,
                "worker profile changed; relaunch required"
            );
        }
    }
    audit::record(
        &state,
        &auth,
        "worker_profile.updated",
        Some("worker_profile"),
        Some(profile.id),
        serde_json::json!({
            "harness": profile.harness_kind,
            "target": profile.target,
            "relaunchRequired": restart_required,
        }),
    )
    .await;
    Ok(Json(profile))
}

#[utoipa::path(get, path = "/api/v1/worker-profiles/{profile_id}", tag = "workers", responses((status = 200, body = WorkerProfile)))]
pub async fn get_profile(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(profile_id): Path<Uuid>,
) -> Result<Json<WorkerProfile>, ApiError> {
    auth.require(Permission::HarnessesRead)?;
    let profile = state
        .store
        .get_worker_profile(auth.org_id, profile_id)
        .await?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    access.require_view(
        "worker profile",
        profile.owner_user_id,
        profile.owner_team_id,
        &profile.visibility,
    )?;
    Ok(Json(profile))
}

#[utoipa::path(post, path = "/api/v1/worker-profiles/{profile_id}/enable", tag = "workers", responses((status = 200, body = WorkerProfile)))]
pub async fn enable_profile(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(profile_id): Path<Uuid>,
) -> Result<Json<WorkerProfile>, ApiError> {
    auth.require(Permission::SettingsWrite)?;
    require_profile_manage(&state, &auth, profile_id).await?;
    Ok(Json(
        state
            .store
            .set_worker_profile_enabled(auth.org_id, profile_id, true)
            .await?,
    ))
}

#[utoipa::path(post, path = "/api/v1/worker-profiles/{profile_id}/disable", tag = "workers", responses((status = 200, body = WorkerProfile)))]
pub async fn disable_profile(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(profile_id): Path<Uuid>,
) -> Result<Json<WorkerProfile>, ApiError> {
    auth.require(Permission::SettingsWrite)?;
    require_profile_manage(&state, &auth, profile_id).await?;
    Ok(Json(
        state
            .store
            .set_worker_profile_enabled(auth.org_id, profile_id, false)
            .await?,
    ))
}

#[utoipa::path(delete, path = "/api/v1/worker-profiles/{profile_id}", tag = "workers", responses((status = 204), (status = 409)))]
pub async fn delete_profile(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(profile_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::SettingsWrite)?;
    let profile = require_profile_manage(&state, &auth, profile_id).await?;
    if profile.enabled {
        return Err(ApiError::conflict(
            "disable the worker profile before deleting it",
        ));
    }
    state
        .store
        .delete_worker_profile(auth.org_id, profile_id)
        .await?;
    audit::record(
        &state,
        &auth,
        "worker_profile.deleted",
        Some("worker_profile"),
        Some(profile_id),
        serde_json::json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

/// Generate Docker, Compose and Kubernetes snippets for starting a worker by hand.
///
/// Each call issues one new worker token for the profile and leaves the existing ones
/// alone: launched and previously bootstrapped workers keep running (#306). Rotating
/// every token of a profile is the separate, explicit `POST …/token`.
#[utoipa::path(post, path = "/api/v1/worker-profiles/{profile_id}/bootstrap", tag = "workers", responses((status = 200, body = WorkerBootstrap)))]
pub async fn bootstrap(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(profile_id): Path<Uuid>,
) -> Result<Json<WorkerBootstrap>, ApiError> {
    auth.require(Permission::SettingsWrite)?;
    let profile = state
        .store
        .get_worker_profile(auth.org_id, profile_id)
        .await?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    access.require_manage(
        "worker profile",
        profile.owner_user_id,
        profile.owner_team_id,
        &profile.visibility,
    )?;
    if !profile.enabled {
        return Err(ApiError::conflict(
            "enable the worker profile before generating bootstrap output",
        ));
    }
    let issued = issue_profile_token(&state, auth.org_id, &profile, false).await?;
    let name = profile_workload_name(&profile.harness_kind, profile.id);
    let image_tag = if profile.image_tag.trim().is_empty() {
        crate::images::image_tag()
    } else {
        profile.image_tag.clone()
    };
    let image = format!("{}:{}", profile.image, image_tag);
    let has_auth = profile.auth_secret_name.is_some();
    let resources: orbit_domain::WorkerResources =
        serde_json::from_value::<orbit_domain::WorkerResources>(profile.resources.clone())
            .unwrap_or_default()
            .with_defaults();
    let cpu_request = resources.requests.cpu.clone().unwrap_or_default();
    let memory_request = resources.requests.memory.clone().unwrap_or_default();
    let cpu_limit = resources.limits.cpu.clone().unwrap_or_default();
    let memory_limit = resources.limits.memory.clone().unwrap_or_default();
    let docker_cpu = resources
        .limits
        .cpu
        .as_deref()
        .and_then(orbit_domain::parse_cpu_millis)
        .map(|millis| format!("{}", millis as f64 / 1000.0));
    let docker_memory = resources
        .limits
        .memory
        .as_deref()
        .and_then(orbit_domain::parse_memory_bytes)
        .map(|bytes| bytes.to_string());

    let mut docker_lines = vec![
        format!("docker run --rm --name {name}"),
        format!("  -e ORBIT_WORKER_PROFILE_ID={}", profile.id),
        format!("  -e ORBIT_WORKER_TOKEN={}", issued.token),
        format!("  -e ORBIT_WORKER_HARNESSES={}", profile.harness_kind),
        format!("  -e ORBIT_WORKER_TARGET={}", profile.target),
        format!("  -e ORBIT_EXECUTION_DRIVER={}", profile.execution_driver),
        "  -e ORBIT_API_URL=http://host.docker.internal:8080".into(),
    ];
    if let Some(cpu) = &docker_cpu {
        docker_lines.push(format!("  --cpus {cpu}"));
    }
    if let Some(memory) = &docker_memory {
        docker_lines.push(format!("  --memory {memory}"));
    }
    let mut compose_volumes = String::new();
    let mut compose_command = String::new();
    match profile.harness_kind.as_str() {
        "opencode" if has_auth => {
            docker_lines.push("  -v ./auth-export.json:/seed/auth-export.json:ro".into());
            docker_lines.push("  --entrypoint /bin/sh".into());
            docker_lines.push(format!("  {image}"));
            docker_lines.push(
                "  -c 'mkdir -p /home/orbit/.local/share/opencode; opencode auth import --standalone /seed/auth-export.json; exec /usr/local/bin/orbit-worker'"
                    .into(),
            );
            compose_volumes =
                "    volumes:\n      - ./auth-export.json:/seed/auth-export.json:ro\n".into();
            compose_command = "    entrypoint: [\"/bin/sh\", \"-c\", \"mkdir -p /home/orbit/.local/share/opencode; opencode auth import --standalone /seed/auth-export.json; exec /usr/local/bin/orbit-worker\"]\n".into();
        }
        "pi" if has_auth => {
            docker_lines.push("  -v ./pi-agent:/home/orbit/.pi/agent".into());
            docker_lines.push(format!("  {image}"));
            compose_volumes = "    volumes:\n      - ./pi-agent:/home/orbit/.pi/agent\n".into();
        }
        _ => docker_lines.push(format!("  {image}")),
    }
    let docker = docker_lines.join(" \\\n");

    let compose = format!(
        "services:\n  {name}:\n    image: {image}\n    restart: unless-stopped\n    cpus: {cpu_limit}\n    mem_limit: {memory_limit}\n    extra_hosts:\n      - \"host.docker.internal:host-gateway\"\n    environment:\n      ORBIT_WORKER_PROFILE_ID: {id}\n      ORBIT_WORKER_TOKEN: {token}\n      ORBIT_WORKER_HARNESSES: {harness}\n      ORBIT_WORKER_TARGET: {target}\n      ORBIT_EXECUTION_DRIVER: {driver}\n      ORBIT_API_URL: http://host.docker.internal:8080\n{volumes}{command}",
        name = name,
        image = image,
        id = profile.id,
        token = issued.token,
        harness = profile.harness_kind,
        target = profile.target,
        driver = profile.execution_driver,
        cpu_limit = cpu_limit,
        memory_limit = memory_limit,
        volumes = compose_volumes,
        command = compose_command
    );

    let kubernetes = format!(
        r#"apiVersion: apps/v1
kind: Deployment
metadata:
  name: {name}
  namespace: orbit
spec:
  replicas: 1
  selector:
    matchLabels:
      app.kubernetes.io/name: {name}
  template:
    metadata:
      labels:
        app.kubernetes.io/name: {name}
    spec:
      securityContext:
        runAsNonRoot: true
        runAsUser: 10001
        runAsGroup: 10001
        fsGroup: 10001
      containers:
        - name: worker
          image: {image}
          env:
            - name: ORBIT_WORKER_PROFILE_ID
              value: "{id}"
            - name: ORBIT_WORKER_TOKEN
              value: "{token}"
            - name: ORBIT_WORKER_HARNESSES
              value: "{harness}"
            - name: ORBIT_WORKER_TARGET
              value: "{target}"
            - name: ORBIT_EXECUTION_DRIVER
              value: "{driver}"
            - name: ORBIT_API_URL
              value: "http://orbit-api:8080"
          resources:
            requests: {{ cpu: {cpu_request}, memory: {memory_request} }}
            limits: {{ cpu: "{cpu_limit}", memory: {memory_limit} }}
          # Mount the harness auth Secret at the adapter's expected path:
          #   OpenCode: /seed/auth-export.json (init import)
          #   Pi:       /home/orbit/.pi/agent
"#,
        name = name,
        image = image,
        id = profile.id,
        token = issued.token,
        harness = profile.harness_kind,
        target = profile.target,
        driver = profile.execution_driver,
        cpu_request = cpu_request,
        memory_request = memory_request,
        cpu_limit = cpu_limit,
        memory_limit = memory_limit
    );

    let mut notes = vec![
        "The worker loads its agent allowlist and skills from the profile at startup.".into(),
        "The worker receives only ORBIT_API_URL and a scoped, revocable ORBIT_WORKER_TOKEN; no database, NATS or master key.".into(),
        "Each generation issues a new worker token for this profile and leaves existing workers running; revoke tokens you no longer use.".into(),
        "Credentials are never included here; mount the harness auth Secret/volume.".into(),
        "Pin the image tag to a release and use the generated manifest as a starting point.".into(),
    ];
    if has_auth {
        notes.push(format!(
            "Profile references auth secret '{}'.",
            profile.auth_secret_name.as_deref().unwrap_or_default()
        ));
    }
    Ok(Json(WorkerBootstrap {
        docker,
        compose,
        kubernetes,
        notes,
    }))
}

trait HarnessKindExt {
    fn from_str_compat(value: &str) -> Result<HarnessKind, ApiError>;
}
impl HarnessKindExt for HarnessKind {
    fn from_str_compat(value: &str) -> Result<HarnessKind, ApiError> {
        value
            .parse()
            .map_err(|_| ApiError::bad_request(format!("unknown harness kind '{value}'")))
    }
}

/// Deployment/service name for a profile bootstrap: `orbit-worker-<harness>-<profile id>`.
/// Uses the full UUID (no hyphens) because the first 8 hex chars of a UUIDv7 are shared by
/// profiles created within ~65 s (issue #106). The harness part is sanitised and truncated
/// so the whole name stays a DNS-1123 label of at most 63 chars.
pub(crate) fn profile_workload_name(harness_kind: &str, profile_id: uuid::Uuid) -> String {
    let harness: String = harness_kind
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .take(16)
        .collect();
    let harness = harness.trim_matches('-');
    format!("orbit-worker-{harness}-{}", profile_id.simple())
}

#[cfg(test)]
mod workload_name_tests {
    use super::profile_workload_name;

    #[test]
    fn same_harness_profiles_get_distinct_names() {
        let a = uuid::Uuid::now_v7();
        let b = uuid::Uuid::now_v7();
        assert_ne!(
            profile_workload_name("opencode", a),
            profile_workload_name("opencode", b)
        );
    }

    #[test]
    fn names_are_dns_safe_for_longest_harness() {
        let long = "x".repeat(80);
        for kind in [
            "claude_code",
            "opencode",
            "pi",
            "generic",
            "mock",
            long.as_str(),
        ] {
            let name = profile_workload_name(kind, uuid::Uuid::now_v7());
            assert!(name.len() <= 63, "{name}");
            assert!(name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'));
            assert!(name.starts_with("orbit-worker-"));
        }
    }
}
