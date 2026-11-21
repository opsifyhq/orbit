//! Access teams and memberships (ADR 0011).

use uuid::Uuid;

use crate::models::{AccessTeam, TeamMember};
use crate::{Store, StoreError, StoreResult};

/// Team-visible resources owned by one access team, per table (ADR 0011).
macro_rules! team_owned_counts_sql {
    ($($table:literal),+) => {
        concat!($(
            "SELECT '", $table, "' AS resource, count(*) AS n FROM ", $table,
            " WHERE org_id = $1 AND owner_team_id = $2 AND visibility = 'team' UNION ALL "
        ),+, "SELECT 'none', 0::bigint WHERE false")
    };
}

const TEAM_OWNED_COUNTS_SQL: &str = team_owned_counts_sql!(
    "agents",
    "skills",
    "connections",
    "providers",
    "worker_profiles",
    "triggers",
    "tool_policies",
    "agent_teams",
    "env_mappings"
);

impl Store {
    pub async fn list_access_teams(&self, org_id: Uuid) -> StoreResult<Vec<AccessTeam>> {
        Ok(sqlx::query_as::<_, AccessTeam>(
            "SELECT * FROM access_teams WHERE org_id = $1 ORDER BY name ASC",
        )
        .bind(org_id)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn get_access_team(&self, org_id: Uuid, team_id: Uuid) -> StoreResult<AccessTeam> {
        sqlx::query_as::<_, AccessTeam>("SELECT * FROM access_teams WHERE org_id = $1 AND id = $2")
            .bind(org_id)
            .bind(team_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| StoreError::NotFound("team".into()))
    }

    pub async fn create_access_team(
        &self,
        org_id: Uuid,
        name: &str,
        slug: &str,
        created_by: Option<Uuid>,
    ) -> StoreResult<AccessTeam> {
        sqlx::query_as::<_, AccessTeam>(
            r#"
            INSERT INTO access_teams (id, org_id, name, slug, created_by)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING *
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(org_id)
        .bind(name)
        .bind(slug)
        .bind(created_by)
        .fetch_one(&self.pool)
        .await
        .map_err(|error| {
            if error
                .as_database_error()
                .map(|db| db.is_unique_violation())
                .unwrap_or(false)
            {
                StoreError::Conflict(format!("team '{name}' already exists"))
            } else {
                error.into()
            }
        })
    }

    pub async fn update_access_team(
        &self,
        org_id: Uuid,
        team_id: Uuid,
        name: &str,
        slug: &str,
    ) -> StoreResult<AccessTeam> {
        sqlx::query_as::<_, AccessTeam>(
            r#"
            UPDATE access_teams SET name = $3, slug = $4, updated_at = now()
            WHERE org_id = $1 AND id = $2
            RETURNING *
            "#,
        )
        .bind(org_id)
        .bind(team_id)
        .bind(name)
        .bind(slug)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::NotFound("team".into()))
    }

    /// Delete an access team. Refused with a `Conflict` that counts what still belongs to
    /// the team while any resource is team-visible and owned by it: the FK would otherwise
    /// null `owner_team_id` and leave those resources visible only to their owner.
    pub async fn delete_access_team(&self, org_id: Uuid, team_id: Uuid) -> StoreResult<()> {
        let mut tx = self.pool.begin().await?;
        // Row lock: concurrent inserts referencing the team take FOR KEY SHARE on it.
        let found: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM access_teams WHERE org_id = $1 AND id = $2 FOR UPDATE",
        )
        .bind(org_id)
        .bind(team_id)
        .fetch_optional(&mut *tx)
        .await?;
        if found.is_none() {
            return Err(StoreError::NotFound("team".into()));
        }
        let counts: Vec<(String, i64)> = sqlx::query_as(TEAM_OWNED_COUNTS_SQL)
            .bind(org_id)
            .bind(team_id)
            .fetch_all(&mut *tx)
            .await?;
        let blocking: Vec<String> = counts
            .into_iter()
            .filter(|(_, n)| *n > 0)
            .map(|(table, n)| format!("{n} {}", table.replace('_', " ")))
            .collect();
        if !blocking.is_empty() {
            return Err(StoreError::Conflict(format!(
                "team still owns team-visible resources ({}); move or delete them first",
                blocking.join(", ")
            )));
        }
        sqlx::query("DELETE FROM access_teams WHERE org_id = $1 AND id = $2")
            .bind(org_id)
            .bind(team_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn list_team_members(&self, team_id: Uuid) -> StoreResult<Vec<TeamMember>> {
        Ok(sqlx::query_as::<_, TeamMember>(
            "SELECT * FROM team_members WHERE team_id = $1 ORDER BY created_at ASC",
        )
        .bind(team_id)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn upsert_team_member(
        &self,
        team_id: Uuid,
        user_id: Uuid,
        role: &str,
    ) -> StoreResult<TeamMember> {
        if !matches!(role, "lead" | "member" | "viewer") {
            return Err(StoreError::Invalid(
                "team role must be lead, member or viewer".into(),
            ));
        }
        // The user must belong to the team's org; otherwise report the same NotFound as an
        // unknown id so the response is not an oracle for user ids in other orgs.
        sqlx::query_as::<_, TeamMember>(
            r#"
            INSERT INTO team_members (team_id, user_id, role)
            SELECT t.id, u.id, $3
            FROM access_teams t
            JOIN users u ON u.org_id = t.org_id
            WHERE t.id = $1 AND u.id = $2
            ON CONFLICT (team_id, user_id) DO UPDATE SET role = EXCLUDED.role
            RETURNING *
            "#,
        )
        .bind(team_id)
        .bind(user_id)
        .bind(role)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::NotFound("user".into()))
    }

    pub async fn remove_team_member(&self, team_id: Uuid, user_id: Uuid) -> StoreResult<()> {
        sqlx::query("DELETE FROM team_members WHERE team_id = $1 AND user_id = $2")
            .bind(team_id)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Ensure a new organization has a Default access team. Safe to call repeatedly.
    pub async fn ensure_default_access_team(&self, org_id: Uuid) -> StoreResult<AccessTeam> {
        if let Some(team) = sqlx::query_as::<_, AccessTeam>(
            "SELECT * FROM access_teams WHERE org_id = $1 AND slug = 'default'",
        )
        .bind(org_id)
        .fetch_optional(&self.pool)
        .await?
        {
            return Ok(team);
        }
        self.create_access_team(org_id, "Default", "default", None)
            .await
    }
}

impl Store {
    pub async fn list_user_team_roles(
        &self,
        org_id: Uuid,
        user_id: Uuid,
    ) -> StoreResult<Vec<(Uuid, String)>> {
        let rows: Vec<(Uuid, String)> = sqlx::query_as(
            r#"
            SELECT m.team_id, m.role
            FROM team_members m
            JOIN access_teams t ON t.id = m.team_id
            WHERE t.org_id = $1 AND m.user_id = $2
            "#,
        )
        .bind(org_id)
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }
}
