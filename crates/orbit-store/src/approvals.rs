use uuid::Uuid;

use orbit_domain::{new_id, ExecutionStatus};

use crate::models::{Approval, ApprovalWithExecution};
use crate::{Store, StoreError, StoreResult};

impl Store {
    pub async fn create_approval(
        &self,
        org_id: Uuid,
        execution_id: Uuid,
        requested_action: serde_json::Value,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> StoreResult<Approval> {
        sqlx::query_as::<_, Approval>(
            r#"
            INSERT INTO approvals (id, org_id, execution_id, status, requested_action, expires_at)
            VALUES ($1, $2, $3, 'pending', $4, $5)
            RETURNING *
            "#,
        )
        .bind(new_id())
        .bind(org_id)
        .bind(execution_id)
        .bind(requested_action)
        .bind(expires_at)
        .fetch_one(&self.pool)
        .await
        .map_err(Into::into)
    }

    pub async fn get_approval(&self, org_id: Uuid, approval_id: Uuid) -> StoreResult<Approval> {
        sqlx::query_as::<_, Approval>("SELECT * FROM approvals WHERE org_id = $1 AND id = $2")
            .bind(org_id)
            .bind(approval_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| StoreError::NotFound("approval".into()))
    }

    /// Approvals for the current org, newest first. `status` is one stored status, or
    /// `decided` for every status but `pending` (newest decision first).
    pub async fn list_approvals(
        &self,
        org_id: Uuid,
        status: Option<&str>,
        execution_id: Option<Uuid>,
        limit: i64,
        access: Option<&crate::access::ResourceAccess>,
    ) -> StoreResult<Vec<ApprovalWithExecution>> {
        let (privileged, user_id, team_ids, isolation) = crate::access::binds(access);
        Ok(sqlx::query_as::<_, ApprovalWithExecution>(
            r#"
            SELECT ap.*, e.agent_name, e.status AS execution_status, e.agent_id
            FROM approvals ap
            JOIN executions e ON e.id = ap.execution_id
            JOIN agents a ON a.id = e.agent_id
            WHERE ap.org_id = $1
              AND ($2::text IS NULL OR ap.status = $2
                   OR ($2 = 'decided' AND ap.status <> 'pending'))
              AND ($8::uuid IS NULL OR ap.execution_id = $8)
              AND ($4::bool OR a.owner_user_id = $5
                   OR CASE WHEN $7::bool AND a.owner_team_id IS NOT NULL
                        THEN a.owner_team_id = ANY($6)
                        ELSE a.visibility = 'org'
                             OR (a.visibility = 'team' AND a.owner_team_id = ANY($6)) END)
            ORDER BY CASE WHEN $2 = 'decided' THEN ap.decided_at END DESC NULLS LAST,
                     ap.created_at DESC, ap.id DESC
            LIMIT $3
            "#,
        )
        .bind(org_id)
        .bind(status)
        .bind(limit)
        .bind(privileged)
        .bind(user_id)
        .bind(&team_ids)
        .bind(isolation)
        .bind(execution_id)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Decide a pending approval. Only `approved`/`rejected` are accepted.
    pub async fn decide_approval(
        &self,
        org_id: Uuid,
        approval_id: Uuid,
        decision: &str,
        decided_by: Uuid,
        comment: Option<&str>,
    ) -> StoreResult<Approval> {
        if !matches!(decision, "approved" | "rejected") {
            return Err(StoreError::Invalid(
                "decision must be approved or rejected".into(),
            ));
        }
        // A pending approval of a run that already ended is dead: expire it so the UI stops
        // showing it, then refuse the decision. `finish_execution` and cancel normally do
        // this in their own transaction; this covers rows written by older versions.
        let terminal: Vec<String> = ExecutionStatus::ALL
            .into_iter()
            .filter(|status| status.is_terminal())
            .map(|status| status.as_str().to_string())
            .collect();
        let ended = sqlx::query(
            r#"
            UPDATE approvals
            SET status = 'expired', decided_at = now()
            WHERE org_id = $1 AND id = $2 AND status = 'pending'
              AND EXISTS (
                  SELECT 1 FROM executions e
                  WHERE e.id = approvals.execution_id AND e.status = ANY($3)
              )
            "#,
        )
        .bind(org_id)
        .bind(approval_id)
        .bind(&terminal)
        .execute(&self.pool)
        .await?
        .rows_affected();
        if ended > 0 {
            return Err(StoreError::Conflict("execution is terminal".into()));
        }
        // An approval past its deadline can no longer be decided: mark it expired first
        // so the UI does not keep showing a dead request, then try the decision.
        let expired = sqlx::query(
            r#"
            UPDATE approvals
            SET status = 'expired', decided_at = now()
            WHERE org_id = $1 AND id = $2 AND status = 'pending'
              AND expires_at IS NOT NULL AND expires_at <= now()
            "#,
        )
        .bind(org_id)
        .bind(approval_id)
        .execute(&self.pool)
        .await?
        .rows_affected();
        if expired > 0 {
            return Err(StoreError::Conflict("approval expired".into()));
        }
        let active: Vec<String> = ExecutionStatus::ALL
            .into_iter()
            .filter(|status| !status.is_terminal())
            .map(|status| status.as_str().to_string())
            .collect();
        sqlx::query_as::<_, Approval>(
            r#"
            UPDATE approvals ap
            SET status = $3, decided_by = $4, decided_at = now(), comment = $5
            WHERE ap.org_id = $1 AND ap.id = $2 AND ap.status = 'pending'
              AND (ap.expires_at IS NULL OR ap.expires_at > now())
              AND EXISTS (
                  SELECT 1 FROM executions e
                  WHERE e.id = ap.execution_id AND e.status = ANY($6)
              )
            RETURNING ap.*
            "#,
        )
        .bind(org_id)
        .bind(approval_id)
        .bind(decision)
        .bind(decided_by)
        .bind(comment)
        .bind(&active)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| {
            StoreError::Conflict("approval is not pending (already decided or expired)".into())
        })
    }

    /// Number of still-pending approvals for an execution (caps broker requests).
    pub async fn count_pending_approvals(
        &self,
        org_id: Uuid,
        execution_id: Uuid,
    ) -> StoreResult<i64> {
        Ok(sqlx::query_scalar(
            "SELECT count(*) FROM approvals WHERE org_id = $1 AND execution_id = $2 AND status = 'pending'",
        )
        .bind(org_id)
        .bind(execution_id)
        .fetch_one(&self.pool)
        .await?)
    }

    /// Broker helper: wait until an approval is decided or the deadline passes.
    pub async fn await_approval(
        &self,
        org_id: Uuid,
        approval_id: Uuid,
        timeout: std::time::Duration,
    ) -> StoreResult<Approval> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let approval = self.get_approval(org_id, approval_id).await?;
            if approval.status != "pending" {
                return Ok(approval);
            }
            if tokio::time::Instant::now() >= deadline {
                // Expire it so the UI does not show a dead request forever.
                let expired = sqlx::query_as::<_, Approval>(
                    r#"
                    UPDATE approvals
                    SET status = 'expired', decided_at = now()
                    WHERE org_id = $1 AND id = $2 AND status = 'pending'
                    RETURNING *
                    "#,
                )
                .bind(org_id)
                .bind(approval_id)
                .fetch_optional(&self.pool)
                .await?;
                return Ok(expired.unwrap_or(approval));
            }
            tokio::time::sleep(std::time::Duration::from_millis(750)).await;
        }
    }
}
