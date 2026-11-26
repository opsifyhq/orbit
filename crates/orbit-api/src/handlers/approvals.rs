use axum::extract::{Path, Query, State};
use axum::Extension;
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use orbit_security::{AuthContext, Permission};
use orbit_store::Approval;

use crate::audit;
use crate::error::ApiError;
use crate::paging::{Page, PageQuery};
use crate::state::AppState;

#[derive(Debug, Clone, Deserialize, utoipa::IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct ApprovalListQuery {
    /// `pending`, `approved`, `rejected`, `expired`, or `decided` (every status but `pending`).
    pub status: Option<String>,
    /// Only approvals requested by this execution.
    pub execution_id: Option<Uuid>,
    pub limit: Option<i64>,
    pub cursor: Option<String>,
}

impl ApprovalListQuery {
    fn page(&self) -> PageQuery {
        PageQuery {
            limit: self.limit,
            cursor: self.cursor.clone(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalDecisionRequest {
    #[serde(default)]
    pub comment: Option<String>,
}

#[utoipa::path(get, path = "/api/v1/approvals", tag = "approvals", params(ApprovalListQuery), responses((status = 200, body = Page<orbit_store::ApprovalWithExecution>)))]
pub async fn list_approvals(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<ApprovalListQuery>,
) -> Result<Json<Page<orbit_store::ApprovalWithExecution>>, ApiError> {
    auth.require(Permission::ApprovalsDecide)?;
    let status = query.status.as_deref().filter(|s| !s.is_empty());
    if let Some(status) = status {
        if !matches!(
            status,
            "pending" | "approved" | "rejected" | "expired" | "decided"
        ) {
            return Err(ApiError::bad_request("unknown approval status"));
        }
    }
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    let store_access = access.store_access();
    let approvals = state
        .store
        .list_approvals(
            auth.org_id,
            status,
            query.execution_id,
            // A fixed window keeps the offset cursor stable across pages; the response
            // itself is bounded by the validated `limit`.
            crate::paging::MAX_PAGE_LIMIT,
            Some(&store_access),
        )
        .await?;
    Ok(Json(Page::from_window(approvals, &query.page(), 100)?))
}

#[utoipa::path(post, path = "/api/v1/approvals/{approval_id}/approve", tag = "approvals", request_body = ApprovalDecisionRequest, responses((status = 200, body = Approval)))]
pub async fn approve(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(approval_id): Path<Uuid>,
    crate::extract::ApiJson(body): crate::extract::ApiJson<ApprovalDecisionRequest>,
) -> Result<Json<Approval>, ApiError> {
    decide(state, auth, approval_id, "approved", body.comment).await
}

#[utoipa::path(post, path = "/api/v1/approvals/{approval_id}/reject", tag = "approvals", request_body = ApprovalDecisionRequest, responses((status = 200, body = Approval)))]
pub async fn reject(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(approval_id): Path<Uuid>,
    crate::extract::ApiJson(body): crate::extract::ApiJson<ApprovalDecisionRequest>,
) -> Result<Json<Approval>, ApiError> {
    decide(state, auth, approval_id, "rejected", body.comment).await
}

async fn decide(
    state: AppState,
    auth: AuthContext,
    approval_id: Uuid,
    decision: &str,
    comment: Option<String>,
) -> Result<Json<Approval>, ApiError> {
    auth.require(Permission::ApprovalsDecide)?;
    let existing = state.store.get_approval(auth.org_id, approval_id).await?;
    let execution = state
        .store
        .get_execution(auth.org_id, existing.execution_id)
        .await?;
    let agent = state
        .store
        .get_agent(auth.org_id, execution.agent_id)
        .await?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    // Deciding lets the agent run a gated action, so it needs the same access as running
    // the agent (use/manage), not just visibility. Invisible agents stay 404.
    access.require_use(
        "approval",
        agent.owner_user_id,
        agent.owner_team_id,
        &agent.visibility,
    )?;
    // An expired approval is reported as expired even when four-eyes would also refuse
    // it; the row is marked expired by the same conditional update.
    if existing
        .expires_at
        .is_some_and(|expires_at| expires_at <= chrono::Utc::now())
    {
        let _ = state
            .store
            .decide_approval(
                auth.org_id,
                approval_id,
                decision,
                auth.user_id,
                comment.as_deref(),
            )
            .await;
        return Err(ApiError::conflict("approval expired"));
    }
    // Optional four-eyes policy: whoever requested the execution cannot decide it.
    if execution.requested_by == Some(auth.user_id)
        && crate::handlers::settings::load_approvals_settings(&state.store)
            .await?
            .approvals_require_different_approver
    {
        return Err(ApiError::forbidden(
            "approvals require a different approver than the user who requested the execution",
        ));
    }
    let approval = state
        .store
        .decide_approval(
            auth.org_id,
            approval_id,
            decision,
            auth.user_id,
            comment.as_deref(),
        )
        .await?;
    audit::record(
        &state,
        &auth,
        if decision == "approved" {
            "approval.approved"
        } else {
            "approval.rejected"
        },
        Some("approval"),
        Some(approval.id),
        serde_json::json!({
            "executionId": approval.execution_id,
            "comment": comment,
        }),
    )
    .await;
    Ok(Json(approval))
}
