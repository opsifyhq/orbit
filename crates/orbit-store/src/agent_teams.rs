//! Agent teams and sequential team runs.

use chrono::Utc;
use serde_json::Value;
use uuid::Uuid;

use crate::models::{AgentTeam, TeamRun, TeamStep};
use crate::{decode_cursor, page_of, Store, StoreError, StoreResult};

/// A step of a workflow run to create.
pub struct NewWorkflowStep<'a> {
    pub run_id: Uuid,
    pub org_id: Uuid,
    pub step_index: i32,
    pub name: &'a str,
    pub agent_id: Uuid,
    pub agent_version_id: Uuid,
    pub skills: &'a [String],
    pub input: &'a Value,
    pub execution_id: Option<Uuid>,
    pub node_id: &'a str,
    pub attempt: i32,
    pub activated_by: &'a Value,
}

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

    #[allow(clippy::too_many_arguments)]
    pub async fn list_team_runs(
        &self,
        org_id: Uuid,
        archived: Option<bool>,
        team_id: Option<Uuid>,
        cursor: Option<&str>,
        query: Option<&str>,
        limit: i64,
        access: Option<&crate::access::ResourceAccess>,
    ) -> StoreResult<(Vec<TeamRun>, Option<String>, i64)> {
        let (privileged, user_id, team_ids, isolation) = crate::access::binds(access);
        let (cursor_ts, cursor_id) = match cursor {
            Some(raw) => match decode_cursor(raw)? {
                Some((ts, id)) => (Some(ts), Some(id)),
                None => (None, None),
            },
            None => (None, None),
        };
        let rows = sqlx::query_as::<_, TeamRun>(
            r#"
            SELECT r.* FROM team_runs r
            JOIN agent_teams t ON t.id = r.team_id AND t.org_id = r.org_id
            WHERE r.org_id = $1
              AND ($7::uuid IS NULL OR r.team_id = $7)
              AND ($2::boolean IS NULL
                   OR ($2 = FALSE AND r.archived_at IS NULL)
                   OR ($2 = TRUE AND r.archived_at IS NOT NULL))
              AND ($9::text IS NULL
                   OR r.input ->> 'task' ILIKE '%' || $9 || '%'
                   OR t.name ILIKE '%' || $9 || '%'
                   OR r.status ILIKE '%' || $9 || '%')
              AND ($10::timestamptz IS NULL OR (r.created_at, r.id) < ($10::timestamptz, $11::uuid))
              AND ($3::bool OR t.owner_user_id = $4
                   OR CASE WHEN $6::bool AND t.owner_team_id IS NOT NULL
                        THEN t.owner_team_id = ANY($5)
                        ELSE t.visibility = 'org'
                             OR (t.visibility = 'team' AND t.owner_team_id = ANY($5)) END)
            ORDER BY r.created_at DESC, r.id DESC
            LIMIT $8
            "#,
        )
        .bind(org_id)
        .bind(archived)
        .bind(privileged)
        .bind(user_id)
        .bind(&team_ids)
        .bind(isolation)
        .bind(team_id)
        .bind(limit + 1)
        .bind(query)
        .bind(cursor_ts)
        .bind(cursor_id)
        .fetch_all(&self.pool)
        .await?;
        let total: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*) FROM team_runs r
            JOIN agent_teams t ON t.id = r.team_id AND t.org_id = r.org_id
            WHERE r.org_id = $1
              AND ($7::uuid IS NULL OR r.team_id = $7)
              AND ($2::boolean IS NULL
                   OR ($2 = FALSE AND r.archived_at IS NULL)
                   OR ($2 = TRUE AND r.archived_at IS NOT NULL))
              AND ($8::text IS NULL
                   OR r.input ->> 'task' ILIKE '%' || $8 || '%'
                   OR t.name ILIKE '%' || $8 || '%'
                   OR r.status ILIKE '%' || $8 || '%')
              AND ($3::bool OR t.owner_user_id = $4
                   OR CASE WHEN $6::bool AND t.owner_team_id IS NOT NULL
                        THEN t.owner_team_id = ANY($5)
                        ELSE t.visibility = 'org'
                             OR (t.visibility = 'team' AND t.owner_team_id = ANY($5)) END)
            "#,
        )
        .bind(org_id)
        .bind(archived)
        .bind(privileged)
        .bind(user_id)
        .bind(&team_ids)
        .bind(isolation)
        .bind(team_id)
        .bind(query)
        .fetch_one(&self.pool)
        .await?;
        let (rows, next) = page_of(rows, limit, |row| (row.created_at, row.id));
        Ok((rows, next, total))
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

    /// One page of running team runs with `id > after`, ordered by id. Callers page with
    /// the last id until a page comes back empty, so every running run is visited and
    /// none is starved by older ones.
    pub async fn list_running_team_runs(
        &self,
        after: Option<Uuid>,
        limit: i64,
    ) -> StoreResult<Vec<TeamRun>> {
        Ok(sqlx::query_as::<_, TeamRun>(
            "SELECT * FROM team_runs WHERE status = 'running' AND ($1::uuid IS NULL OR id > $1) ORDER BY id ASC LIMIT $2",
        )
        .bind(after)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Take the advance lease for a running run. Only one caller (replica) holds it at a
    /// time; an expired lease (crashed holder) can be taken over.
    pub async fn claim_team_run_advance(
        &self,
        run_id: Uuid,
        lease_secs: i64,
    ) -> StoreResult<Option<(TeamRun, chrono::DateTime<chrono::Utc>)>> {
        let row = sqlx::query(
            r#"
            UPDATE team_runs
            SET advance_claimed_at = clock_timestamp()
            WHERE id = $1 AND status = 'running'
              AND (advance_claimed_at IS NULL
                   OR advance_claimed_at < now() - make_interval(secs => $2::double precision))
            RETURNING *
            "#,
        )
        .bind(run_id)
        .bind(lease_secs as f64)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else { return Ok(None) };
        let claimed_at: chrono::DateTime<chrono::Utc> =
            sqlx::Row::try_get(&row, "advance_claimed_at")?;
        let run = <TeamRun as sqlx::FromRow<sqlx::postgres::PgRow>>::from_row(&row)?;
        Ok(Some((run, claimed_at)))
    }

    /// Release the advance lease, but only if `claimed_at` (the token returned by
    /// `claim_team_run_advance`) is still the current lease. A holder whose lease expired
    /// and was taken over must not clear the new holder's lease.
    pub async fn release_team_run_advance(
        &self,
        run_id: Uuid,
        claimed_at: chrono::DateTime<chrono::Utc>,
    ) -> StoreResult<bool> {
        let result = sqlx::query(
            "UPDATE team_runs SET advance_claimed_at = NULL WHERE id = $1 AND advance_claimed_at = $2",
        )
        .bind(run_id)
        .bind(claimed_at)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Move a `running` run to a terminal status. `None` when it was no longer running.
    /// Request cancellation of every live execution of one team run. Queued executions
    /// are cancelled outright (no worker has them); claimed ones get `cancel_requested`
    /// and are stopped by their worker. Returns the executions that were changed.
    pub async fn request_team_run_cancel(
        &self,
        org_id: Uuid,
        run_id: Uuid,
    ) -> StoreResult<Vec<crate::models::ExecutionRecord>> {
        self.request_team_executions_cancel(org_id, Some(run_id), None)
            .await
    }

    /// Like [`Self::request_team_run_cancel`] for every run of a team. Call it before the
    /// team is deleted: deleting it detaches the executions (`team_run_id` is
    /// `ON DELETE SET NULL`), and nothing would stop them afterwards.
    pub async fn request_team_cancel(
        &self,
        org_id: Uuid,
        team_id: Uuid,
    ) -> StoreResult<Vec<crate::models::ExecutionRecord>> {
        self.request_team_executions_cancel(org_id, None, Some(team_id))
            .await
    }

    async fn request_team_executions_cancel(
        &self,
        org_id: Uuid,
        run_id: Option<Uuid>,
        team_id: Option<Uuid>,
    ) -> StoreResult<Vec<crate::models::ExecutionRecord>> {
        let mut tx = self.pool.begin().await?;
        let mut changed = sqlx::query_as::<_, crate::models::ExecutionRecord>(
            r#"
            UPDATE executions
            SET status = 'cancelled', finished_at = now(), cancel_requested = true, updated_at = now()
            WHERE org_id = $1
              AND status = 'queued'
              AND team_run_id IN (
                  SELECT r.id FROM team_runs r
                  WHERE r.org_id = $1
                    AND ($2::uuid IS NULL OR r.id = $2)
                    AND ($3::uuid IS NULL OR r.team_id = $3)
              )
            RETURNING *
            "#,
        )
        .bind(org_id)
        .bind(run_id)
        .bind(team_id)
        .fetch_all(&mut *tx)
        .await?;
        let flagged = sqlx::query_as::<_, crate::models::ExecutionRecord>(
            r#"
            UPDATE executions
            SET cancel_requested = true, updated_at = now()
            WHERE org_id = $1
              AND status IN ('dispatched', 'running', 'awaiting_approval')
              AND NOT cancel_requested
              AND team_run_id IN (
                  SELECT r.id FROM team_runs r
                  WHERE r.org_id = $1
                    AND ($2::uuid IS NULL OR r.id = $2)
                    AND ($3::uuid IS NULL OR r.team_id = $3)
              )
            RETURNING *
            "#,
        )
        .bind(org_id)
        .bind(run_id)
        .bind(team_id)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        changed.extend(flagged);
        Ok(changed)
    }

    /// True while any execution of the run is not terminal.
    pub async fn team_run_has_live_executions(&self, run_id: Uuid) -> StoreResult<bool> {
        Ok(sqlx::query_scalar(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM executions
                WHERE team_run_id = $1
                  AND status IN ('queued', 'dispatched', 'running', 'awaiting_approval')
            )
            "#,
        )
        .bind(run_id)
        .fetch_one(&self.pool)
        .await?)
    }

    /// End a running team run. Steps still `running` are closed in the same statement
    /// (#431): a finished run must never show a live step, whichever path ended it.
    pub async fn finish_team_run(
        &self,
        run_id: Uuid,
        status: &str,
        output: Option<&Value>,
        error: Option<&str>,
    ) -> StoreResult<Option<TeamRun>> {
        Ok(sqlx::query_as::<_, TeamRun>(
            r#"
            WITH run AS (
                UPDATE team_runs
                SET status = $2, output = COALESCE($3, output), error = $4,
                    finished_at = now(), updated_at = now()
                WHERE id = $1 AND status = 'running'
                RETURNING *
            ), closed AS (
                UPDATE team_steps
                SET status = 'cancelled',
                    error = CASE WHEN $2 = 'cancelled' THEN error
                                 ELSE 'stopped because the run ended' END,
                    finished_at = now(), updated_at = now()
                WHERE run_id IN (SELECT id FROM run) AND status = 'running'
            )
            SELECT * FROM run
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
        // As in `finish_team_run`, a run that ends closes its running steps (#431).
        sqlx::query_as::<_, TeamRun>(
            r#"
            WITH run AS (
                UPDATE team_runs
                SET status = $2, output = COALESCE($3, output), error = $4,
                    finished_at = CASE WHEN $5 THEN now() ELSE finished_at END,
                    updated_at = now()
                WHERE id = $1
                  AND (status NOT IN ('succeeded', 'failed', 'cancelled') OR status = $2)
                RETURNING *
            ), closed AS (
                UPDATE team_steps
                SET status = 'cancelled',
                    error = CASE WHEN $2 = 'cancelled' THEN error
                                 ELSE 'stopped because the run ended' END,
                    finished_at = now(), updated_at = now()
                WHERE $5 AND run_id IN (SELECT id FROM run) AND status = 'running'
            )
            SELECT * FROM run
            "#,
        )
        .bind(run_id)
        .bind(status)
        .bind(output)
        .bind(error)
        .bind(finished)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::Conflict("team run is already finished".into()))
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

    /// Create a step of a workflow run. `step_index` is the order of creation; the
    /// unique `(run_id, step_index)` makes a repeated create a conflict instead of a twin.
    pub async fn create_workflow_step(&self, step: NewWorkflowStep<'_>) -> StoreResult<TeamStep> {
        sqlx::query_as::<_, TeamStep>(
            r#"
            INSERT INTO team_steps
                (id, run_id, org_id, step_index, name, agent_id, agent_version_id, skills,
                 status, input, execution_id, started_at, node_id, attempt, activated_by)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'running', $9, $10, now(), $11, $12, $13)
            RETURNING *
            "#,
        )
        .bind(orbit_domain::new_id())
        .bind(step.run_id)
        .bind(step.org_id)
        .bind(step.step_index)
        .bind(step.name)
        .bind(step.agent_id)
        .bind(step.agent_version_id)
        .bind(step.skills)
        .bind(step.input)
        .bind(step.execution_id)
        .bind(step.node_id)
        .bind(step.attempt)
        .bind(step.activated_by)
        .fetch_one(&self.pool)
        .await
        .map_err(Into::into)
    }

    /// Close the steps of a run that are still `running`, when the run itself is over
    /// (cancelled, or failed while other branches were still working). Without this a
    /// finished run would keep showing a step as running. Returns how many changed.
    pub async fn close_running_team_steps(
        &self,
        run_id: Uuid,
        status: &str,
        error: Option<&str>,
    ) -> StoreResult<u64> {
        let result = sqlx::query(
            r#"
            UPDATE team_steps
            SET status = $2, error = $3, finished_at = now(), updated_at = now()
            WHERE run_id = $1 AND status = 'running'
            "#,
        )
        .bind(run_id)
        .bind(status)
        .bind(error)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// The step an execution belongs to, if it is a team step.
    pub async fn team_step_by_execution(
        &self,
        execution_id: Uuid,
    ) -> StoreResult<Option<TeamStep>> {
        Ok(sqlx::query_as::<_, TeamStep>(
            "SELECT * FROM team_steps WHERE execution_id = $1 ORDER BY step_index DESC LIMIT 1",
        )
        .bind(execution_id)
        .fetch_optional(&self.pool)
        .await?)
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
        self.finish_team_step_with_outcome(step_id, status, output, error, None)
            .await
    }

    /// [`Self::finish_team_step`] that also records the outcome a workflow step reported.
    pub async fn finish_team_step_with_outcome(
        &self,
        step_id: Uuid,
        status: &str,
        output: Option<&Value>,
        error: Option<&str>,
        outcome: Option<&str>,
    ) -> StoreResult<Option<TeamStep>> {
        Ok(sqlx::query_as::<_, TeamStep>(
            r#"
            UPDATE team_steps
            SET status = $2, output = $3, error = $4, outcome = $5,
                finished_at = now(), updated_at = now()
            WHERE id = $1 AND status = 'running'
            RETURNING *
            "#,
        )
        .bind(step_id)
        .bind(status)
        .bind(output)
        .bind(error)
        .bind(outcome)
        .fetch_optional(&self.pool)
        .await?)
    }
}
