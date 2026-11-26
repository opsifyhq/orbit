use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use chrono::{Duration, Utc};
use uuid::Uuid;

use orbit_domain::UserRole;
use orbit_security::{issue_api_token, AuthContext, Permission};
use orbit_store::{ApiTokenView, Organization, User};

use crate::audit;
use crate::dto::{AuditListQuery, CreateApiTokenRequest, CreatedApiToken, MeResponse};
use crate::error::ApiError;
use crate::handlers::limit;
use crate::paging::{Page, PageQuery};
use crate::state::AppState;

#[utoipa::path(get, path = "/api/v1/me", tag = "account", responses((status = 200, body = MeResponse)))]
pub async fn me(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> Result<Json<MeResponse>, ApiError> {
    let user = state.store.get_user(auth.org_id, auth.user_id).await?;
    let organization = state.store.get_organization(auth.org_id).await?;
    Ok(Json(MeResponse {
        permissions: auth.permissions(),
        auth_method: format!("{:?}", auth.method).to_lowercase(),
        scopes: auth.scopes.clone(),
        user,
        organization,
    }))
}

#[utoipa::path(get, path = "/api/v1/organizations/current", tag = "account", responses((status = 200, body = Organization)))]
pub async fn current_organization(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> Result<Json<Organization>, ApiError> {
    auth.require(Permission::MembersRead)?;
    Ok(Json(state.store.get_organization(auth.org_id).await?))
}

#[utoipa::path(get, path = "/api/v1/users", tag = "account", params(PageQuery), responses((status = 200, body = Page<User>)))]
pub async fn list_users(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<PageQuery>,
) -> Result<Json<Page<User>>, ApiError> {
    auth.require(Permission::MembersRead)?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    let mut users = state.store.list_users(auth.org_id).await?;
    if !access.is_privileged() {
        // Members can see who else is in the organisation (pickers, owners), but another
        // user's identity-provider subject is only for owners and admins.
        for user in users.iter_mut().filter(|user| user.id != auth.user_id) {
            user.oidc_issuer = None;
            user.oidc_subject = None;
        }
    }
    Ok(Json(Page::from_vec(users, &query, 100)?))
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateUserRoleRequest {
    pub role: String,
}

/// Permission set a role name resolves to: a built-in role or an organisation custom role.
pub(crate) async fn role_permission_set(
    state: &AppState,
    org_id: Uuid,
    name: &str,
) -> Result<Vec<String>, ApiError> {
    if let Ok(role) = name.parse::<UserRole>() {
        return Ok(Permission::all_for(role)
            .into_iter()
            .map(str::to_string)
            .collect());
    }
    Ok(state
        .store
        .custom_role_permissions(org_id, name)
        .await?
        .unwrap_or_default())
}

#[utoipa::path(patch, path = "/api/v1/users/{user_id}", tag = "account", request_body = UpdateUserRoleRequest, responses((status = 200, body = User)))]
pub async fn update_user_role(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(user_id): Path<Uuid>,
    crate::extract::ApiJson(body): crate::extract::ApiJson<UpdateUserRoleRequest>,
) -> Result<Json<User>, ApiError> {
    auth.require(Permission::SettingsWrite)?;
    let role_name = body.role.trim();
    let builtin = role_name.parse::<UserRole>().is_ok();
    let custom = !builtin
        && state
            .store
            .custom_role_permissions(auth.org_id, role_name)
            .await?
            .is_some();
    if !builtin && !custom {
        return Err(ApiError::bad_request("invalid role"));
    }
    if custom {
        state.require_feature(orbit_security::license::FEATURE_CUSTOM_ROLES)?;
    }
    if user_id == auth.user_id {
        return Err(ApiError::bad_request("you cannot change your own role"));
    }
    // Grant ceiling: only an Owner assigns or changes the Owner role, and nobody hands out
    // (or takes away) a role that is more powerful than what they hold themselves.
    let is_owner = auth.role == UserRole::Owner;
    let target = state.store.get_user(auth.org_id, user_id).await?;
    if !is_owner && (role_name == "owner" || target.role == "owner") {
        return Err(ApiError::forbidden(
            "only an owner can assign or change the owner role",
        ));
    }
    let held = auth.permissions();
    for name in [role_name, target.role.as_str()] {
        let permissions = role_permission_set(&state, auth.org_id, name).await?;
        let beyond = orbit_security::ungranted_permissions(&held, &permissions);
        if !beyond.is_empty() {
            return Err(ApiError::forbidden(format!(
                "role '{name}' includes permissions you do not hold: {}",
                beyond.join(", ")
            )));
        }
    }
    let user = state
        .store
        .set_user_role_name(auth.org_id, user_id, role_name)
        .await?;
    audit::record(
        &state,
        &auth,
        "user.role_changed",
        Some("user"),
        Some(user_id),
        serde_json::json!({ "role": body.role }),
    )
    .await;
    Ok(Json(user))
}

#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct ApiTokenListQuery {
    /// Include revoked tokens. Off by default so a long-lived installation's list stays
    /// useful; they remain queryable when an audit needs them.
    pub include_revoked: Option<bool>,
    pub limit: Option<i64>,
    pub cursor: Option<String>,
}

impl ApiTokenListQuery {
    fn page(&self) -> PageQuery {
        PageQuery {
            limit: self.limit,
            cursor: self.cursor.clone(),
        }
    }
}

#[utoipa::path(get, path = "/api/v1/api-tokens", tag = "account", params(ApiTokenListQuery), responses((status = 200, body = Page<ApiTokenView>)))]
pub async fn list_api_tokens(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<ApiTokenListQuery>,
) -> Result<Json<Page<ApiTokenView>>, ApiError> {
    auth.require(Permission::SettingsRead)?;
    // Owners and admins see every token; everyone else only their own.
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    let only_user = (!access.is_privileged()).then_some(auth.user_id);
    let tokens = state.store.list_api_tokens(auth.org_id, only_user).await?;
    let tokens = tokens
        .into_iter()
        .filter(|token| query.include_revoked.unwrap_or(false) || token.revoked_at.is_none())
        .collect();
    Ok(Json(Page::from_vec(tokens, &query.page(), 100)?))
}

/// Longest API token lifetime, in days (ten years).
const MAX_TOKEN_DAYS: i64 = 3650;

#[utoipa::path(post, path = "/api/v1/api-tokens", tag = "account", request_body = CreateApiTokenRequest, responses((status = 201, body = CreatedApiToken)))]
pub async fn create_api_token(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    crate::extract::ApiJson(body): crate::extract::ApiJson<CreateApiTokenRequest>,
) -> Result<(StatusCode, Json<CreatedApiToken>), ApiError> {
    auth.require(Permission::SettingsWrite)?;
    if body.name.trim().is_empty() {
        return Err(ApiError::validation(vec![
            orbit_domain::ValidationIssue::new("name", "token name is required"),
        ]));
    }
    if let Some(issue) = orbit_domain::name_length_issue("name", &body.name) {
        return Err(ApiError::validation(vec![issue]));
    }
    let scopes = if body.scopes.is_empty() {
        auth.permissions()
    } else {
        body.scopes.clone()
    };
    let valid: std::collections::BTreeSet<&str> =
        Permission::all_for(UserRole::Owner).into_iter().collect();
    for scope in &scopes {
        if scope != "*" && !valid.contains(scope.as_str()) {
            return Err(ApiError::validation(vec![
                orbit_domain::ValidationIssue::new("scopes", format!("unknown scope '{scope}'")),
            ]));
        }
    }
    // `*` means "everything my role grants" and is bounded at authentication time, so it
    // is always allowed; explicit scopes must be held by the caller right now.
    let held = auth.permissions();
    let denied: Vec<orbit_domain::ValidationIssue> = scopes
        .iter()
        .filter(|scope| scope.as_str() != "*" && !orbit_security::scope_granted(&held, scope))
        .map(|scope| {
            orbit_domain::ValidationIssue::new(
                "scopes",
                format!("scope '{scope}' exceeds your current permissions"),
            )
        })
        .collect();
    if !denied.is_empty() {
        return Err(ApiError::validation(denied));
    }
    // A zero, negative or oversized lifetime used to create a token that never expires
    // (or a silently shortened one); refuse it instead (#438).
    if body
        .expires_in_days
        .is_some_and(|days| !(1..=MAX_TOKEN_DAYS).contains(&days))
    {
        return Err(ApiError::validation(vec![
            orbit_domain::ValidationIssue::new(
                "expiresInDays",
                format!(
                    "must be 1 to {MAX_TOKEN_DAYS}; leave it out for a token that does not expire"
                ),
            ),
        ]));
    }
    let expires_at = body
        .expires_in_days
        .map(|days| Utc::now() + Duration::days(days));
    let issued = issue_api_token();
    let view = state
        .store
        .create_api_token(
            auth.org_id,
            auth.user_id,
            body.name.trim(),
            &issued.prefix,
            &issued.hash,
            &scopes,
            expires_at,
        )
        .await?;
    audit::record(
        &state,
        &auth,
        "api_token.created",
        Some("api_token"),
        Some(view.id),
        serde_json::json!({ "name": view.name, "scopes": scopes }),
    )
    .await;
    Ok((
        StatusCode::CREATED,
        Json(CreatedApiToken {
            id: view.id,
            name: view.name,
            token: issued.token,
            token_prefix: view.token_prefix,
            scopes: view.scopes,
            expires_at: view.expires_at,
        }),
    ))
}

#[utoipa::path(delete, path = "/api/v1/api-tokens/{token_id}", tag = "account", responses((status = 204)))]
pub async fn revoke_api_token(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(token_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::SettingsWrite)?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    let only_user = (!access.is_privileged()).then_some(auth.user_id);
    state
        .store
        .revoke_api_token(auth.org_id, token_id, only_user)
        .await?;
    audit::record(
        &state,
        &auth,
        "api_token.revoked",
        Some("api_token"),
        Some(token_id),
        serde_json::json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuditLogPage {
    pub items: Vec<orbit_store::AuditLogRecord>,
    pub next_cursor: Option<String>,
    pub total: i64,
}

#[utoipa::path(get, path = "/api/v1/audit-logs", tag = "account", params(AuditListQuery), responses((status = 200, body = AuditLogPage)))]
pub async fn list_audit_logs(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<AuditListQuery>,
) -> Result<Json<AuditLogPage>, ApiError> {
    auth.require(Permission::AuditRead)?;
    let area = query.area.as_deref().filter(|area| !area.is_empty());
    if let Some(area) = area {
        if area.len() > 64
            || !area
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        {
            return Err(ApiError::bad_request(
                "area must be an action family such as `agent` or `team_run`",
            ));
        }
    }
    let action = query.action.as_deref().filter(|action| !action.is_empty());
    let (items, next) = state
        .store
        .list_audit_log_page(
            auth.org_id,
            limit(query.limit)?,
            query.before_id,
            action,
            area,
        )
        .await?;
    let total = state
        .store
        .count_audit_logs(auth.org_id, action, area)
        .await?;
    Ok(Json(AuditLogPage {
        items,
        next_cursor: next.map(|id| id.to_string()),
        total,
    }))
}
