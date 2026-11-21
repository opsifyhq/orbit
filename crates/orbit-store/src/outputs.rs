//! Execution result outputs and their durable delivery queue.

use chrono::{Duration, Utc};
use uuid::Uuid;

use crate::models::{ExecutionRecord, Output, OutputDelivery, OutputDeliveryJob, TeamRun};
use crate::{Store, StoreError, StoreResult};

/// Delivery attempts (including interrupted ones) before a retryable delivery is `dead`.
pub const MAX_DELIVERY_ATTEMPTS: i32 = 5;

/// Result of one delivery attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryOutcome {
    Succeeded,
    /// Transient failure: retry with backoff until [`MAX_DELIVERY_ATTEMPTS`], then `dead`.
    Retry,
    /// Permanent failure: never retried.
    Failed,
    /// Nothing to deliver (output deleted or disabled).
    Skipped,
}

impl Store {
    pub async fn list_outputs(&self, org_id: Uuid) -> StoreResult<Vec<Output>> {
        Ok(
            sqlx::query_as::<_, Output>(
                "SELECT * FROM outputs WHERE org_id = $1 ORDER BY name ASC",
            )
            .bind(org_id)
            .fetch_all(&self.pool)
            .await?,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_output(
        &self,
        org_id: Uuid,
        name: &str,
        kind: &str,
        agent_id: Option<Uuid>,
        team_id: Option<Uuid>,
        events: &[String],
        secret_ref: &str,
        template: Option<&str>,
        enabled: bool,
    ) -> StoreResult<Output> {
        sqlx::query_as::<_, Output>(
            r#"
            INSERT INTO outputs (id, org_id, name, kind, agent_id, team_id, events, secret_ref, template, enabled)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            RETURNING *
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(org_id)
        .bind(name)
        .bind(kind)
        .bind(agent_id)
        .bind(team_id)
        .bind(events)
        .bind(secret_ref)
        .bind(template)
        .bind(enabled)
        .fetch_one(&self.pool)
        .await
        .map_err(|error| {
            if error
                .as_database_error()
                .map(|db| db.is_unique_violation())
                .unwrap_or(false)
            {
                StoreError::Conflict(format!("output '{name}' already exists"))
            } else {
                error.into()
            }
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_output(
        &self,
        org_id: Uuid,
        id: Uuid,
        name: &str,
        kind: &str,
        agent_id: Option<Uuid>,
        team_id: Option<Uuid>,
        events: &[String],
        secret_ref: Option<&str>,
        template: Option<&str>,
        enabled: bool,
    ) -> StoreResult<Output> {
        sqlx::query_as::<_, Output>(
            r#"
            UPDATE outputs
            SET name = $3, kind = $4, agent_id = $5, team_id = $6, events = $7,
                secret_ref = COALESCE($8, secret_ref), template = $9, enabled = $10,
                enqueue_from = CASE WHEN $10 AND NOT enabled THEN now() ELSE enqueue_from END,
                enqueue_cursor = CASE WHEN $10 AND NOT enabled THEN now() ELSE enqueue_cursor END,
                updated_at = now()
            WHERE org_id = $1 AND id = $2
            RETURNING *
            "#,
        )
        .bind(org_id)
        .bind(id)
        .bind(name)
        .bind(kind)
        .bind(agent_id)
        .bind(team_id)
        .bind(events)
        .bind(secret_ref)
        .bind(template)
        .bind(enabled)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::NotFound("output".into()))
    }

    pub async fn delete_output(&self, org_id: Uuid, id: Uuid) -> StoreResult<()> {
        let result = sqlx::query("DELETE FROM outputs WHERE org_id = $1 AND id = $2")
            .bind(org_id)
            .bind(id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(StoreError::NotFound("output".into()));
        }
        Ok(())
    }

    pub async fn get_output(&self, org_id: Uuid, id: Uuid) -> StoreResult<Output> {
        sqlx::query_as::<_, Output>("SELECT * FROM outputs WHERE org_id = $1 AND id = $2")
            .bind(org_id)
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| StoreError::NotFound("output".into()))
    }

    /// Enqueue deliveries for terminal executions. Idempotent per output/execution.
    ///
    /// Not time-windowed: each output scans from its own high-water mark
    /// (`enqueue_cursor`, minus a safety overlap for late commits), never earlier than
    /// `enqueue_from` (creation / last re-enable), so an outage of any length still
    /// delivers each finished run once and a new output does not back-fill old runs.
    pub async fn enqueue_output_deliveries(&self) -> StoreResult<u64> {
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            r#"
            INSERT INTO output_deliveries (id, org_id, output_id, execution_id)
            SELECT gen_random_uuid(), o.org_id, o.id, e.id
            FROM outputs o
            JOIN executions e
              ON e.org_id = o.org_id
             AND (o.agent_id IS NULL OR o.agent_id = e.agent_id)
             AND e.status = ANY(o.events)
             AND e.updated_at >= GREATEST(o.enqueue_from, o.enqueue_cursor - interval '5 minutes')
             AND COALESCE(e.finished_at, e.updated_at) >= o.enqueue_from
            WHERE o.enabled AND o.team_id IS NULL
            ON CONFLICT (output_id, execution_id) DO NOTHING
            "#,
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE outputs SET enqueue_cursor = now() WHERE enabled AND team_id IS NULL AND enqueue_cursor < now()",
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(result.rows_affected())
    }

    /// Enqueue team-run outputs. Same cursor semantics as [`Self::enqueue_output_deliveries`].
    pub async fn enqueue_team_output_deliveries(&self) -> StoreResult<u64> {
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            r#"
            INSERT INTO output_deliveries (id, org_id, output_id, team_run_id)
            SELECT gen_random_uuid(), o.org_id, o.id, t.id
            FROM outputs o
            JOIN team_runs t
              ON t.org_id = o.org_id
             AND o.team_id = t.team_id
             AND t.status = ANY(o.events)
             AND t.updated_at >= GREATEST(o.enqueue_from, o.enqueue_cursor - interval '5 minutes')
             AND COALESCE(t.finished_at, t.updated_at) >= o.enqueue_from
            WHERE o.enabled AND o.team_id IS NOT NULL
            ON CONFLICT (output_id, team_run_id) WHERE team_run_id IS NOT NULL DO NOTHING
            "#,
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE outputs SET enqueue_cursor = now() WHERE enabled AND team_id IS NOT NULL AND enqueue_cursor < now()",
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(result.rows_affected())
    }

    /// Atomically claim up to `limit` due deliveries (pending and due, or `sending` with an
    /// expired lease). Claimed rows are `sending` with a fresh lease and claim token and
    /// `attempts` already incremented, so a crash mid-send still counts as an attempt.
    /// Rows whose lease expired after the final attempt are marked `dead`.
    pub async fn claim_output_deliveries(
        &self,
        limit: i64,
        lease: Duration,
    ) -> StoreResult<Vec<OutputDeliveryJob>> {
        sqlx::query(
            r#"
            UPDATE output_deliveries
            SET status = 'dead', last_error = 'lease expired after final attempt',
                claim_token = NULL, lease_until = NULL, updated_at = now()
            WHERE status = 'sending' AND lease_until < now() AND attempts >= $1
            "#,
        )
        .bind(MAX_DELIVERY_ATTEMPTS)
        .execute(&self.pool)
        .await?;
        Ok(sqlx::query_as::<_, OutputDeliveryJob>(
            r#"
            UPDATE output_deliveries
            SET status = 'sending', attempts = attempts + 1,
                lease_until = now() + make_interval(secs => $2),
                claim_token = gen_random_uuid(), updated_at = now()
            WHERE id IN (
                SELECT id FROM output_deliveries
                WHERE (status = 'pending' AND next_attempt_at <= now())
                   OR (status = 'sending' AND lease_until < now())
                ORDER BY next_attempt_at ASC
                LIMIT $1
                FOR UPDATE SKIP LOCKED
            )
            RETURNING id, org_id, output_id, execution_id, team_run_id, attempts, claim_token
            "#,
        )
        .bind(limit)
        .bind(lease.num_milliseconds() as f64 / 1000.0)
        .fetch_all(&self.pool)
        .await?)
    }

    /// The most recent deliveries of one output, newest first, with the task (and agent)
    /// of the run each one reports on.
    pub async fn list_output_deliveries(
        &self,
        org_id: Uuid,
        output_id: Uuid,
        limit: i64,
    ) -> StoreResult<Vec<OutputDelivery>> {
        Ok(sqlx::query_as::<_, OutputDelivery>(
            r#"
            SELECT d.id, d.output_id, d.execution_id, d.team_run_id, d.status, d.attempts,
                   d.response_status, d.last_error, d.next_attempt_at, d.created_at, d.updated_at,
                   COALESCE(e.input->>'task', t.input->>'task') AS task, e.agent_name
            FROM output_deliveries d
            LEFT JOIN executions e ON e.id = d.execution_id
            LEFT JOIN team_runs t ON t.id = d.team_run_id
            WHERE d.org_id = $1 AND d.output_id = $2
            ORDER BY d.created_at DESC, d.id DESC
            LIMIT $3
            "#,
        )
        .bind(org_id)
        .bind(output_id)
        .bind(limit.clamp(1, 200))
        .fetch_all(&self.pool)
        .await?)
    }

    /// The newest delivery of every output in the organisation that has one.
    pub async fn latest_output_deliveries(&self, org_id: Uuid) -> StoreResult<Vec<OutputDelivery>> {
        Ok(sqlx::query_as::<_, OutputDelivery>(
            r#"
            SELECT DISTINCT ON (d.output_id) d.id, d.output_id, d.execution_id, d.team_run_id, d.status, d.attempts,
                   d.response_status, d.last_error, d.next_attempt_at, d.created_at, d.updated_at,
                   COALESCE(e.input->>'task', t.input->>'task') AS task, e.agent_name
            FROM output_deliveries d
            LEFT JOIN executions e ON e.id = d.execution_id
            LEFT JOIN team_runs t ON t.id = d.team_run_id
            WHERE d.org_id = $1
            ORDER BY d.output_id, d.created_at DESC, d.id DESC
            "#,
        )
        .bind(org_id)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn get_team_run_any_org(&self, id: Uuid) -> StoreResult<TeamRun> {
        sqlx::query_as::<_, TeamRun>("SELECT * FROM team_runs WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| StoreError::NotFound("team run".into()))
    }

    pub async fn get_execution_by_id_any_org(&self, id: Uuid) -> StoreResult<ExecutionRecord> {
        sqlx::query_as::<_, ExecutionRecord>("SELECT * FROM executions WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| StoreError::NotFound("execution".into()))
    }

    /// Record the result of a claimed delivery. Conditional on the caller still holding the
    /// claim (`status = 'sending'` and matching token); returns `false` if the claim was lost
    /// (lease expired and reclaimed), in which case nothing is written.
    pub async fn finish_output_delivery(
        &self,
        id: Uuid,
        claim_token: Uuid,
        outcome: DeliveryOutcome,
        response_status: Option<i32>,
        error: Option<&str>,
        attempts: i32,
    ) -> StoreResult<bool> {
        let (status, next_attempt) = match outcome {
            DeliveryOutcome::Succeeded => ("succeeded", Utc::now()),
            DeliveryOutcome::Failed => ("failed", Utc::now()),
            DeliveryOutcome::Skipped => ("skipped", Utc::now()),
            DeliveryOutcome::Retry if attempts >= MAX_DELIVERY_ATTEMPTS => ("dead", Utc::now()),
            DeliveryOutcome::Retry => (
                "pending",
                Utc::now() + Duration::seconds(30 * i64::from(attempts.max(1))),
            ),
        };
        let result = sqlx::query(
            r#"
            UPDATE output_deliveries
            SET status = $3, next_attempt_at = $4, response_status = $5, last_error = $6,
                claim_token = NULL, lease_until = NULL, updated_at = now()
            WHERE id = $1 AND status = 'sending' AND claim_token = $2
            "#,
        )
        .bind(id)
        .bind(claim_token)
        .bind(status)
        .bind(next_attempt)
        .bind(response_status)
        .bind(error)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }
}
