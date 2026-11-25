use chrono::{DateTime, Utc};
use serde_json::json;
use uuid::Uuid;

use crate::models::ExecutionRecord;
use crate::{Store, StoreResult};

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct WorkerHeartbeat {
    pub worker_id: String,
    pub harnesses: Vec<String>,
    pub target: String,
    pub hostname: Option<String>,
    pub started_at: DateTime<Utc>,
    pub last_heartbeat_at: DateTime<Utc>,
    pub boot_id: Option<Uuid>,
    pub boot_started_at: Option<DateTime<Utc>>,
}

/// Result of a heartbeat that also reports a worker boot identity.
#[derive(Debug, Clone)]
pub struct HeartbeatOutcome {
    /// Whether the calling token owned the worker record and the heartbeat landed.
    pub accepted: bool,
    /// Whether the reported boot replaced a different boot on the same worker id.
    pub boot_changed: bool,
    /// Non-terminal executions from the previous boot that were failed by the change.
    pub failed_executions: Vec<ExecutionRecord>,
}

impl Store {
    /// Record a heartbeat. The row is owned by the first token that reports a worker id;
    /// another token (or org) cannot overwrite it. Returns `false` when the worker id
    /// belongs to someone else. Pre-binding rows (NULL owner) are adopted.
    ///
    /// Callers without boot identity (legacy workers) keep the original behaviour; use
    /// [`Self::upsert_worker_heartbeat_with_boot`] to report a boot and detect restarts.
    pub async fn upsert_worker_heartbeat(
        &self,
        org_id: Uuid,
        token_id: Uuid,
        worker_id: &str,
        harnesses: &[String],
        target: &str,
        hostname: Option<&str>,
    ) -> StoreResult<bool> {
        Ok(self
            .upsert_worker_heartbeat_with_boot(
                org_id, token_id, worker_id, harnesses, target, hostname, None, None,
            )
            .await?
            .accepted)
    }

    /// Record a heartbeat together with the identity of the current worker boot.
    ///
    /// A boot change means the previous process can no longer report an outcome, so any
    /// non-terminal execution bound to that boot is failed in the same transaction. A
    /// heartbeat from a token that does not own the worker id is refused without changing
    /// either the heartbeat row or any execution.
    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_worker_heartbeat_with_boot(
        &self,
        org_id: Uuid,
        token_id: Uuid,
        worker_id: &str,
        harnesses: &[String],
        target: &str,
        hostname: Option<&str>,
        boot_id: Option<Uuid>,
        boot_started_at: Option<DateTime<Utc>>,
    ) -> StoreResult<HeartbeatOutcome> {
        let mut tx = self.pool.begin().await?;
        // Lock the row so two concurrent boots cannot both read the same predecessor.
        let previous: Option<(Option<Uuid>, Option<DateTime<Utc>>)> = sqlx::query_as(
            "SELECT boot_id, boot_started_at FROM worker_heartbeats WHERE worker_id = $1 FOR UPDATE",
        )
        .bind(worker_id)
        .fetch_optional(&mut *tx)
        .await?;
        let (previous_boot_id, previous_boot_started_at) = previous.unwrap_or((None, None));
        // During a rolling update the terminating pod keeps heartbeating under the same
        // worker id. Its boot is older than the recorded one: it is neither a restart nor
        // the boot to remember, so it must not fail the newer boot's executions.
        let stale_boot = match (previous_boot_id, boot_id) {
            (Some(previous), Some(current)) if previous != current => {
                boot_is_older(current, boot_started_at, previous, previous_boot_started_at)
            }
            _ => false,
        };

        let result = sqlx::query(
            r#"
            INSERT INTO worker_heartbeats
                (worker_id, harnesses, target, hostname, last_heartbeat_at, org_id, token_id,
                 boot_id, boot_started_at)
            VALUES ($1, $2, $3, $4, now(), $5, $6, $7, $8)
            ON CONFLICT (worker_id) DO UPDATE SET
                harnesses = EXCLUDED.harnesses,
                target = EXCLUDED.target,
                hostname = COALESCE(EXCLUDED.hostname, worker_heartbeats.hostname),
                last_heartbeat_at = now(),
                org_id = EXCLUDED.org_id,
                token_id = EXCLUDED.token_id,
                boot_id = CASE WHEN $9 THEN worker_heartbeats.boot_id
                               ELSE COALESCE(EXCLUDED.boot_id, worker_heartbeats.boot_id) END,
                boot_started_at = CASE WHEN $9 THEN worker_heartbeats.boot_started_at
                                       ELSE COALESCE(EXCLUDED.boot_started_at, worker_heartbeats.boot_started_at) END
            WHERE worker_heartbeats.token_id IS NULL
               OR worker_heartbeats.token_id = EXCLUDED.token_id
            "#,
        )
        .bind(worker_id)
        .bind(harnesses)
        .bind(target)
        .bind(hostname)
        .bind(org_id)
        .bind(token_id)
        .bind(boot_id)
        .bind(boot_started_at)
        .bind(stale_boot)
        .execute(&mut *tx)
        .await?;

        if result.rows_affected() == 0 {
            // Nothing was accepted: no boot change and no executions to fail.
            tx.commit().await?;
            return Ok(HeartbeatOutcome {
                accepted: false,
                boot_changed: false,
                failed_executions: Vec::new(),
            });
        }

        let boot_changed = !stale_boot
            && matches!(
                (previous_boot_id, boot_id),
                (Some(previous), Some(current)) if previous != current
            );
        let failed_executions = if boot_changed {
            let current_boot_id = boot_id.expect("boot_id is Some when boot_changed");
            let rows = sqlx::query_as::<_, ExecutionRecord>(
                r#"
                UPDATE executions e
                SET status = 'failed',
                    error = $4::jsonb,
                    finished_at = now(),
                    updated_at = now(),
                    execution_lease_until = NULL
                WHERE e.org_id = $1
                  AND e.worker_id = $2
                  AND e.status IN ('dispatched', 'running', 'awaiting_approval')
                  AND e.worker_boot_id IS NOT NULL
                  AND e.worker_boot_id <> $3
                RETURNING e.*
                "#,
            )
            .bind(org_id)
            .bind(worker_id)
            .bind(current_boot_id)
            .bind(worker_restarted_error())
            .fetch_all(&mut *tx)
            .await?;

            for row in &rows {
                crate::executions::expire_pending_approvals(&mut tx, row.id).await?;
            }
            rows
        } else {
            Vec::new()
        };
        tx.commit().await?;
        for row in &failed_executions {
            self.observe_finished(row, crate::observe::FinishCause::WorkerLost);
        }
        Ok(HeartbeatOutcome {
            accepted: true,
            boot_changed,
            failed_executions,
        })
    }

    /// Renew the per-execution leases the worker is actively running.
    ///
    /// Only non-terminal executions claimed by this worker and this token are updated;
    /// ids the caller does not list are left untouched. Boot identity is stamped once,
    /// so a legacy worker that reports it later inherits it without overwriting an
    /// already-recorded boot. Returns the ids whose leases were actually renewed.
    ///
    /// Renewal is not activity: `updated_at` is left alone, so a `dispatched` run that
    /// its worker keeps listing but never starts is still ended by
    /// [`Self::reap_unstarted_executions`] (#395).
    #[allow(clippy::too_many_arguments)]
    pub async fn touch_execution_leases(
        &self,
        org_id: Uuid,
        token_id: Uuid,
        worker_id: &str,
        boot_id: Option<Uuid>,
        boot_started_at: Option<DateTime<Utc>>,
        active_execution_ids: &[Uuid],
        lease_seconds: i64,
    ) -> StoreResult<Vec<Uuid>> {
        Ok(sqlx::query_scalar::<_, Uuid>(
            r#"
            UPDATE executions
            SET last_execution_heartbeat_at = now(),
                execution_lease_until = now() + make_interval(secs => $7),
                worker_boot_id = COALESCE(worker_boot_id, $5),
                worker_boot_started_at = COALESCE(worker_boot_started_at, $6)
            WHERE org_id = $1
              AND worker_id = $2
              AND worker_token_id = $3
              AND id = ANY($4)
              AND status IN ('dispatched', 'running', 'awaiting_approval')
            RETURNING id
            "#,
        )
        .bind(org_id)
        .bind(worker_id)
        .bind(token_id)
        .bind(active_execution_ids)
        .bind(boot_id)
        .bind(boot_started_at)
        .bind(lease_seconds as f64)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Delete a worker record, only if the calling token owns it.
    pub async fn delete_worker_heartbeat(
        &self,
        org_id: Uuid,
        token_id: Uuid,
        worker_id: &str,
    ) -> StoreResult<bool> {
        let result = sqlx::query(
            "DELETE FROM worker_heartbeats WHERE worker_id = $1 AND org_id = $2 AND token_id = $3",
        )
        .bind(worker_id)
        .bind(org_id)
        .bind(token_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Mark executions whose worker heartbeat expired as failed and expire their pending
    /// approvals. Returns the affected executions so the caller can emit events.
    ///
    /// This is deliberately conservative: orphaned work is failed, not re-run, to avoid
    /// duplicate side effects. Retries become a separate, idempotency-aware feature.
    ///
    /// A `dispatched` run whose worker has never written a heartbeat is skipped: the
    /// launcher claims the run for the new worker id before the container or Job starts.
    /// [`Self::reap_unstarted_executions`] ends it if that worker never appears.
    pub async fn reap_orphaned_executions(
        &self,
        stale_after_seconds: i64,
    ) -> StoreResult<Vec<ExecutionRecord>> {
        self.reap_orphaned_executions_scoped(None, stale_after_seconds)
            .await
    }

    /// Org-scoped reaper used by tests and future per-tenant maintenance jobs.
    pub async fn reap_orphaned_executions_for_org(
        &self,
        org_id: Uuid,
        stale_after_seconds: i64,
    ) -> StoreResult<Vec<ExecutionRecord>> {
        self.reap_orphaned_executions_scoped(Some(org_id), stale_after_seconds)
            .await
    }

    async fn reap_orphaned_executions_scoped(
        &self,
        org_id: Option<Uuid>,
        stale_after_seconds: i64,
    ) -> StoreResult<Vec<ExecutionRecord>> {
        let mut tx = self.pool.begin().await?;
        let rows = sqlx::query_as::<_, ExecutionRecord>(
            r#"
            UPDATE executions e
            SET status = 'failed',
                error = jsonb_build_object(
                    'code', 'worker_lost',
                    'message', 'worker heartbeat expired; execution outcome is unknown',
                    'retryable', true
                ),
                finished_at = now(),
                updated_at = now()
            WHERE e.status IN ('dispatched', 'running', 'awaiting_approval')
              AND e.worker_id IS NOT NULL
              -- Grace period: do not reap work that was just claimed.
              AND COALESCE(e.started_at, e.created_at) < now() - interval '15 seconds'
              AND ($2::uuid IS NULL OR e.org_id = $2)
              AND NOT EXISTS (
                  SELECT 1 FROM worker_heartbeats h
                  WHERE h.worker_id = e.worker_id
                    AND h.last_heartbeat_at > now() - make_interval(secs => $1)
              )
              -- A launch claims its execution for the new worker id before the
              -- container or Job exists, so a `dispatched` run whose worker has never
              -- reported is still starting (image pull, scheduling), not lost. Those
              -- are ended by `reap_unstarted_executions` if the worker never appears.
              AND (
                  e.status <> 'dispatched'
                  OR EXISTS (
                      SELECT 1 FROM worker_heartbeats h WHERE h.worker_id = e.worker_id
                  )
              )
            RETURNING e.*
            "#,
        )
        .bind(stale_after_seconds as f64)
        .bind(org_id)
        .fetch_all(&mut *tx)
        .await?;

        if !rows.is_empty() {
            let ids: Vec<Uuid> = rows.iter().map(|row| row.id).collect();
            sqlx::query(
                r#"
                UPDATE approvals
                SET status = 'expired', decided_at = now()
                WHERE execution_id = ANY($1) AND status = 'pending'
                "#,
            )
            .bind(&ids)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        for row in &rows {
            self.observe_finished(row, crate::observe::FinishCause::WorkerLost);
        }
        Ok(rows)
    }

    /// End runs a live worker claimed but never started.
    ///
    /// A worker that cannot decode a claim (for example one older than the platform) or
    /// fails before reporting leaves the run `dispatched` while its heartbeat stays fresh,
    /// so [`Self::reap_orphaned_executions`] never sees it and the run blocks the agent's
    /// concurrency. A run is ended once nothing has happened on it (claim, cancel request
    /// or event) for `silent_after_seconds`, or `cancel_after_seconds` once a cancel was
    /// requested. Cancelled runs end as `cancelled`, the rest as `failed`. A worker that
    /// reports in afterwards is refused, because terminal states are never overwritten.
    pub async fn reap_unstarted_executions(
        &self,
        org_id: Option<Uuid>,
        silent_after_seconds: i64,
        cancel_after_seconds: i64,
    ) -> StoreResult<Vec<ExecutionRecord>> {
        let rows = sqlx::query_as::<_, ExecutionRecord>(
            r#"
            UPDATE executions e
            SET status = CASE WHEN e.cancel_requested THEN 'cancelled' ELSE 'failed' END,
                error = CASE
                    WHEN e.cancel_requested THEN e.error
                    ELSE jsonb_build_object(
                        'code', 'worker_did_not_start',
                        'message', 'a worker took this execution but never started it',
                        'retryable', true
                    )
                END,
                finished_at = now(),
                updated_at = now()
            WHERE e.status = 'dispatched'
              AND ($3::uuid IS NULL OR e.org_id = $3)
              AND GREATEST(
                    e.updated_at,
                    COALESCE(e.started_at, e.created_at),
                    COALESCE(
                        (SELECT max(ev.created_at) FROM execution_events ev
                         WHERE ev.execution_id = e.id),
                        e.created_at
                    )
                  ) < now() - make_interval(
                    secs => CASE WHEN e.cancel_requested THEN $2 ELSE $1 END
                  )
            RETURNING e.*
            "#,
        )
        .bind(silent_after_seconds as f64)
        .bind(cancel_after_seconds as f64)
        .bind(org_id)
        .fetch_all(&self.pool)
        .await?;
        for row in &rows {
            self.observe_finished(row, crate::observe::FinishCause::NeverStarted);
        }
        Ok(rows)
    }

    /// End runs stuck `running` past their deadline, regardless of worker heartbeat.
    ///
    /// Liveness is otherwise tracked per worker: a healthy worker whose runner task died,
    /// or whose `/complete` call never got through, keeps heartbeating while the run stays
    /// `running` and holds the agent's concurrency slot. A run is ended as `timed_out`
    /// once `running_started_at + timeout_seconds + grace_seconds` has passed (the clock
    /// starts when the harness reports `running`, so checkout and worker start-up do not
    /// count; dispatch is bounded by [`Self::reap_unstarted_executions`]). Time spent waiting
    /// on approvals is added to the deadline because the worker pauses the run's clock
    /// for it. The conditional `status = 'running'` makes this safe to run from several
    /// API replicas and loses cleanly to a worker that reports in at the same moment.
    pub async fn reap_timed_out_executions(
        &self,
        org_id: Option<Uuid>,
        grace_seconds: i64,
    ) -> StoreResult<Vec<ExecutionRecord>> {
        let mut tx = self.pool.begin().await?;
        let rows = sqlx::query_as::<_, ExecutionRecord>(
            r#"
            UPDATE executions e
            SET status = 'timed_out',
                error = jsonb_build_object(
                    'code', 'execution_timeout',
                    'message', 'execution exceeded its timeout and was not reported finished by its worker',
                    'retryable', true
                ),
                finished_at = now(),
                updated_at = now()
            WHERE e.status = 'running'
              AND COALESCE(e.running_started_at, e.started_at) IS NOT NULL
              AND ($2::uuid IS NULL OR e.org_id = $2)
              AND COALESCE(e.running_started_at, e.started_at)
                  + make_interval(secs =>
                        e.timeout_seconds::float8
                        + $1
                        + COALESCE((
                            SELECT sum(EXTRACT(EPOCH FROM (COALESCE(a.decided_at, now()) - a.created_at)))
                            FROM approvals a WHERE a.execution_id = e.id
                          ), 0)::float8
                    ) < now()
            RETURNING e.*
            "#,
        )
        .bind(grace_seconds as f64)
        .bind(org_id)
        .fetch_all(&mut *tx)
        .await?;

        if !rows.is_empty() {
            let ids: Vec<Uuid> = rows.iter().map(|row| row.id).collect();
            sqlx::query(
                "UPDATE approvals SET status = 'expired', decided_at = now() WHERE execution_id = ANY($1) AND status = 'pending'",
            )
            .bind(&ids)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        for row in &rows {
            self.observe_finished(row, crate::observe::FinishCause::TimedOut);
        }
        Ok(rows)
    }

    /// Fail executions whose lease expired while their worker is still heartbeating.
    ///
    /// A live worker whose runner task died (or whose completion call never arrived) would
    /// otherwise keep the execution in a non-terminal state forever. Runs whose worker has
    /// gone quiet are left to [`Self::reap_orphaned_executions`], which reports the worker
    /// as lost instead of blaming the harness.
    pub async fn reap_expired_execution_leases(
        &self,
        worker_stale_after_seconds: i64,
    ) -> StoreResult<Vec<ExecutionRecord>> {
        let mut tx = self.pool.begin().await?;
        let rows = sqlx::query_as::<_, ExecutionRecord>(
            r#"
            UPDATE executions e
            SET status = 'failed',
                error = $2::jsonb,
                finished_at = now(),
                updated_at = now(),
                execution_lease_until = NULL
            WHERE e.status IN ('dispatched', 'running', 'awaiting_approval')
              AND e.execution_lease_until IS NOT NULL
              AND e.execution_lease_until < now()
              AND EXISTS (
                  SELECT 1 FROM worker_heartbeats h
                  WHERE h.worker_id = e.worker_id
                    AND h.last_heartbeat_at > now() - make_interval(secs => $1)
              )
            RETURNING e.*
            "#,
        )
        .bind(worker_stale_after_seconds as f64)
        .bind(harness_unresponsive_error())
        .fetch_all(&mut *tx)
        .await?;

        for row in &rows {
            crate::executions::expire_pending_approvals(&mut tx, row.id).await?;
        }
        tx.commit().await?;
        for row in &rows {
            self.observe_finished(row, crate::observe::FinishCause::HarnessUnresponsive);
        }
        Ok(rows)
    }

    /// Remove heartbeat rows that have been stale for longer than `retention_hours`.
    ///
    /// Workers that are SIGKILLed, evicted or lose their node never get to call the
    /// graceful `offline` endpoint. Keeping those rows forever makes the Workers page
    /// accumulate ghost workers, so the control plane prunes them after a bounded window.
    pub async fn delete_stale_worker_heartbeats(&self, retention_hours: i64) -> StoreResult<u64> {
        let result = sqlx::query(
            "DELETE FROM worker_heartbeats WHERE last_heartbeat_at < now() - make_interval(hours => $1::int)",
        )
        .bind(retention_hours)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    pub async fn list_worker_heartbeats(&self, org_id: Uuid) -> StoreResult<Vec<WorkerHeartbeat>> {
        Ok(sqlx::query_as::<_, WorkerHeartbeat>(
            "SELECT * FROM worker_heartbeats WHERE org_id = $1 ORDER BY last_heartbeat_at DESC",
        )
        .bind(org_id)
        .fetch_all(&self.pool)
        .await?)
    }
}

/// JSON payload used in execution error records for orphaned work.
pub fn worker_lost_error() -> serde_json::Value {
    json!({
        "code": "worker_lost",
        "message": "worker heartbeat expired; execution outcome is unknown",
        "retryable": true
    })
}

/// Error attached to executions inherited from a worker boot that has gone away.
pub fn worker_restarted_error() -> serde_json::Value {
    json!({
        "code": "worker_lost",
        "message": "worker restarted; execution outcome is unknown",
        "retryable": true
    })
}

/// Error attached to executions whose lease expired while the worker is still alive.
pub fn harness_unresponsive_error() -> serde_json::Value {
    json!({
        "code": "harness_unresponsive",
        "message": "worker stopped reporting this execution; outcome is unknown",
        "retryable": true
    })
}

/// Whether boot `current` started before boot `previous`.
///
/// The reported start times decide when both are known; otherwise the UUIDv7 timestamps
/// do. Boots that cannot be ordered (no times, non-v7 ids, the same millisecond) are not
/// older, so they keep the previous rule that any other boot is a restart.
fn boot_is_older(
    current: Uuid,
    current_started_at: Option<DateTime<Utc>>,
    previous: Uuid,
    previous_started_at: Option<DateTime<Utc>>,
) -> bool {
    if let (Some(current), Some(previous)) = (current_started_at, previous_started_at) {
        if current != previous {
            return current < previous;
        }
    }
    match (v7_millis(current), v7_millis(previous)) {
        (Some(current), Some(previous)) => current < previous,
        _ => false,
    }
}

fn v7_millis(id: Uuid) -> Option<u64> {
    if id.get_version_num() != 7 {
        return None;
    }
    let bytes = id.as_bytes();
    Some(
        bytes[..6]
            .iter()
            .fold(0u64, |acc, byte| (acc << 8) | u64::from(*byte)),
    )
}

#[cfg(test)]
mod boot_order_tests {
    use super::*;

    fn v7_at(millis: u64) -> Uuid {
        let mut bytes = *Uuid::now_v7().as_bytes();
        bytes[..6].copy_from_slice(&millis.to_be_bytes()[2..]);
        Uuid::from_bytes(bytes)
    }

    #[test]
    fn start_times_decide_when_both_are_known() {
        let earlier = Utc::now() - chrono::Duration::seconds(30);
        let later = Utc::now();
        // Ids deliberately ordered the other way: the reported start time wins.
        let (a, b) = (v7_at(2_000), v7_at(1_000));
        assert!(boot_is_older(a, Some(earlier), b, Some(later)));
        assert!(!boot_is_older(b, Some(later), a, Some(earlier)));
    }

    #[test]
    fn uuidv7_timestamps_decide_without_start_times() {
        let (old, new) = (v7_at(1_000), v7_at(2_000));
        assert!(boot_is_older(old, None, new, None));
        assert!(!boot_is_older(new, None, old, None));
        let at = Utc::now();
        assert!(
            boot_is_older(old, Some(at), new, Some(at)),
            "equal times fall back to ids"
        );
    }

    #[test]
    fn boots_that_cannot_be_ordered_are_not_older() {
        let v4 = Uuid::new_v4();
        assert!(!boot_is_older(v4, None, v7_at(2_000), None));
        assert!(!boot_is_older(v7_at(1_000), None, v4, None));
        assert!(!boot_is_older(v7_at(1_000), None, v7_at(1_000), None));
    }
}
