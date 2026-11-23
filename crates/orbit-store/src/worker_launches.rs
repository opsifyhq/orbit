//! Worker launches: durable records of launcher-managed worker instances.

use chrono::Utc;
use uuid::Uuid;

use crate::models::WorkerLaunch;
use crate::{Store, StoreError, StoreResult};

impl Store {
    #[allow(clippy::too_many_arguments)]
    pub async fn create_worker_launch(
        &self,
        org_id: Uuid,
        profile_id: Uuid,
        name: &str,
        target: &str,
        image: &str,
        worker_token_id: Option<Uuid>,
        created_by: Option<Uuid>,
        single_execution: bool,
    ) -> StoreResult<WorkerLaunch> {
        sqlx::query_as::<_, WorkerLaunch>(
            r#"
            INSERT INTO worker_launches
                (id, org_id, profile_id, name, target, image, worker_token_id, created_by,
                 single_execution)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            RETURNING *
            "#,
        )
        .bind(orbit_domain::new_id())
        .bind(org_id)
        .bind(profile_id)
        .bind(name)
        .bind(target)
        .bind(image)
        .bind(worker_token_id)
        .bind(created_by)
        .bind(single_execution)
        .fetch_one(&self.pool)
        .await
        .map_err(Into::into)
    }

    /// Mark a launch as existing for exactly one execution.
    pub async fn set_worker_launch_single_execution(
        &self,
        launch_id: Uuid,
        execution_id: Uuid,
    ) -> StoreResult<()> {
        sqlx::query("UPDATE worker_launches SET single_execution_id = $2 WHERE id = $1")
            .bind(launch_id)
            .bind(execution_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Revoke the worker tokens of single-execution launches whose execution is terminal.
    /// Returns the revoked token ids.
    pub async fn revoke_finished_single_execution_tokens(&self) -> StoreResult<Vec<Uuid>> {
        Ok(sqlx::query_scalar::<_, Uuid>(
            r#"
            UPDATE worker_tokens t
            SET revoked_at = now()
            FROM worker_launches l
            JOIN executions e ON e.id = l.single_execution_id
            WHERE l.worker_token_id = t.id
              AND t.revoked_at IS NULL
              AND e.status IN ('succeeded', 'failed', 'timed_out', 'cancelled', 'rejected')
            RETURNING t.id
            "#,
        )
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn get_worker_launch(
        &self,
        org_id: Uuid,
        launch_id: Uuid,
    ) -> StoreResult<WorkerLaunch> {
        sqlx::query_as::<_, WorkerLaunch>(
            "SELECT * FROM worker_launches WHERE org_id = $1 AND id = $2",
        )
        .bind(org_id)
        .bind(launch_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::NotFound("worker launch".into()))
    }

    /// The live (pending/starting/running, not archived) long-lived launch of a profile.
    /// Exact lookup: not subject to any list cap. Single-execution launches are excluded.
    pub async fn find_live_launch_for_profile(
        &self,
        org_id: Uuid,
        profile_id: Uuid,
    ) -> StoreResult<Option<WorkerLaunch>> {
        Ok(sqlx::query_as::<_, WorkerLaunch>(
            r#"
            SELECT * FROM worker_launches
            WHERE org_id = $1 AND profile_id = $2
              AND single_execution = false
              AND archived_at IS NULL
              AND status IN ('pending', 'starting', 'running')
            ORDER BY created_at DESC
            LIMIT 1
            "#,
        )
        .bind(org_id)
        .bind(profile_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    /// Every live (pending/starting/running, not archived) launch of a profile, including
    /// single-execution launches. Uncapped.
    pub async fn active_launches_for_profile(
        &self,
        org_id: Uuid,
        profile_id: Uuid,
    ) -> StoreResult<Vec<WorkerLaunch>> {
        Ok(sqlx::query_as::<_, WorkerLaunch>(
            r#"
            SELECT * FROM worker_launches
            WHERE org_id = $1 AND profile_id = $2
              AND archived_at IS NULL
              AND status IN ('pending', 'starting', 'running')
            ORDER BY created_at DESC
            "#,
        )
        .bind(org_id)
        .bind(profile_id)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn list_worker_launches(
        &self,
        org_id: Uuid,
        archived: Option<bool>,
    ) -> StoreResult<Vec<WorkerLaunch>> {
        Ok(sqlx::query_as::<_, WorkerLaunch>(
            "SELECT * FROM worker_launches WHERE org_id = $1 AND ($2::boolean IS NULL OR ($2 = FALSE AND archived_at IS NULL) OR ($2 = TRUE AND archived_at IS NOT NULL)) ORDER BY created_at DESC LIMIT 100",
        )
        .bind(org_id)
        .bind(archived)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Archive terminal worker launches whose last activity predates `cutoff`.
    pub async fn archive_worker_launches_before(
        &self,
        cutoff: chrono::DateTime<Utc>,
    ) -> StoreResult<u64> {
        let result = sqlx::query(
            r#"
            UPDATE worker_launches
            SET archived_at = now(), updated_at = now()
            WHERE archived_at IS NULL
              AND stopped_at IS NOT NULL
              AND stopped_at < $1
              AND status IN ('stopped', 'failed')
            "#,
        )
        .bind(cutoff)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// Update a launch's status. A launch that is already `stopped`/`failed` is never moved
    /// back to a live status (a poll refresh can race a stop); in that case the current row
    /// is returned unchanged. `error` is only overwritten when a new value is provided, and
    /// `stopped_at` keeps its first value.
    #[allow(clippy::too_many_arguments)]
    pub async fn update_worker_launch(
        &self,
        org_id: Uuid,
        launch_id: Uuid,
        status: &str,
        runtime_id: Option<&str>,
        worker_id: Option<&str>,
        error: Option<&str>,
        started: bool,
        stopped: bool,
    ) -> StoreResult<WorkerLaunch> {
        let started_at = started.then(Utc::now);
        let stopped_at = stopped.then(Utc::now);
        let updated = sqlx::query_as::<_, WorkerLaunch>(
            r#"
            UPDATE worker_launches
            SET status = $3,
                runtime_id = COALESCE($4, runtime_id),
                worker_id = COALESCE($5, worker_id),
                error = COALESCE($6, error),
                started_at = COALESCE($7, started_at),
                stopped_at = COALESCE(stopped_at, $8),
                updated_at = now()
            WHERE org_id = $1 AND id = $2
              AND (status NOT IN ('stopped', 'failed') OR $3 IN ('stopped', 'failed'))
            RETURNING *
            "#,
        )
        .bind(org_id)
        .bind(launch_id)
        .bind(status)
        .bind(runtime_id)
        .bind(worker_id)
        .bind(error)
        .bind(started_at)
        .bind(stopped_at)
        .fetch_optional(&self.pool)
        .await?;
        match updated {
            Some(launch) => Ok(launch),
            // Guarded out (already terminal) or missing: return the current row / NotFound.
            None => self.get_worker_launch(org_id, launch_id).await,
        }
    }
}

impl Store {
    /// Delete a launch that has ended. A live launch keeps its row: without it the stop
    /// endpoint can no longer reach the container or Job, which would keep running with
    /// its worker token (#306).
    pub async fn delete_worker_launch(&self, org_id: Uuid, launch_id: Uuid) -> StoreResult<()> {
        let result = sqlx::query(
            "DELETE FROM worker_launches WHERE org_id = $1 AND id = $2 AND status IN ('stopped', 'failed')",
        )
        .bind(org_id)
        .bind(launch_id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() > 0 {
            return Ok(());
        }
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM worker_launches WHERE org_id = $1 AND id = $2)",
        )
        .bind(org_id)
        .bind(launch_id)
        .fetch_one(&self.pool)
        .await?;
        if exists {
            Err(StoreError::Conflict(
                "stop the worker launch before deleting it".into(),
            ))
        } else {
            Err(StoreError::NotFound("worker launch".into()))
        }
    }
}

impl Store {
    /// Claim a queued execution for a launch and bind it to the launch's worker token in a
    /// single statement. `None` when the execution is no longer queued.
    pub async fn claim_execution_for_launch(
        &self,
        execution_id: Uuid,
        worker_id: &str,
        worker_token_id: Uuid,
    ) -> StoreResult<Option<crate::models::ExecutionRecord>> {
        Ok(sqlx::query_as::<_, crate::models::ExecutionRecord>(
            r#"
            UPDATE executions
            SET status = 'dispatched', worker_id = $2, worker_token_id = $3,
                started_at = COALESCE(started_at, now()), updated_at = now()
            WHERE id = $1 AND status = 'queued'
            RETURNING *
            "#,
        )
        .bind(execution_id)
        .bind(worker_id)
        .bind(worker_token_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn archive_worker_launch(
        &self,
        org_id: Uuid,
        launch_id: Uuid,
    ) -> StoreResult<WorkerLaunch> {
        let row = sqlx::query_as::<_, WorkerLaunch>(
            r#"
            UPDATE worker_launches
            SET archived_at = COALESCE(archived_at, now()), updated_at = now()
            WHERE org_id = $1 AND id = $2 AND status IN ('stopped', 'failed')
            RETURNING *
            "#,
        )
        .bind(org_id)
        .bind(launch_id)
        .fetch_optional(&self.pool)
        .await?;
        if let Some(launch) = row {
            return Ok(launch);
        }
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM worker_launches WHERE org_id = $1 AND id = $2)",
        )
        .bind(org_id)
        .bind(launch_id)
        .fetch_one(&self.pool)
        .await?;
        if exists {
            Err(StoreError::Conflict(
                "stop the launch before archiving it".into(),
            ))
        } else {
            Err(StoreError::NotFound("worker launch".into()))
        }
    }
}

impl Store {
    /// Flag every live launch of a profile as needing a relaunch after a profile update.
    pub async fn mark_profile_launches_relaunch_required(
        &self,
        org_id: Uuid,
        profile_id: Uuid,
    ) -> StoreResult<u64> {
        let result = sqlx::query(
            r#"
            UPDATE worker_launches
            SET relaunch_required = true, updated_at = now()
            WHERE org_id = $1 AND profile_id = $2
              AND status IN ('pending', 'starting', 'running')
            "#,
        )
        .bind(org_id)
        .bind(profile_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }
}

impl Store {
    /// Launches currently in flight. Terminal/stopped launches are history and are hidden
    /// from the default Workers view.
    pub async fn list_active_worker_launches(
        &self,
        org_id: Uuid,
    ) -> StoreResult<Vec<WorkerLaunch>> {
        Ok(sqlx::query_as::<_, WorkerLaunch>(
            r#"
            SELECT * FROM worker_launches
            WHERE org_id = $1
              AND archived_at IS NULL
              AND status IN ('pending', 'starting', 'running')
            ORDER BY created_at DESC
            LIMIT 100
            "#,
        )
        .bind(org_id)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Installation-wide live launches for the background termination poller. Bounded so a
    /// large backlog cannot stall one tick; the next tick picks up the rest.
    pub async fn list_active_worker_launches_installation(
        &self,
        limit: i64,
    ) -> StoreResult<Vec<WorkerLaunch>> {
        Ok(sqlx::query_as::<_, WorkerLaunch>(
            r#"
            SELECT * FROM worker_launches
            WHERE archived_at IS NULL
              AND status IN ('pending', 'starting', 'running')
            ORDER BY created_at ASC
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Installation-wide count of workers actually in flight. Disabled/enabled profiles
    /// are configuration, not running workers, so they do not consume worker seats.
    pub async fn count_active_worker_launches_installation(&self) -> StoreResult<i64> {
        Ok(sqlx::query_scalar(
            r#"
            SELECT COUNT(*) FROM worker_launches
            WHERE archived_at IS NULL
              AND status IN ('pending', 'starting', 'running')
            "#,
        )
        .fetch_one(&self.pool)
        .await?)
    }
}
