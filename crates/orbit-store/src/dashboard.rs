use serde_json::json;
use uuid::Uuid;

use crate::access::{binds, ResourceAccess};
use crate::models::{DashboardSummary, ExecutionRecord};
use crate::{Store, StoreResult};

/// Standard visibility predicate (see `access::binds`) for rows aliased `alias`.
/// Expects `$2..$5` = privileged, user id, team ids, team isolation.
fn visible(alias: &str) -> String {
    format!(
        "($2::bool OR {a}.owner_user_id = $3 \
           OR CASE WHEN $5::bool AND {a}.owner_team_id IS NOT NULL \
                THEN {a}.owner_team_id = ANY($4) \
                ELSE {a}.visibility = 'org' \
                     OR ({a}.visibility = 'team' AND {a}.owner_team_id = ANY($4)) END)",
        a = alias
    )
}

impl Store {
    /// Dashboard counts and recent executions, restricted to what `access` can see
    /// (`None` = platform/internal access, org-wide).
    pub async fn dashboard_summary(
        &self,
        org_id: Uuid,
        access: Option<&ResourceAccess>,
    ) -> StoreResult<DashboardSummary> {
        let (privileged, user_id, team_ids, isolation) = binds(access);
        let agent_ok = visible("a");

        let (total_agents, active_agents, paused_agents, draft_agents): (i64, i64, i64, i64) =
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                r#"
                SELECT
                    COUNT(*) FILTER (WHERE a.status <> 'archived'),
                    COUNT(*) FILTER (WHERE a.status = 'active'),
                    COUNT(*) FILTER (WHERE a.status = 'paused'),
                    COUNT(*) FILTER (WHERE a.status = 'draft')
                FROM agents a WHERE a.org_id = $1 AND {agent_ok}
                "#
            )))
            .bind(org_id)
            .bind(privileged)
            .bind(user_id)
            .bind(&team_ids)
            .bind(isolation)
            .fetch_one(&self.pool)
            .await?;

        let (executions_24h, failed_executions_24h, running_executions): (i64, i64, i64) =
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                r#"
                SELECT
                    COUNT(*) FILTER (WHERE e.created_at > now() - interval '24 hours'),
                    COUNT(*) FILTER (WHERE e.created_at > now() - interval '24 hours'
                                      AND e.status IN ('failed', 'timed_out')),
                    COUNT(*) FILTER (WHERE e.status IN ('queued', 'dispatched', 'running'))
                FROM executions e JOIN agents a ON a.id = e.agent_id
                WHERE e.org_id = $1 AND {agent_ok}
                  AND (e.created_at > now() - interval '24 hours'
                       OR e.status IN ('queued', 'dispatched', 'running'))
                "#
            )))
            .bind(org_id)
            .bind(privileged)
            .bind(user_id)
            .bind(&team_ids)
            .bind(isolation)
            .fetch_one(&self.pool)
            .await?;

        let connection_ok = visible("c");
        let total_connections: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT COUNT(*) FROM connections c WHERE c.org_id = $1 AND c.status = 'active' AND {connection_ok}"
        )))
        .bind(org_id)
        .bind(privileged)
        .bind(user_id)
        .bind(&team_ids)
        .bind(isolation)
        .fetch_one(&self.pool)
        .await?;

        let trigger_ok = visible("t");
        let enabled_triggers: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT COUNT(*) FROM triggers t WHERE t.org_id = $1 AND t.enabled AND {trigger_ok}"
        )))
        .bind(org_id)
        .bind(privileged)
        .bind(user_id)
        .bind(&team_ids)
        .bind(isolation)
        .fetch_one(&self.pool)
        .await?;

        let pending_approvals: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            r#"
            SELECT COUNT(*) FROM approvals p
            JOIN executions e ON e.id = p.execution_id
            JOIN agents a ON a.id = e.agent_id
            WHERE p.org_id = $1 AND p.status = 'pending' AND {agent_ok}
            "#
        )))
        .bind(org_id)
        .bind(privileged)
        .bind(user_id)
        .bind(&team_ids)
        .bind(isolation)
        .fetch_one(&self.pool)
        .await?;

        let recent_executions = sqlx::query_as::<_, ExecutionRecord>(sqlx::AssertSqlSafe(format!(
            r#"
            SELECT e.* FROM executions e JOIN agents a ON a.id = e.agent_id
            WHERE e.org_id = $1 AND {agent_ok}
            ORDER BY e.created_at DESC LIMIT 8
            "#
        )))
        .bind(org_id)
        .bind(privileged)
        .bind(user_id)
        .bind(&team_ids)
        .bind(isolation)
        .fetch_all(&self.pool)
        .await?;

        Ok(DashboardSummary {
            total_agents,
            active_agents,
            paused_agents,
            draft_agents,
            executions_24h,
            failed_executions_24h,
            running_executions,
            total_connections,
            enabled_triggers,
            recent_executions,
            pending_approvals,
        })
    }

    /// Lightweight usage rollup used by the dashboard and future quota enforcement.
    pub async fn usage_rollup(
        &self,
        org_id: Uuid,
        days: i64,
        access: Option<&ResourceAccess>,
    ) -> StoreResult<serde_json::Value> {
        let (privileged, user_id, team_ids, isolation) = binds(access);
        let agent_ok = visible("a");
        let rows: Vec<(String, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            r#"
            SELECT to_char(date_trunc('day', e.created_at), 'YYYY-MM-DD') AS day, COUNT(*)
            FROM executions e JOIN agents a ON a.id = e.agent_id
            WHERE e.org_id = $1 AND e.created_at > now() - ($6 || ' days')::interval
              AND {agent_ok}
            GROUP BY 1 ORDER BY 1
            "#
        )))
        .bind(org_id)
        .bind(privileged)
        .bind(user_id)
        .bind(&team_ids)
        .bind(isolation)
        .bind(days.to_string())
        .fetch_all(&self.pool)
        .await?;
        Ok(json!({
            "days": rows.into_iter().map(|(day, count)| json!({ "day": day, "executions": count })).collect::<Vec<_>>()
        }))
    }
}
