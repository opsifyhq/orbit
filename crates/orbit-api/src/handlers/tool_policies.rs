//! Tool policy CRUD. Rules restrict native/brokered tool calls per organisation and per
//! agent; the worker receives the effective set in the execution context.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use orbit_domain::{validate_tool_policy, ToolPolicyAction, ToolPolicyMatcher, ToolPolicyRule};
use orbit_security::{AuthContext, Permission};
use orbit_store::ToolPolicy;

use crate::audit;
use crate::error::ApiError;
use crate::paging::{Page, PageQuery};
use crate::patch::{merge_patch, parse_patch};
use crate::state::AppState;

#[derive(Debug, Clone, Deserialize, utoipa::IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct ListToolPoliciesQuery {
    #[serde(default)]
    pub agent_id: Option<Uuid>,
    pub limit: Option<i64>,
    pub cursor: Option<String>,
}

impl ListToolPoliciesQuery {
    fn page(&self) -> PageQuery {
        PageQuery {
            limit: self.limit,
            cursor: self.cursor.clone(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolPolicyRequest {
    pub name: String,
    #[serde(default)]
    pub agent_id: Option<Uuid>,
    #[serde(default = "default_priority")]
    pub priority: i32,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    pub action: String,
    #[serde(default)]
    pub matcher: ToolPolicyMatcher,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub impact: Option<String>,
}

fn default_priority() -> i32 {
    100
}

fn default_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolPolicyView {
    pub id: Uuid,
    pub org_id: Uuid,
    pub agent_id: Option<Uuid>,
    pub name: String,
    pub priority: i32,
    pub enabled: bool,
    pub action: String,
    pub matcher: ToolPolicyMatcher,
    pub reason: Option<String>,
    pub impact: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl TryFrom<ToolPolicy> for ToolPolicyView {
    type Error = ApiError;

    fn try_from(value: ToolPolicy) -> Result<Self, Self::Error> {
        let matcher: ToolPolicyMatcher =
            serde_json::from_value(value.matcher).map_err(|error| {
                ApiError::internal(format!("stored tool policy matcher is invalid: {error}"))
            })?;
        Ok(Self {
            id: value.id,
            org_id: value.org_id,
            agent_id: value.agent_id,
            name: value.name,
            priority: value.priority,
            enabled: value.enabled,
            action: value.action,
            matcher,
            reason: value.reason,
            impact: value.impact,
            created_at: value.created_at,
            updated_at: value.updated_at,
        })
    }
}

fn validated(body: &ToolPolicyRequest) -> Result<ToolPolicyRule, ApiError> {
    let action = ToolPolicyAction::parse(body.action.trim())
        .ok_or_else(|| ApiError::bad_request("action must be allow, deny or require_approval"))?;
    let rule = ToolPolicyRule {
        id: Uuid::nil(),
        name: body.name.trim().to_string(),
        priority: body.priority,
        enabled: body.enabled,
        action,
        matcher: body.matcher.clone(),
        reason: body.reason.clone(),
        impact: body.impact.clone(),
    };
    let mut issues = validate_tool_policy(&rule);
    if body.priority < -10_000 || body.priority > 10_000 {
        issues.push("priority must be between -10000 and 10000".into());
    }
    if !issues.is_empty() {
        return Err(ApiError::validation(
            issues
                .into_iter()
                .map(|message| orbit_domain::ValidationIssue::new("toolPolicy", message))
                .collect(),
        ));
    }
    Ok(rule)
}

/// A policy attached to an agent changes what that agent may do, so the caller must be
/// able to manage the agent. Org-wide policies (no agent) only need `settings:write`.
async fn require_agent_manage(
    state: &AppState,
    auth: &AuthContext,
    access: &crate::access::AccessScope,
    agent_id: Option<Uuid>,
) -> Result<(), ApiError> {
    if let Some(agent_id) = agent_id {
        let agent = state.store.get_agent(auth.org_id, agent_id).await?;
        access.require_manage(
            "agent",
            agent.owner_user_id,
            agent.owner_team_id,
            &agent.visibility,
        )?;
    }
    Ok(())
}

#[utoipa::path(get, path = "/api/v1/tool-policies", tag = "tool-policies", params(("agentId" = Option<Uuid>, Query, description = "Include agent rules and org defaults")), responses((status = 200, body = Page<ToolPolicyView>)))]
pub async fn list(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<ListToolPoliciesQuery>,
) -> Result<Json<Page<ToolPolicyView>>, ApiError> {
    auth.require(Permission::SettingsRead)?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    let rows = state
        .store
        .list_tool_policies(auth.org_id, query.agent_id, Some(&access.store_access()))
        .await?;
    let mut views = Vec::with_capacity(rows.len());
    for row in rows {
        views.push(ToolPolicyView::try_from(row)?);
    }
    Ok(Json(Page::from_vec(views, &query.page(), 100)?))
}

#[utoipa::path(post, path = "/api/v1/tool-policies", tag = "tool-policies", request_body = ToolPolicyRequest, responses((status = 201, body = ToolPolicyView)))]
pub async fn create(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    crate::extract::ApiJson(body): crate::extract::ApiJson<ToolPolicyRequest>,
) -> Result<(StatusCode, Json<ToolPolicyView>), ApiError> {
    auth.require(Permission::SettingsWrite)?;
    let rule = validated(&body)?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    require_agent_manage(&state, &auth, &access, body.agent_id).await?;
    let matcher = serde_json::to_value(&rule.matcher)
        .map_err(|error| ApiError::internal(error.to_string()))?;
    let record = state
        .store
        .create_tool_policy(
            auth.org_id,
            body.agent_id,
            &rule.name,
            rule.priority,
            rule.enabled,
            rule.action.as_str(),
            &matcher,
            rule.reason.as_deref(),
            rule.impact.as_deref(),
        )
        .await?;
    state
        .store
        .assign_resource_owner(auth.org_id, "tool_policies", record.id, auth.user_id)
        .await?;
    audit::record(
        &state,
        &auth,
        "tool_policy.created",
        Some("tool_policy"),
        Some(record.id),
        serde_json::json!({
            "name": record.name,
            "agentId": record.agent_id,
            "action": record.action,
        }),
    )
    .await;
    Ok((StatusCode::CREATED, Json(ToolPolicyView::try_from(record)?)))
}

#[utoipa::path(patch, path = "/api/v1/tool-policies/{policy_id}", tag = "tool-policies", request_body = ToolPolicyRequest, responses((status = 200, body = ToolPolicyView)))]
pub async fn update(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(policy_id): Path<Uuid>,
    crate::extract::ApiJson(patch): crate::extract::ApiJson<serde_json::Value>,
) -> Result<Json<ToolPolicyView>, ApiError> {
    auth.require(Permission::SettingsWrite)?;
    let existing = state.store.get_tool_policy(auth.org_id, policy_id).await?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    access.require_manage(
        "tool policy",
        existing.owner_user_id,
        existing.owner_team_id,
        &existing.visibility,
    )?;
    // Both the agent the policy is moving away from and the one it targets.
    require_agent_manage(&state, &auth, &access, existing.agent_id).await?;
    let merged = merge_patch(
        serde_json::json!({
            "name": existing.name,
            "agentId": existing.agent_id,
            "priority": existing.priority,
            "enabled": existing.enabled,
            "action": existing.action,
            "matcher": existing.matcher,
            "reason": existing.reason,
            "impact": existing.impact,
        }),
        patch,
        &[],
    )?;
    let body: ToolPolicyRequest = parse_patch(merged)?;
    let rule = validated(&body)?;
    require_agent_manage(&state, &auth, &access, body.agent_id).await?;
    let matcher = serde_json::to_value(&rule.matcher)
        .map_err(|error| ApiError::internal(error.to_string()))?;
    let record = state
        .store
        .update_tool_policy(
            auth.org_id,
            policy_id,
            body.agent_id,
            &rule.name,
            rule.priority,
            rule.enabled,
            rule.action.as_str(),
            &matcher,
            rule.reason.as_deref(),
            rule.impact.as_deref(),
        )
        .await?;
    audit::record(
        &state,
        &auth,
        "tool_policy.updated",
        Some("tool_policy"),
        Some(record.id),
        serde_json::json!({
            "name": record.name,
            "agentId": record.agent_id,
            "action": record.action,
        }),
    )
    .await;
    Ok(Json(ToolPolicyView::try_from(record)?))
}

#[utoipa::path(delete, path = "/api/v1/tool-policies/{policy_id}", tag = "tool-policies", responses((status = 204)))]
pub async fn delete(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(policy_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::SettingsWrite)?;
    let existing = state.store.get_tool_policy(auth.org_id, policy_id).await?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    access.require_manage(
        "tool policy",
        existing.owner_user_id,
        existing.owner_team_id,
        &existing.visibility,
    )?;
    require_agent_manage(&state, &auth, &access, existing.agent_id).await?;
    state
        .store
        .delete_tool_policy(auth.org_id, policy_id)
        .await?;
    audit::record(
        &state,
        &auth,
        "tool_policy.deleted",
        Some("tool_policy"),
        Some(policy_id),
        serde_json::json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}
