use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

use orbit_domain::{new_id, ExecutionStatus, HarnessKind, LogLevel, TriggerSource};

use crate::models::{ExecutionEventRecord, ExecutionRecord};
use crate::{decode_cursor, page_of, Store, StoreError, StoreResult};

/// Expire an execution's outstanding approvals in the same transaction that ends the run.
/// A human must never be able to authorise work for a run that is already over.
pub(crate) async fn expire_pending_approvals(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    execution_id: Uuid,
) -> StoreResult<()> {
    sqlx::query(
        "UPDATE approvals SET status = 'expired', decided_at = now() \
         WHERE execution_id = $1 AND status = 'pending'",
    )
    .bind(execution_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Optional, presentation-driven narrowing of the event list. All fields are ANDed;
/// text fields are literal case-insensitive substrings (no SQL wildcard injection).
#[derive(Debug, Clone, Default)]
pub struct EventFilter {
    pub kinds: Vec<String>,
    pub tool: Option<String>,
    pub q: Option<String>,
}

pub struct NewExecution<'a> {
    pub org_id: Uuid,
    pub agent_id: Uuid,
    pub agent_version_id: Uuid,
    pub agent_name: &'a str,
    pub harness_kind: HarnessKind,
    pub trigger_source: TriggerSource,
    pub placement: &'a str,
    pub trigger_id: Option<Uuid>,
    pub input: Value,
    /// Resume an existing harness session when supplied.
    pub session_ref: Option<&'a str>,
    pub skills: &'a [String],
    pub timeout_seconds: u64,
    pub requested_by: Option<Uuid>,
    pub idempotency_key: Option<&'a str>,
    pub team_run_id: Option<Uuid>,
    pub team_step_index: Option<i32>,
}

impl Store {
    /// Insert a queued execution. When `idempotency_key` is supplied, a duplicate request
    /// returns the previously created execution instead of creating another one.
    pub async fn create_execution(&self, new: NewExecution<'_>) -> StoreResult<ExecutionRecord> {
        if let Some(key) = new.idempotency_key {
            if let Some(existing) = self
                .get_execution_by_idempotency_key(new.org_id, key)
                .await?
            {
                return Ok(existing);
            }
        }

        let id = new_id();
        let inserted = sqlx::query_as::<_, ExecutionRecord>(
            r#"
            INSERT INTO executions (id, org_id, agent_id, agent_version_id, agent_name, harness_kind,
                                    trigger_id, trigger_source, placement, status, input, skills,
                                    timeout_seconds, idempotency_key, requested_by,
                                    team_run_id, team_step_index, session_ref)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'queued', $10, $11, $12, $13, $14, $15, $16, $17)
            ON CONFLICT (org_id, idempotency_key) WHERE idempotency_key IS NOT NULL DO NOTHING
            RETURNING *
            "#,
        )
        .bind(id)
        .bind(new.org_id)
        .bind(new.agent_id)
        .bind(new.agent_version_id)
        .bind(new.agent_name)
        .bind(new.harness_kind.as_str())
        .bind(new.trigger_id)
        .bind(new.trigger_source.as_str())
        .bind(new.placement)
        .bind(&new.input)
        .bind(new.skills)
        .bind(new.timeout_seconds as i32)
        .bind(new.idempotency_key)
        .bind(new.requested_by)
        .bind(new.team_run_id)
        .bind(new.team_step_index)
        .bind(new.session_ref)
        .fetch_optional(&self.pool)
        .await?;

        match inserted {
            Some(record) => {
                self.observe_created(&record);
                Ok(record)
            }
            None => {
                let key = new.idempotency_key.ok_or_else(|| {
                    StoreError::Internal("insert conflict without idempotency key".into())
                })?;
                self.get_execution_by_idempotency_key(new.org_id, key)
                    .await?
                    .ok_or_else(|| {
                        StoreError::Internal("idempotent execution lookup failed".into())
                    })
            }
        }
    }

    pub async fn get_execution(
        &self,
        org_id: Uuid,
        execution_id: Uuid,
    ) -> StoreResult<ExecutionRecord> {
        sqlx::query_as::<_, ExecutionRecord>(
            "SELECT * FROM executions WHERE org_id = $1 AND id = $2",
        )
        .bind(org_id)
        .bind(execution_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::NotFound("execution".into()))
    }

    pub async fn get_execution_by_idempotency_key(
        &self,
        org_id: Uuid,
        key: &str,
    ) -> StoreResult<Option<ExecutionRecord>> {
        Ok(sqlx::query_as::<_, ExecutionRecord>(
            "SELECT * FROM executions WHERE org_id = $1 AND idempotency_key = $2",
        )
        .bind(org_id)
        .bind(key)
        .fetch_optional(&self.pool)
        .await?)
    }

    /// Keyset-paginated execution list with optional filters. Visibility is applied in SQL
    /// via the owning agent (`access = None` means org-wide/internal). The returned total
    /// uses the same predicate, so it is the visible total for the filter.
    /// `status` is one status or a comma-separated set (`failed,timed_out`).
    #[allow(clippy::type_complexity, clippy::too_many_arguments)]
    pub async fn list_executions(
        &self,
        org_id: Uuid,
        limit: i64,
        cursor: Option<&str>,
        status: Option<&str>,
        agent_id: Option<Uuid>,
        archived: Option<bool>,
        access: Option<&crate::access::ResourceAccess>,
    ) -> StoreResult<(Vec<ExecutionRecord>, Option<String>, i64)> {
        let (cursor_ts, cursor_id) = match cursor {
            Some(raw) => match decode_cursor(raw)? {
                Some((ts, id)) => (Some(ts), Some(id)),
                None => (None, None),
            },
            None => (None, None),
        };
        let (privileged, user_id, team_ids, isolation) = crate::access::binds(access);
        let rows = sqlx::query_as::<_, ExecutionRecord>(
            r#"
            SELECT e.* FROM executions e
            JOIN agents a ON a.id = e.agent_id
            WHERE e.org_id = $1
              AND ($2::text IS NULL OR e.status = ANY(string_to_array($2, ',')))
              AND ($3::uuid IS NULL OR e.agent_id = $3)
              AND ($4::timestamptz IS NULL OR (e.created_at, e.id) < ($4::timestamptz, $5::uuid))
              AND ($6::boolean IS NULL OR ($6 = FALSE AND e.archived_at IS NULL) OR ($6 = TRUE AND e.archived_at IS NOT NULL))
              AND ($8::bool OR a.owner_user_id = $9
                   OR CASE WHEN $11::bool AND a.owner_team_id IS NOT NULL
                        THEN a.owner_team_id = ANY($10)
                        ELSE a.visibility = 'org'
                             OR (a.visibility = 'team' AND a.owner_team_id = ANY($10)) END)
            ORDER BY e.created_at DESC, e.id DESC
            LIMIT $7
            "#,
        )
        .bind(org_id)
        .bind(status)
        .bind(agent_id)
        .bind(cursor_ts)
        .bind(cursor_id)
        .bind(archived)
        .bind(limit + 1)
        .bind(privileged)
        .bind(user_id)
        .bind(&team_ids)
        .bind(isolation)
        .fetch_all(&self.pool)
        .await?;

        let total: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*) FROM executions e
            JOIN agents a ON a.id = e.agent_id
            WHERE e.org_id = $1
              AND ($2::text IS NULL OR e.status = ANY(string_to_array($2, ',')))
              AND ($3::uuid IS NULL OR e.agent_id = $3)
              AND ($4::boolean IS NULL OR ($4 = FALSE AND e.archived_at IS NULL) OR ($4 = TRUE AND e.archived_at IS NOT NULL))
              AND ($5::bool OR a.owner_user_id = $6
                   OR CASE WHEN $8::bool AND a.owner_team_id IS NOT NULL
                        THEN a.owner_team_id = ANY($7)
                        ELSE a.visibility = 'org'
                             OR (a.visibility = 'team' AND a.owner_team_id = ANY($7)) END)
            "#,
        )
        .bind(org_id)
        .bind(status)
        .bind(agent_id)
        .bind(archived)
        .bind(privileged)
        .bind(user_id)
        .bind(&team_ids)
        .bind(isolation)
        .fetch_one(&self.pool)
        .await?;

        let (rows, next) = page_of(rows, limit, |e| (e.created_at, e.id));
        Ok((rows, next, total))
    }

    /// Archive terminal executions whose last activity predates `cutoff`.
    pub async fn archive_executions_before(
        &self,
        cutoff: chrono::DateTime<Utc>,
    ) -> StoreResult<u64> {
        let result = sqlx::query(
            r#"
            UPDATE executions
            SET archived_at = now(), updated_at = now()
            WHERE archived_at IS NULL
              AND finished_at IS NOT NULL
              AND finished_at < $1
              AND status IN ('succeeded', 'failed', 'timed_out', 'cancelled', 'rejected')
            "#,
        )
        .bind(cutoff)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// Atomically claim a queued execution. Returns `None` if another worker won.
    pub async fn claim_execution(
        &self,
        execution_id: Uuid,
        worker_id: &str,
    ) -> StoreResult<Option<ExecutionRecord>> {
        Ok(sqlx::query_as::<_, ExecutionRecord>(
            r#"
            UPDATE executions
            SET status = 'dispatched', worker_id = $2, started_at = COALESCE(started_at, now()), updated_at = now()
            WHERE id = $1 AND status = 'queued'
            RETURNING *
            "#,
        )
        .bind(execution_id)
        .bind(worker_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn mark_execution_running(
        &self,
        execution_id: Uuid,
        session_ref: Option<&str>,
    ) -> StoreResult<Option<ExecutionRecord>> {
        let updated = sqlx::query_as::<_, ExecutionRecord>(
            r#"
            UPDATE executions
            SET status = 'running',
                session_ref = COALESCE($2, session_ref),
                started_at = COALESCE(started_at, now()),
                running_started_at = COALESCE(running_started_at, now()),
                updated_at = now()
            WHERE id = $1 AND status IN ('dispatched', 'awaiting_approval')
            RETURNING *
            "#,
        )
        .bind(execution_id)
        .bind(session_ref)
        .fetch_optional(&self.pool)
        .await?;
        // `now()` is the statement's transaction time, so the two timestamps are equal
        // only on the first start; a resume after an approval keeps the older one.
        if let Some(record) = &updated {
            if record.running_started_at == Some(record.updated_at) {
                self.observe_started(record);
            }
        }
        Ok(updated)
    }

    /// Move an execution to a terminal state. Terminal states are never overwritten,
    /// cancellation always wins over a late harness result, and only transitions allowed by
    /// `ExecutionStatus::can_transition_to` are accepted (`None` otherwise).
    pub async fn finish_execution(
        &self,
        execution_id: Uuid,
        status: ExecutionStatus,
        output: Option<&Value>,
        error: Option<&Value>,
        usage: Option<&Value>,
        session_ref: Option<&str>,
    ) -> StoreResult<Option<ExecutionRecord>> {
        if !status.is_terminal() {
            return Err(StoreError::Invalid(
                "finish_execution requires a terminal status".into(),
            ));
        }
        // The source states come from the domain transition table, so e.g.
        // queued -> succeeded is rejected (returns None) instead of fabricating a result.
        let from: Vec<String> = ExecutionStatus::sources_for(status)
            .iter()
            .map(|s| s.as_str().to_string())
            .collect();
        let mut tx = self.pool.begin().await?;
        let finished = sqlx::query_as::<_, ExecutionRecord>(
            r#"
            UPDATE executions
            SET status = $2,
                output = COALESCE($3, output),
                error = COALESCE($4, error),
                usage = COALESCE($5, usage),
                session_ref = COALESCE($6, session_ref),
                finished_at = now(),
                updated_at = now(),
                execution_lease_until = NULL
            WHERE id = $1
              AND status = ANY($7)
            RETURNING *
            "#,
        )
        .bind(execution_id)
        .bind(status.as_str())
        .bind(output)
        .bind(error)
        .bind(usage)
        .bind(session_ref)
        .bind(&from)
        .fetch_optional(&mut *tx)
        .await?;
        if finished.is_some() {
            expire_pending_approvals(&mut tx, execution_id).await?;
        }
        tx.commit().await?;
        if let Some(record) = &finished {
            self.observe_finished(record, crate::observe::FinishCause::Reported);
        }
        Ok(finished)
    }

    /// Request cancellation. Queued executions are cancelled immediately; running ones
    /// are flagged for the worker to abort the harness.
    pub async fn request_execution_cancel(
        &self,
        org_id: Uuid,
        execution_id: Uuid,
    ) -> StoreResult<ExecutionRecord> {
        let mut tx = self.pool.begin().await?;
        let cancelled = sqlx::query_as::<_, ExecutionRecord>(
            r#"
            UPDATE executions
            SET status = 'cancelled', finished_at = now(), cancel_requested = true, updated_at = now()
            WHERE org_id = $1 AND id = $2 AND status = 'queued'
            RETURNING *
            "#,
        )
        .bind(org_id)
        .bind(execution_id)
        .fetch_optional(&mut *tx)
        .await?;

        if let Some(record) = cancelled {
            expire_pending_approvals(&mut tx, execution_id).await?;
            tx.commit().await?;
            self.observe_finished(&record, crate::observe::FinishCause::CancelledWhileQueued);
            return Ok(record);
        }

        let flagged = sqlx::query_as::<_, ExecutionRecord>(
            r#"
            UPDATE executions
            SET cancel_requested = true, updated_at = now()
            WHERE org_id = $1 AND id = $2
              AND status IN ('dispatched', 'running', 'awaiting_approval')
            RETURNING *
            "#,
        )
        .bind(org_id)
        .bind(execution_id)
        .fetch_optional(&mut *tx)
        .await?;

        if let Some(record) = flagged {
            // The run is not terminal yet, but a cancel means nobody may decide its
            // approvals from here on; expire them with the cancel flag in one commit.
            expire_pending_approvals(&mut tx, execution_id).await?;
            tx.commit().await?;
            return Ok(record);
        }
        tx.commit().await?;

        let existing = self.get_execution(org_id, execution_id).await?;
        if existing.status == "cancelled" {
            Ok(existing)
        } else {
            Err(StoreError::Invalid(format!(
                "execution in status '{}' cannot be cancelled",
                existing.status
            )))
        }
    }

    pub async fn is_cancel_requested(&self, execution_id: Uuid) -> StoreResult<bool> {
        let flag: Option<bool> =
            sqlx::query_scalar("SELECT cancel_requested FROM executions WHERE id = $1")
                .bind(execution_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(flag.unwrap_or(false))
    }

    pub async fn append_execution_event(
        &self,
        org_id: Uuid,
        execution_id: Uuid,
        kind: &str,
        level: LogLevel,
        message: &str,
        data: &Value,
    ) -> StoreResult<ExecutionEventRecord> {
        // seq is MAX+1 per execution. Under READ COMMITTED concurrent appends would read the
        // same MAX and collide on UNIQUE (execution_id, seq), so serialize allocation per
        // execution with a transaction-scoped advisory lock. An advisory lock (rather than
        // `SELECT ... FOR UPDATE` on the execution row) never blocks claim/finish/cancel
        // updates, and it is released automatically on commit/rollback.
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "SELECT pg_advisory_xact_lock(hashtextextended('execution_events:' || $1::text, 0))",
        )
        .bind(execution_id)
        .execute(&mut *tx)
        .await?;
        let record = sqlx::query_as::<_, ExecutionEventRecord>(
            r#"
            WITH next_seq AS (
                SELECT COALESCE(MAX(seq), 0) + 1 AS seq
                FROM execution_events
                WHERE execution_id = $1
            )
            INSERT INTO execution_events (org_id, execution_id, seq, kind, level, message, data)
            SELECT $2, $1, next_seq.seq, $3, $4, $5, $6 FROM next_seq
            RETURNING *
            "#,
        )
        .bind(execution_id)
        .bind(org_id)
        .bind(kind)
        .bind(level.as_str())
        .bind(message)
        .bind(data)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(record)
    }

    pub async fn list_execution_events(
        &self,
        org_id: Uuid,
        execution_id: Uuid,
        after_seq: i64,
        limit: i64,
    ) -> StoreResult<Vec<ExecutionEventRecord>> {
        self.list_execution_events_filtered(
            org_id,
            execution_id,
            after_seq,
            limit,
            &EventFilter::default(),
        )
        .await
    }

    /// Filter in SQL, before `limit`, so a filtered timeline is not restricted to
    /// whatever page happened to be loaded. Event rows and their order are unchanged.
    pub async fn list_execution_events_filtered(
        &self,
        org_id: Uuid,
        execution_id: Uuid,
        after_seq: i64,
        limit: i64,
        filter: &EventFilter,
    ) -> StoreResult<Vec<ExecutionEventRecord>> {
        let mut query = sqlx::QueryBuilder::new("SELECT * FROM execution_events WHERE org_id = ");
        query
            .push_bind(org_id)
            .push(" AND execution_id = ")
            .push_bind(execution_id)
            .push(" AND seq > ")
            .push_bind(after_seq);

        if !filter.kinds.is_empty() {
            query
                .push(" AND kind = ANY(")
                .push_bind(&filter.kinds)
                .push(")");
        }
        if let Some(tool) = &filter.tool {
            query.push(" AND ");
            push_contains(
                &mut query,
                tool,
                &[
                    "coalesce(data->>'tool', '')",
                    "coalesce(data->>'capability', '')",
                    "coalesce(data->>'connector', '')",
                    "coalesce(data->>'connectorName', '')",
                    "coalesce(data->>'connection', '')",
                    "coalesce(data->>'connectionName', '')",
                    "coalesce(data->'arguments'->>'connector', '')",
                    "coalesce(data->'arguments'->>'connectionName', '')",
                ],
            );
        }
        if let Some(term) = &filter.q {
            query.push(" AND ");
            push_contains(
                &mut query,
                term,
                &[
                    "message",
                    "coalesce(data->>'tool', '')",
                    "coalesce(data->>'capability', '')",
                    "coalesce(data->>'connector', '')",
                    "coalesce(data->>'connectionName', '')",
                ],
            );
        }
        query.push(" ORDER BY seq ASC LIMIT ").push_bind(limit);
        Ok(query.build_query_as().fetch_all(&self.pool).await?)
    }

    pub async fn count_active_executions_for_agent(
        &self,
        org_id: Uuid,
        agent_id: Uuid,
    ) -> StoreResult<i64> {
        Ok(sqlx::query_scalar(
            r#"
            SELECT COUNT(*) FROM executions
            WHERE org_id = $1 AND agent_id = $2
              AND status IN ('queued', 'dispatched', 'running', 'awaiting_approval')
            "#,
        )
        .bind(org_id)
        .bind(agent_id)
        .fetch_one(&self.pool)
        .await?)
    }

    pub async fn count_executions_since(
        &self,
        org_id: Uuid,
        agent_id: Uuid,
        since: DateTime<Utc>,
    ) -> StoreResult<i64> {
        Ok(sqlx::query_scalar(
            "SELECT COUNT(*) FROM executions WHERE org_id = $1 AND agent_id = $2 AND created_at >= $3",
        )
        .bind(org_id)
        .bind(agent_id)
        .bind(since)
        .fetch_one(&self.pool)
        .await?)
    }
}

impl Store {
    /// Claim the oldest queued execution atomically. `profile_team_id` restricts the claim to
    /// agents visible to that access team; `None` applies no visibility filter (tokens that
    /// are not bound to a worker profile). Profile-bound workers use
    /// [`Self::claim_next_queued_for_profile`], which also covers org-level profiles.
    #[allow(clippy::too_many_arguments)]
    pub async fn claim_next_queued(
        &self,
        org_id: Uuid,
        worker_id: &str,
        harnesses: &[String],
        worker_target: &str,
        agent_ids: &[Uuid],
        profile_team_id: Option<Uuid>,
        worker_token_id: Option<Uuid>,
    ) -> StoreResult<Option<ExecutionRecord>> {
        self.claim_next_queued_inner(
            org_id,
            worker_id,
            harnesses,
            worker_target,
            agent_ids,
            profile_team_id.is_some(),
            profile_team_id,
            worker_token_id,
        )
        .await
    }

    /// Claim for a worker bound to a profile. Mirrors the visibility rule the worker API
    /// enforces afterwards (`ensure_execution`): org-visible agents, plus the profile's own
    /// team's agents when the profile has an owner team. An org-level profile (no owner
    /// team) therefore never claims team-private executions it would then be refused.
    #[allow(clippy::too_many_arguments)]
    pub async fn claim_next_queued_for_profile(
        &self,
        org_id: Uuid,
        worker_id: &str,
        harnesses: &[String],
        worker_target: &str,
        agent_ids: &[Uuid],
        profile_owner_team_id: Option<Uuid>,
        worker_token_id: Option<Uuid>,
    ) -> StoreResult<Option<ExecutionRecord>> {
        self.claim_next_queued_inner(
            org_id,
            worker_id,
            harnesses,
            worker_target,
            agent_ids,
            true,
            profile_owner_team_id,
            worker_token_id,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn claim_next_queued_inner(
        &self,
        org_id: Uuid,
        worker_id: &str,
        harnesses: &[String],
        worker_target: &str,
        agent_ids: &[Uuid],
        visibility_scoped: bool,
        team_id: Option<Uuid>,
        worker_token_id: Option<Uuid>,
    ) -> StoreResult<Option<ExecutionRecord>> {
        let team_filter = match (visibility_scoped, team_id.is_some()) {
            (true, true) => {
                r#"
                  AND EXISTS (
                      SELECT 1 FROM agents a
                      WHERE a.id = executions.agent_id
                        AND (a.visibility = 'org' OR a.owner_team_id = $7)
                  )
            "#
            }
            (true, false) => {
                r#"
                  AND EXISTS (
                      SELECT 1 FROM agents a
                      WHERE a.id = executions.agent_id AND a.visibility = 'org'
                  )
            "#
            }
            (false, _) => "",
        };
        let sql = format!(
            r#"
            UPDATE executions
            SET status = 'dispatched', worker_id = $2, worker_token_id = $6,
                started_at = COALESCE(started_at, now()), updated_at = now()
            WHERE id = (
                SELECT id FROM executions
                WHERE org_id = $1
                  AND status = 'queued'
                  AND harness_kind = ANY($3)
                  AND (placement = 'auto' OR placement = $4)
                  AND (cardinality($5::uuid[]) = 0 OR agent_id = ANY($5))
                  {team_filter}
                ORDER BY created_at ASC
                LIMIT 1
                FOR UPDATE SKIP LOCKED
            )
            RETURNING *
            "#
        );
        // Safe: `team_filter` is one of three static strings; all values are bound.
        let mut query = sqlx::query_as::<_, ExecutionRecord>(sqlx::AssertSqlSafe(sql))
            .bind(org_id)
            .bind(worker_id)
            .bind(harnesses)
            .bind(worker_target)
            .bind(agent_ids)
            .bind(worker_token_id);
        if visibility_scoped {
            if let Some(team_id) = team_id {
                query = query.bind(team_id);
            }
        }
        Ok(query.fetch_optional(&self.pool).await?)
    }
}

impl Store {
    pub async fn get_execution_by_id(&self, execution_id: Uuid) -> StoreResult<ExecutionRecord> {
        sqlx::query_as::<_, ExecutionRecord>("SELECT * FROM executions WHERE id = $1")
            .bind(execution_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| StoreError::NotFound("execution".into()))
    }

    /// Record which worker token owns a claimed execution (single-execution launches,
    /// where the API claims on behalf of the launched worker).
    pub async fn bind_execution_worker_token(
        &self,
        execution_id: Uuid,
        worker_token_id: Uuid,
    ) -> StoreResult<()> {
        sqlx::query("UPDATE executions SET worker_token_id = $2 WHERE id = $1")
            .bind(execution_id)
            .bind(worker_token_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Token that claimed the execution (`None` for unclaimed or pre-binding legacy rows).
    pub async fn execution_worker_token(&self, execution_id: Uuid) -> StoreResult<Option<Uuid>> {
        Ok(
            sqlx::query_scalar("SELECT worker_token_id FROM executions WHERE id = $1")
                .bind(execution_id)
                .fetch_optional(&self.pool)
                .await?
                .flatten(),
        )
    }

    /// Bind an already-dispatched execution to a specific worker (used by the
    /// per-execution container dispatcher and single-execution worker mode).
    pub async fn take_execution_for_worker(
        &self,
        execution_id: Uuid,
        worker_id: &str,
    ) -> StoreResult<Option<ExecutionRecord>> {
        Ok(sqlx::query_as::<_, ExecutionRecord>(
            r#"
            UPDATE executions
            SET worker_id = $2, updated_at = now()
            WHERE id = $1 AND status IN ('dispatched', 'running')
            RETURNING *
            "#,
        )
        .bind(execution_id)
        .bind(worker_id)
        .fetch_optional(&self.pool)
        .await?)
    }
}

impl Store {
    /// Executions currently running on a worker. Used to fail them when the worker
    /// container is OOM-killed and the process cannot report its own failure.
    pub async fn running_executions_for_worker(
        &self,
        org_id: Uuid,
        worker_id: &str,
    ) -> StoreResult<Vec<ExecutionRecord>> {
        Ok(sqlx::query_as::<_, ExecutionRecord>(
            r#"
            SELECT * FROM executions
            WHERE org_id = $1 AND worker_id = $2 AND status = 'running'
            ORDER BY created_at ASC
            "#,
        )
        .bind(org_id)
        .bind(worker_id)
        .fetch_all(&self.pool)
        .await?)
    }
}

impl Store {
    /// Installation-wide daily execution count used for the license limit.
    pub async fn count_all_executions_since_installation(
        &self,
        since: DateTime<Utc>,
    ) -> StoreResult<i64> {
        Ok(
            sqlx::query_scalar("SELECT COUNT(*) FROM executions WHERE created_at >= $1")
                .bind(since)
                .fetch_one(&self.pool)
                .await?,
        )
    }
}

impl Store {
    pub async fn has_active_executions_for_worker(
        &self,
        org_id: Uuid,
        worker_id: &str,
    ) -> StoreResult<bool> {
        Ok(sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS(
                SELECT 1 FROM executions
                WHERE org_id = $1 AND worker_id = $2
                  AND status IN ('running', 'awaiting_approval')
            )
            "#,
        )
        .bind(org_id)
        .bind(worker_id)
        .fetch_one(&self.pool)
        .await?)
    }
}

/// Adds `(position(lower(needle) in lower(column)) > 0 OR …)` to a query.
/// `position` treats `%` and `_` literally, unlike `LIKE`.
fn push_contains(query: &mut sqlx::QueryBuilder<sqlx::Postgres>, needle: &str, columns: &[&str]) {
    query.push("(");
    for (index, column) in columns.iter().enumerate() {
        if index > 0 {
            query.push(" OR ");
        }
        query
            .push("position(lower(")
            .push_bind(needle)
            .push(") in lower(")
            .push(*column)
            .push(")) > 0");
    }
    query.push(")");
}
