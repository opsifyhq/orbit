//! Access teams and memberships (ADR 0011).

use uuid::Uuid;

use crate::models::{AccessTeam, TeamMember};
use crate::{Store, StoreError, StoreResult};

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

    pub async fn delete_access_team(&self, org_id: Uuid, team_id: Uuid) -> StoreResult<()> {
        let mut tx = self.pool.begin().await?;
        let exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM access_teams WHERE org_id = $1 AND id = $2)",
        )
        .bind(org_id)
        .bind(team_id)
        .fetch_one(&mut *tx)
        .await?;
        if !exists {
            return Err(StoreError::NotFound("team".into()));
        }

        let mut blockers: Vec<String> = Vec::new();
        for (label, table) in [
            ("agents", "agents"),
            ("skills", "skills"),
            ("connections", "connections"),
            ("providers", "providers"),
            ("worker profiles", "worker_profiles"),
            ("agent teams", "agent_teams"),
            ("triggers", "triggers"),
            ("tool policies", "tool_policies"),
            ("env mappings", "env_mappings"),
        ] {
            let count: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                "SELECT COUNT(*) FROM {table} WHERE owner_team_id = $1"
            )))
            .bind(team_id)
            .fetch_one(&mut *tx)
            .await?;
            if count > 0 {
                blockers.push(format!("{label} ({count})"));
            }
        }

        if !blockers.is_empty() {
            return Err(StoreError::Conflict(format!(
                "team still owns resources: {}; reassign or remove them before deleting the team",
                blockers.join(", ")
            )));
        }

        let result = sqlx::query("DELETE FROM access_teams WHERE org_id = $1 AND id = $2")
            .bind(org_id)
            .bind(team_id)
            .execute(&mut *tx)
            .await?;
        if result.rows_affected() == 0 {
            return Err(StoreError::NotFound("team".into()));
        }
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
        Ok(sqlx::query_as::<_, TeamMember>(
            r#"
            INSERT INTO team_members (team_id, user_id, role)
            VALUES ($1, $2, $3)
            ON CONFLICT (team_id, user_id) DO UPDATE SET role = EXCLUDED.role
            RETURNING *
            "#,
        )
        .bind(team_id)
        .bind(user_id)
        .bind(role)
        .fetch_one(&self.pool)
        .await?)
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
