//! Resource visibility sharing (ADR 0011).

use chrono::{DateTime, Utc};
use sqlx::FromRow;
use uuid::Uuid;

use crate::{Store, StoreError, StoreResult};

/// Resource keys that carry `owner_user_id`, `owner_team_id` and `visibility`.
/// Outputs are deliberately absent: they are delivery targets scoped to an agent or an
/// Agent Team, not shareable resources.
pub const SHAREABLE_RESOURCES: &[&str] = &[
    "agents",
    "skills",
    "connections",
    "providers",
    "worker_profiles",
    "triggers",
    "tool_policies",
    "agent_teams",
    "env_mappings",
];

fn table(resource: &str) -> StoreResult<&'static str> {
    SHAREABLE_RESOURCES
        .iter()
        .copied()
        .find(|name| *name == resource)
        .ok_or_else(|| StoreError::Invalid(format!("resource '{resource}' is not shareable")))
}

impl Store {
    pub async fn resource_ownership(
        &self,
        org_id: Uuid,
        resource: &str,
        id: Uuid,
    ) -> StoreResult<(Option<Uuid>, Option<Uuid>, String)> {
        let table = table(resource)?;
        let query = format!(
            "SELECT owner_user_id, owner_team_id, visibility FROM {table} WHERE org_id = $1 AND id = $2"
        );
        // Safe: `table` comes from the SHAREABLE_RESOURCES allowlist; all values are bound.
        sqlx::query_as::<_, (Option<Uuid>, Option<Uuid>, String)>(sqlx::AssertSqlSafe(query))
            .bind(org_id)
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| StoreError::NotFound(resource.into()))
    }

    pub async fn set_resource_visibility(
        &self,
        org_id: Uuid,
        resource: &str,
        id: Uuid,
        visibility: &str,
        owner_team_id: Option<Uuid>,
    ) -> StoreResult<()> {
        if !matches!(visibility, "private" | "team" | "org") {
            return Err(StoreError::Invalid(
                "visibility must be private, team or org".into(),
            ));
        }
        let table = table(resource)?;
        let query = format!(
            "UPDATE {table} SET visibility = $3, owner_team_id = $4, updated_at = now() WHERE org_id = $1 AND id = $2"
        );
        // Safe: `table` comes from the SHAREABLE_RESOURCES allowlist; all values are bound.
        let result = sqlx::query(sqlx::AssertSqlSafe(query))
            .bind(org_id)
            .bind(id)
            .bind(visibility)
            .bind(owner_team_id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(StoreError::NotFound(resource.into()));
        }
        Ok(())
    }
}

impl Store {
    /// Set the owner of a newly created resource when the insert did not carry one.
    pub async fn assign_resource_owner(
        &self,
        org_id: Uuid,
        resource: &str,
        id: Uuid,
        user_id: Uuid,
    ) -> StoreResult<()> {
        let table = table(resource)?;
        let query = format!(
            "UPDATE {table} SET owner_user_id = COALESCE(owner_user_id, $3), updated_at = now() WHERE org_id = $1 AND id = $2"
        );
        // Safe: `table` comes from the SHAREABLE_RESOURCES allowlist; all values are bound.
        sqlx::query(sqlx::AssertSqlSafe(query))
            .bind(org_id)
            .bind(id)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

/// One row of a team's access overview. Ownership and visibility come straight from the
/// resource table; the caller's effective access is derived in the API from `AccessScope`.
#[derive(Debug, Clone, FromRow)]
pub struct TeamResourceRow {
    pub resource_type: String,
    pub id: Uuid,
    pub name: String,
    pub owner_user_id: Option<Uuid>,
    pub owner_name: Option<String>,
    pub owner_team_id: Option<Uuid>,
    pub visibility: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct TeamResourceCounts {
    pub total: i64,
    pub agents: i64,
    pub skills: i64,
    pub connections: i64,
    pub providers: i64,
    pub worker_profiles: i64,
    pub agent_teams: i64,
    pub triggers: i64,
    pub tool_policies: i64,
    pub env_mappings: i64,
    pub org_visible: i64,
}

/// Resources owned by this team, plus org-visible resources with no owning team. Never
/// includes another team's resources, even for org admins using the overview.
fn team_resource_union() -> String {
    SHAREABLE_RESOURCES
        .iter()
        .map(|table| {
            // Triggers have no `name` column; their kind is the best human-readable label.
            let name = if *table == "triggers" {
                "kind || ' trigger'"
            } else {
                "name"
            };
            format!(
                "SELECT '{table}'::text AS resource_type, id, {name} AS name, owner_user_id, owner_team_id, visibility, updated_at \
                 FROM {table} WHERE org_id = $1 \
                 AND (owner_team_id = $2 OR (owner_team_id IS NULL AND visibility = 'org'))"
            )
        })
        .collect::<Vec<_>>()
        .join(" UNION ALL ")
}

/// The standard access predicate with placeholders starting at `offset` (ADR 0011).
fn team_visibility_predicate(offset: usize) -> String {
    let p = offset;
    format!(
        "(${p}::bool OR r.owner_user_id = ${} OR CASE WHEN ${}::bool AND r.owner_team_id IS NOT NULL \
         THEN r.owner_team_id = ANY(${}) ELSE r.visibility = 'org' \
         OR (r.visibility = 'team' AND r.owner_team_id = ANY(${})) END)",
        p + 1,
        p + 3,
        p + 2,
        p + 2
    )
}

impl Store {
    /// Page of resources relevant to an access team. The access predicate hides exactly
    /// what `AccessScope::can_view` hides for the calling principal.
    #[allow(clippy::too_many_arguments)]
    pub async fn list_team_resources(
        &self,
        org_id: Uuid,
        team_id: Uuid,
        resource_type: Option<&str>,
        limit: i64,
        cursor: Option<&str>,
        access: Option<&crate::access::ResourceAccess>,
    ) -> StoreResult<(Vec<TeamResourceRow>, Option<String>, i64)> {
        let (privileged, user_id, team_ids, isolation) = crate::access::binds(access);
        let (cursor_ts, cursor_id) = match cursor {
            Some(raw) => match crate::decode_cursor(raw)? {
                Some((ts, id)) => (Some(ts), Some(id)),
                None => (None, None),
            },
            None => (None, None),
        };
        let query = sqlx::AssertSqlSafe(format!(
            "SELECT r.resource_type, r.id, r.name, r.owner_user_id, \
                    u.display_name AS owner_name, r.owner_team_id, r.visibility, r.updated_at \
             FROM ({}) r \
             LEFT JOIN users u ON u.id = r.owner_user_id AND u.org_id = $1 \
             WHERE ($3::text IS NULL OR r.resource_type = $3) \
               AND {} \
               AND ($4::timestamptz IS NULL OR (r.updated_at, r.id) < ($4::timestamptz, $5::uuid)) \
             ORDER BY r.updated_at DESC, r.id DESC \
             LIMIT $6",
            team_resource_union(),
            team_visibility_predicate(7)
        ));
        let rows = sqlx::query_as::<_, TeamResourceRow>(query)
            .bind(org_id)
            .bind(team_id)
            .bind(resource_type)
            .bind(cursor_ts)
            .bind(cursor_id)
            .bind(limit + 1)
            .bind(privileged)
            .bind(user_id)
            .bind(&team_ids)
            .bind(isolation)
            .fetch_all(&self.pool)
            .await?;

        let total_query = sqlx::AssertSqlSafe(format!(
            "SELECT COUNT(*) FROM ({}) r \
             WHERE ($3::text IS NULL OR r.resource_type = $3) AND {}",
            team_resource_union(),
            team_visibility_predicate(4)
        ));
        let total = sqlx::query_scalar::<_, i64>(total_query)
            .bind(org_id)
            .bind(team_id)
            .bind(resource_type)
            .bind(privileged)
            .bind(user_id)
            .bind(&team_ids)
            .bind(isolation)
            .fetch_one(&self.pool)
            .await?;

        let (rows, next) = crate::page_of(rows, limit, |row| (row.updated_at, row.id));
        Ok((rows, next, total))
    }

    /// Visible resource counts by type for the team overview. `org_visible` counts
    /// team-owned resources with organization visibility (the warning callout).
    pub async fn count_team_resources(
        &self,
        org_id: Uuid,
        team_id: Uuid,
        access: Option<&crate::access::ResourceAccess>,
    ) -> StoreResult<TeamResourceCounts> {
        let (privileged, user_id, team_ids, isolation) = crate::access::binds(access);
        let count = |resource: &str| -> String {
            format!("COALESCE(SUM(CASE WHEN r.resource_type = '{resource}' THEN 1 ELSE 0 END), 0)::bigint")
        };
        let query = sqlx::AssertSqlSafe(format!(
            "SELECT {agents}, {skills}, {connections}, {providers}, {worker_profiles}, \
                    {agent_teams}, {triggers}, {tool_policies}, {env_mappings}, \
                    COALESCE(SUM(CASE WHEN r.owner_team_id = $2 AND r.visibility = 'org' THEN 1 ELSE 0 END), 0)::bigint \
             FROM ({union}) r WHERE {visibility}",
            agents = count("agents"),
            skills = count("skills"),
            connections = count("connections"),
            providers = count("providers"),
            worker_profiles = count("worker_profiles"),
            agent_teams = count("agent_teams"),
            triggers = count("triggers"),
            tool_policies = count("tool_policies"),
            env_mappings = count("env_mappings"),
            union = team_resource_union(),
            visibility = team_visibility_predicate(3),
        ));
        let row: (i64, i64, i64, i64, i64, i64, i64, i64, i64, i64) = sqlx::query_as(query)
            .bind(org_id)
            .bind(team_id)
            .bind(privileged)
            .bind(user_id)
            .bind(&team_ids)
            .bind(isolation)
            .fetch_one(&self.pool)
            .await?;
        Ok(TeamResourceCounts {
            total: row.0 + row.1 + row.2 + row.3 + row.4 + row.5 + row.6 + row.7 + row.8,
            agents: row.0,
            skills: row.1,
            connections: row.2,
            providers: row.3,
            worker_profiles: row.4,
            agent_teams: row.5,
            triggers: row.6,
            tool_policies: row.7,
            env_mappings: row.8,
            org_visible: row.9,
        })
    }
}
