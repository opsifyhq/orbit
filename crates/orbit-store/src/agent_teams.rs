//! Agent teams and sequential team runs.

use chrono::Utc;
use serde_json::Value;
use uuid::Uuid;

use crate::models::{AgentTeam, TeamRun, TeamStep};
use crate::{Store, StoreError, StoreResult};

impl Store {
    #[allow(clippy::too_many_arguments)]
    pub async fn create_agent_team(
        &self,
        org_id: Uuid,
        name: &str,
        description: &str,
        definition: &Value,
        created_by: Option<Uuid>,
    ) -> StoreResult<AgentTeam> {
        sqlx::query_as::<_, AgentTeam>(
            r#"
            INSERT INTO agent_teams (id, org_id, name, description, definition, created_by)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING *
            "#,
        )
        .bind(orbit_domain::new_id())
        .bind(org_id)
        .bind(name)
        .bind(description)
        .bind(definition)
        .bind(created_by)
        .fetch_one(&self.pool)
        .await
        .map_err(Into::into)
    }

    pub async fn list_agent_teams(
        &self,
        org_id: Uuid,
        access: Option<&crate::access::ResourceAccess>,
    ) -> StoreResult<Vec<AgentTeam>> {
        let (privileged, user_id, team_ids, isolation) = crate::access::binds(access);
        Ok(sqlx::query_as::<_, AgentTeam>(
            r#"
            SELECT * FROM agent_teams
            WHERE org_id = $1
              AND ($2::bool OR owner_user_id = $3
                   OR CASE WHEN $5::bool AND owner_team_id IS NOT NULL
                        THEN owner_team_id = ANY($4)
                        ELSE visibility = 'org'
                             OR (visibility = 'team' AND owner_team_id = ANY($4)) END)
            ORDER BY created_at DESC
            "#,
        )
        .bind(org_id)
        .bind(privileged)
        .bind(user_id)
        .bind(&team_ids)
        .bind(isolation)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Teams with an enabled work source, for the background dispatcher.
    pub async fn list_agent_teams_with_enabled_work_source(&self) -> StoreResult<Vec<AgentTeam>> {
        Ok(sqlx::query_as::<_, AgentTeam>(
            "SELECT * FROM agent_teams WHERE definition -> 'workSource' ->> 'enabled' = 'true'",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn set_team_poll_status(
        &self,
        org_id: Uuid,
        team_id: Uuid,
        last_error: Option<&str>,
    ) -> StoreResult<AgentTeam> {
        sqlx::query_as::<_, AgentTeam>(
            r#"
            UPDATE agent_teams
            SET last_poll_at = now(), last_poll_error = $3, updated_at = now()
            WHERE org_id = $1 AND id = $2
            RETURNING *
            "#,
        )
        .bind(org_id)
        .bind(team_id)
        .bind(last_error)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::NotFound("agent team".into()))
    }

    pub async fn get_agent_team(&self, org_id: Uuid, team_id: Uuid) -> StoreResult<AgentTeam> {
        sqlx::query_as::<_, AgentTeam>("SELECT * FROM agent_teams WHERE org_id = $1 AND id = $2")
            .bind(org_id)
            .bind(team_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| StoreError::NotFound("agent team".into()))
    }

    pub async fn update_agent_team(
        &self,
        org_id: Uuid,
        team_id: Uuid,
        name: &str,
        description: &str,
        definition: &Value,
    ) -> StoreResult<AgentTeam> {
        sqlx::query_as::<_, AgentTeam>(
            r#"
            UPDATE agent_teams
            SET name = $3, description = $4, definition = $5, updated_at = now()
            WHERE org_id = $1 AND id = $2
            RETURNING *
            "#,
        )
        .bind(org_id)
        .bind(team_id)
        .bind(name)
        .bind(description)
        .bind(definition)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::NotFound("agent team".into()))
    }

    pub async fn delete_agent_team(&self, org_id: Uuid, team_id: Uuid) -> StoreResult<()> {
        let result = sqlx::query("DELETE FROM agent_teams WHERE org_id = $1 AND id = $2")
            .bind(org_id)
            .bind(team_id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(StoreError::NotFound("agent team".into()));
        }
        Ok(())
    }

    pub async fn create_team_run(
        &self,
        org_id: Uuid,
        team_id: Uuid,
        input: &Value,
        requested_by: Option<Uuid>,
    ) -> StoreResult<TeamRun> {
        sqlx::query_as::<_, TeamRun>(
            r#"
            INSERT INTO team_runs (id, org_id, team_id, status, input, requested_by)
            VALUES ($1, $2, $3, 'running', $4, $5)
            RETURNING *
            "#,
        )
        .bind(orbit_domain::new_id())
        .bind(org_id)
        .bind(team_id)
        .bind(input)
        .bind(requested_by)
        .fetch_one(&self.pool)
        .await
        .map_err(Into::into)
    }

    pub async fn get_team_run(&self, org_id: Uuid, run_id: Uuid) -> StoreResult<TeamRun> {
        sqlx::query_as::<_, TeamRun>("SELECT * FROM team_runs WHERE org_id = $1 AND id = $2")
            .bind(org_id)
            .bind(run_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| StoreError::NotFound("team run".into()))
    }

    pub async fn list_team_runs(
        &self,
        org_id: Uuid,
        archived: Option<bool>,
        access: Option<&crate::access::ResourceAccess>,
    ) -> StoreResult<Vec<TeamRun>> {
        let (privileged, user_id, team_ids, isolation) = crate::access::binds(access);
        Ok(sqlx::query_as::<_, TeamRun>(
            r#"
            SELECT r.* FROM team_runs r
            JOIN agent_teams t ON t.id = r.team_id AND t.org_id = r.org_id
            WHERE r.org_id = $1
              AND ($2::boolean IS NULL
                   OR ($2 = FALSE AND r.archived_at IS NULL)
                   OR ($2 = TRUE AND r.archived_at IS NOT NULL))
              AND ($3::bool OR t.owner_user_id = $4
                   OR CASE WHEN $6::bool AND t.owner_team_id IS NOT NULL
                        THEN t.owner_team_id = ANY($5)
                        ELSE t.visibility = 'org'
                             OR (t.visibility = 'team' AND t.owner_team_id = ANY($5)) END)
            ORDER BY r.created_at DESC
            LIMIT 100
            "#,
        )
        .bind(org_id)
        .bind(archived)
        .bind(privileged)
        .bind(user_id)
        .bind(&team_ids)
        .bind(isolation)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Archive terminal team runs whose last activity predates `cutoff`.
    pub async fn archive_team_runs_before(
        &self,
        cutoff: chrono::DateTime<Utc>,
    ) -> StoreResult<u64> {
        let result = sqlx::query(
            r#"
            UPDATE team_runs
            SET archived_at = now(), updated_at = now()
            WHERE archived_at IS NULL
              AND finished_at IS NOT NULL
              AND finished_at < $1
              AND status IN ('succeeded', 'failed', 'cancelled')
            "#,
        )
        .bind(cutoff)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    pub async fn list_running_team_runs(&self) -> StoreResult<Vec<TeamRun>> {
        Ok(sqlx::query_as::<_, TeamRun>(
            "SELECT * FROM team_runs WHERE status = 'running' ORDER BY created_at ASC LIMIT 50",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    /// Take the advance lease for a running run. Only one caller (replica) holds it at a
    /// time; an expired lease (crashed holder) can be taken over.
    pub async fn claim_team_run_advance(
        &self,
        run_id: Uuid,
        lease_secs: i64,
    ) -> StoreResult<Option<TeamRun>> {
        Ok(sqlx::query_as::<_, TeamRun>(
            r#"
            UPDATE team_runs
            SET advance_claimed_at = now()
            WHERE id = $1 AND status = 'running'
              AND (advance_claimed_at IS NULL
                   OR advance_claimed_at < now() - make_interval(secs => $2::double precision))
            RETURNING *
            "#,
        )
        .bind(run_id)
        .bind(lease_secs as f64)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn release_team_run_advance(
        &self,
        run_id: Uuid,
        claimed_at: chrono::DateTime<Utc>,
    ) -> StoreResult<()> {
        sqlx::query(
            "UPDATE team_runs SET advance_claimed_at = NULL \
             WHERE id = $1 AND advance_claimed_at = $2",
        )
        .bind(run_id)
        .bind(claimed_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Move a `running` run to a terminal status. `None` when it was no longer running.
    pub async fn finish_team_run(
        &self,
        run_id: Uuid,
        status: &str,
        output: Option<&Value>,
        error: Option<&str>,
    ) -> StoreResult<Option<TeamRun>> {
        Ok(sqlx::query_as::<_, TeamRun>(
            r#"
            UPDATE team_runs
            SET status = $2, output = COALESCE($3, output), error = $4,
                finished_at = now(), updated_at = now()
            WHERE id = $1 AND status = 'running'
            RETURNING *
            "#,
        )
        .bind(run_id)
        .bind(status)
        .bind(output)
        .bind(error)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn update_team_run(
        &self,
        run_id: Uuid,
        status: &str,
        output: Option<&Value>,
        error: Option<&str>,
    ) -> StoreResult<TeamRun> {
        let finished = matches!(status, "succeeded" | "failed" | "cancelled");
        sqlx::query_as::<_, TeamRun>(
            r#"
            UPDATE team_runs
            SET status = $2, output = COALESCE($3, output), error = $4,
                finished_at = CASE WHEN $5 THEN now() ELSE finished_at END,
                updated_at = now()
            WHERE id = $1 AND status NOT IN ('succeeded', 'failed', 'cancelled')
            RETURNING *
            "#,
        )
        .bind(run_id)
        .bind(status)
        .bind(output)
        .bind(error)
        .bind(finished)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::Conflict("team run is already terminal".into()))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_team_step(
        &self,
        run_id: Uuid,
        org_id: Uuid,
        step_index: i32,
        name: &str,
        agent_id: Uuid,
        agent_version_id: Uuid,
        skills: &[String],
        input: &Value,
        execution_id: Option<Uuid>,
    ) -> StoreResult<TeamStep> {
        sqlx::query_as::<_, TeamStep>(
            r#"
            INSERT INTO team_steps
                (id, run_id, org_id, step_index, name, agent_id, agent_version_id, skills,
                 status, input, execution_id)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'running', $9, $10)
            RETURNING *
            "#,
        )
        .bind(orbit_domain::new_id())
        .bind(run_id)
        .bind(org_id)
        .bind(step_index)
        .bind(name)
        .bind(agent_id)
        .bind(agent_version_id)
        .bind(skills)
        .bind(input)
        .bind(execution_id)
        .fetch_one(&self.pool)
        .await
        .map_err(Into::into)
    }

    pub async fn list_team_steps(&self, run_id: Uuid) -> StoreResult<Vec<TeamStep>> {
        Ok(sqlx::query_as::<_, TeamStep>(
            "SELECT * FROM team_steps WHERE run_id = $1 ORDER BY step_index ASC",
        )
        .bind(run_id)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn current_team_step(&self, run_id: Uuid) -> StoreResult<Option<TeamStep>> {
        Ok(sqlx::query_as::<_, TeamStep>(
            "SELECT * FROM team_steps WHERE run_id = $1 ORDER BY step_index DESC LIMIT 1",
        )
        .bind(run_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    /// Conditional on `status = 'running'`; `None` means another caller already finished it.
    pub async fn finish_team_step(
        &self,
        step_id: Uuid,
        status: &str,
        output: Option<&Value>,
        error: Option<&str>,
    ) -> StoreResult<Option<TeamStep>> {
        Ok(sqlx::query_as::<_, TeamStep>(
            r#"
            UPDATE team_steps
            SET status = $2, output = $3, error = $4,
                finished_at = now(), updated_at = now()
            WHERE id = $1 AND status = 'running'
            RETURNING *
            "#,
        )
        .bind(step_id)
        .bind(status)
        .bind(output)
        .bind(error)
        .fetch_optional(&self.pool)
        .await?)
    }
}
