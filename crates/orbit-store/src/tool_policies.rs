//! Tool policy persistence.

use uuid::Uuid;

use orbit_domain::{ToolPolicyAction, ToolPolicyMatcher, ToolPolicyRule};

use crate::models::ToolPolicy;
use crate::{Store, StoreError, StoreResult};

impl Store {
    #[allow(clippy::too_many_arguments)]
    pub async fn create_tool_policy(
        &self,
        org_id: Uuid,
        agent_id: Option<Uuid>,
        name: &str,
        priority: i32,
        enabled: bool,
        action: &str,
        matcher: &serde_json::Value,
        reason: Option<&str>,
        impact: Option<&str>,
    ) -> StoreResult<ToolPolicy> {
        sqlx::query_as::<_, ToolPolicy>(
            r#"
            INSERT INTO tool_policies
                (id, org_id, agent_id, name, priority, enabled, action, matcher, reason, impact)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            RETURNING *
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(org_id)
        .bind(agent_id)
        .bind(name)
        .bind(priority)
        .bind(enabled)
        .bind(action)
        .bind(matcher)
        .bind(reason)
        .bind(impact)
        .fetch_one(&self.pool)
        .await
        .map_err(|error| {
            if error
                .as_database_error()
                .map(|db| db.is_unique_violation())
                .unwrap_or(false)
            {
                StoreError::Conflict(format!("tool policy '{name}' already exists"))
            } else {
                error.into()
            }
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_tool_policy(
        &self,
        org_id: Uuid,
        policy_id: Uuid,
        agent_id: Option<Uuid>,
        name: &str,
        priority: i32,
        enabled: bool,
        action: &str,
        matcher: &serde_json::Value,
        reason: Option<&str>,
        impact: Option<&str>,
    ) -> StoreResult<ToolPolicy> {
        sqlx::query_as::<_, ToolPolicy>(
            r#"
            UPDATE tool_policies
            SET agent_id = $3, name = $4, priority = $5, enabled = $6, action = $7,
                matcher = $8, reason = $9, impact = $10, updated_at = now()
            WHERE org_id = $1 AND id = $2
            RETURNING *
            "#,
        )
        .bind(org_id)
        .bind(policy_id)
        .bind(agent_id)
        .bind(name)
        .bind(priority)
        .bind(enabled)
        .bind(action)
        .bind(matcher)
        .bind(reason)
        .bind(impact)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::NotFound("tool policy".into()))
    }

    pub async fn delete_tool_policy(&self, org_id: Uuid, policy_id: Uuid) -> StoreResult<()> {
        let result = sqlx::query("DELETE FROM tool_policies WHERE org_id = $1 AND id = $2")
            .bind(org_id)
            .bind(policy_id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(StoreError::NotFound("tool policy".into()));
        }
        Ok(())
    }

    pub async fn get_tool_policy(&self, org_id: Uuid, policy_id: Uuid) -> StoreResult<ToolPolicy> {
        sqlx::query_as::<_, ToolPolicy>("SELECT * FROM tool_policies WHERE org_id = $1 AND id = $2")
            .bind(org_id)
            .bind(policy_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| StoreError::NotFound("tool policy".into()))
    }

    /// List rules for management: without an agent this is the org-level set; with an
    /// agent it is the effective set (org defaults + that agent's rules).
    pub async fn list_tool_policies(
        &self,
        org_id: Uuid,
        agent_id: Option<Uuid>,
        access: Option<&crate::access::ResourceAccess>,
    ) -> StoreResult<Vec<ToolPolicy>> {
        let (privileged, user_id, team_ids, isolation) = crate::access::binds(access);
        let rows = match agent_id {
            Some(agent_id) => {
                sqlx::query_as::<_, ToolPolicy>(
                    r#"
                    SELECT * FROM tool_policies
                    WHERE org_id = $1 AND (agent_id IS NULL OR agent_id = $2)
                      AND ($3::bool OR owner_user_id = $4
                           OR CASE WHEN $6::bool AND owner_team_id IS NOT NULL
                                THEN owner_team_id = ANY($5)
                                ELSE visibility = 'org'
                                     OR (visibility = 'team' AND owner_team_id = ANY($5)) END)
                    ORDER BY priority ASC, name ASC
                    "#,
                )
                .bind(org_id)
                .bind(agent_id)
                .bind(privileged)
                .bind(user_id)
                .bind(&team_ids)
                .bind(isolation)
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query_as::<_, ToolPolicy>(
                    r#"
                    SELECT * FROM tool_policies
                    WHERE org_id = $1 AND agent_id IS NULL
                      AND ($2::bool OR owner_user_id = $3
                           OR CASE WHEN $5::bool AND owner_team_id IS NOT NULL
                                THEN owner_team_id = ANY($4)
                                ELSE visibility = 'org'
                                     OR (visibility = 'team' AND owner_team_id = ANY($4)) END)
                    ORDER BY priority ASC, name ASC
                    "#,
                )
                .bind(org_id)
                .bind(privileged)
                .bind(user_id)
                .bind(&team_ids)
                .bind(isolation)
                .fetch_all(&self.pool)
                .await?
            }
        };
        Ok(rows)
    }

    /// Effective rules for an execution, ready for the worker/broker. A row whose stored
    /// action or matcher cannot be parsed is never dropped: it becomes a `deny` rule over
    /// the scope that is still readable (everything when the matcher itself is unreadable).
    pub async fn effective_tool_policy_rules(
        &self,
        org_id: Uuid,
        agent_id: Uuid,
    ) -> StoreResult<Vec<ToolPolicyRule>> {
        let rows = self
            .list_tool_policies(org_id, Some(agent_id), None)
            .await?;
        Ok(rows.into_iter().map(policy_into_rule).collect())
    }
}

fn policy_into_rule(policy: ToolPolicy) -> ToolPolicyRule {
    let action = ToolPolicyAction::parse(&policy.action);
    let matcher = serde_json::from_value::<ToolPolicyMatcher>(policy.matcher.clone());
    let invalid = action.is_none() || matcher.is_err();
    if invalid {
        tracing::error!(
            policy_id = %policy.id,
            policy = %policy.name,
            action = %policy.action,
            "stored tool policy is unparseable; enforcing it as deny"
        );
    }
    let mut reason = policy.reason;
    if invalid && reason.is_none() {
        reason = Some("tool policy could not be parsed; denied (fail closed)".into());
    }
    ToolPolicyRule {
        id: policy.id,
        name: policy.name,
        priority: policy.priority,
        enabled: policy.enabled,
        action: if matcher.is_err() {
            ToolPolicyAction::Deny
        } else {
            action.unwrap_or(ToolPolicyAction::Deny)
        },
        matcher: matcher.unwrap_or_default(),
        reason,
        impact: policy.impact,
    }
}
