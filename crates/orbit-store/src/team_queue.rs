//! Durable Agent Team work queue.

use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

use crate::{Store, StoreError, StoreResult};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, sqlx::FromRow, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TeamQueueItem {
    pub id: Uuid,
    pub org_id: Uuid,
    pub team_id: Uuid,
    pub external_key: String,
    pub repository: Option<String>,
    pub payload: Value,
    pub status: String,
    pub team_run_id: Option<Uuid>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub attempts: i32,
    pub max_attempts: i32,
}

/// A manual queue item to insert (see [`Store::enqueue_team_items`]).
pub struct NewQueueItem<'a> {
    pub external_key: &'a str,
    pub repository: Option<&'a str>,
    pub payload: &'a Value,
}

impl Store {
    /// Most recent `limit` items for a team, returned oldest-first.
    /// The newest `limit` items, oldest first. With `before`, the newest items created
    /// before that item, so older items stay reachable page by page (#432).
    pub async fn list_team_queue(
        &self,
        org_id: Uuid,
        team_id: Uuid,
        limit: i64,
        before: Option<Uuid>,
    ) -> StoreResult<Vec<TeamQueueItem>> {
        Ok(sqlx::query_as::<_, TeamQueueItem>(
            r#"
            SELECT * FROM (
                SELECT * FROM team_queue_items
                WHERE org_id = $1 AND team_id = $2
                  AND ($4::uuid IS NULL OR (created_at, id) < (
                      SELECT created_at, id FROM team_queue_items
                      WHERE id = $4 AND team_id = $2
                  ))
                ORDER BY created_at DESC, id DESC
                LIMIT $3
            ) recent
            ORDER BY created_at ASC, id ASC
            "#,
        )
        .bind(org_id)
        .bind(team_id)
        .bind(limit.clamp(1, 1000))
        .bind(before)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Insert manual queue items in one transaction: all or nothing. An item whose key is
    /// already queued is returned unchanged (payload and timestamps kept) with
    /// `created = false` (#432).
    pub async fn enqueue_team_items(
        &self,
        org_id: Uuid,
        team_id: Uuid,
        items: &[NewQueueItem<'_>],
    ) -> StoreResult<Vec<(TeamQueueItem, bool)>> {
        let mut tx = self.pool.begin().await?;
        let mut stored = Vec::with_capacity(items.len());
        for item in items {
            let inserted = sqlx::query_as::<_, TeamQueueItem>(
                r#"
                INSERT INTO team_queue_items (id, org_id, team_id, external_key, repository, payload)
                VALUES ($1, $2, $3, $4, $5, $6)
                ON CONFLICT (team_id, external_key) DO NOTHING
                RETURNING *
                "#,
            )
            .bind(orbit_domain::new_id())
            .bind(org_id)
            .bind(team_id)
            .bind(item.external_key)
            .bind(item.repository)
            .bind(item.payload)
            .fetch_optional(&mut *tx)
            .await?;
            match inserted {
                Some(row) => stored.push((row, true)),
                None => {
                    let existing = sqlx::query_as::<_, TeamQueueItem>(
                        "SELECT * FROM team_queue_items WHERE team_id = $1 AND external_key = $2",
                    )
                    .bind(team_id)
                    .bind(item.external_key)
                    .fetch_one(&mut *tx)
                    .await?;
                    stored.push((existing, false));
                }
            }
        }
        tx.commit().await?;
        Ok(stored)
    }

    /// Oldest pending items first; what the auto-run loop needs.
    pub async fn list_pending_team_queue(
        &self,
        org_id: Uuid,
        team_id: Uuid,
        limit: i64,
    ) -> StoreResult<Vec<TeamQueueItem>> {
        Ok(sqlx::query_as::<_, TeamQueueItem>(
            r#"
            SELECT * FROM team_queue_items
            WHERE org_id = $1 AND team_id = $2 AND status = 'pending'
            ORDER BY created_at ASC, id ASC
            LIMIT $3
            "#,
        )
        .bind(org_id)
        .bind(team_id)
        .bind(limit.clamp(1, 1000))
        .fetch_all(&self.pool)
        .await?)
    }

    /// Insert a work-source item only when the dedupe key is not already present.
    /// Returns `None` when the item already exists, so callers can skip cleanly.
    #[allow(clippy::too_many_arguments)]
    pub async fn try_enqueue_team_item(
        &self,
        org_id: Uuid,
        team_id: Uuid,
        external_key: &str,
        repository: Option<&str>,
        payload: &Value,
        max_attempts: i32,
    ) -> StoreResult<Option<TeamQueueItem>> {
        sqlx::query_as::<_, TeamQueueItem>(
            r#"
            INSERT INTO team_queue_items
                (id, org_id, team_id, external_key, repository, payload, max_attempts)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (team_id, external_key) DO NOTHING
            RETURNING *
            "#,
        )
        .bind(orbit_domain::new_id())
        .bind(org_id)
        .bind(team_id)
        .bind(external_key)
        .bind(repository)
        .bind(payload)
        .bind(max_attempts)
        .fetch_optional(&self.pool)
        .await
        .map_err(Into::into)
    }

    /// Most recent external keys for dedupe. Bounded; `try_enqueue_team_item` remains the
    /// authoritative dedupe (`ON CONFLICT DO NOTHING`).
    pub async fn list_team_queue_external_keys(
        &self,
        team_id: Uuid,
        limit: i64,
    ) -> StoreResult<Vec<String>> {
        Ok(sqlx::query_scalar::<_, String>(
            "SELECT external_key FROM team_queue_items WHERE team_id = $1 ORDER BY created_at DESC, id DESC LIMIT $2",
        )
        .bind(team_id)
        .bind(limit.clamp(1, 20_000))
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn get_team_queue_item_by_run(
        &self,
        team_run_id: Uuid,
    ) -> StoreResult<Option<TeamQueueItem>> {
        Ok(sqlx::query_as::<_, TeamQueueItem>(
            "SELECT * FROM team_queue_items WHERE team_run_id = $1",
        )
        .bind(team_run_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn get_team_queue_item(
        &self,
        org_id: Uuid,
        item_id: Uuid,
    ) -> StoreResult<TeamQueueItem> {
        sqlx::query_as::<_, TeamQueueItem>(
            "SELECT * FROM team_queue_items WHERE org_id = $1 AND id = $2",
        )
        .bind(org_id)
        .bind(item_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::NotFound("team queue item".into()))
    }

    /// Atomically claim a `pending`/`failed` item for starting a run. Exactly one caller wins.
    pub async fn claim_team_queue_item(
        &self,
        org_id: Uuid,
        item_id: Uuid,
    ) -> StoreResult<Option<TeamQueueItem>> {
        Ok(sqlx::query_as::<_, TeamQueueItem>(
            r#"
            UPDATE team_queue_items
            SET status = 'running', error = NULL, team_run_id = NULL, updated_at = now()
            WHERE org_id = $1 AND id = $2 AND status IN ('pending', 'failed')
            RETURNING *
            "#,
        )
        .bind(org_id)
        .bind(item_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    /// Attach the run created for a claimed item. False when the claim was lost
    /// (cancelled, or already attached).
    pub async fn attach_team_run_to_queue_item(
        &self,
        org_id: Uuid,
        item_id: Uuid,
        team_run_id: Uuid,
    ) -> StoreResult<bool> {
        let result = sqlx::query(
            r#"
            UPDATE team_queue_items
            SET team_run_id = $3, updated_at = now()
            WHERE org_id = $1 AND id = $2 AND status = 'running' AND team_run_id IS NULL
            "#,
        )
        .bind(org_id)
        .bind(item_id)
        .bind(team_run_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// A claimed item whose run could not be started: detach and make it retryable.
    pub async fn fail_claimed_team_queue_item(
        &self,
        org_id: Uuid,
        item_id: Uuid,
        error: &str,
    ) -> StoreResult<bool> {
        let result = sqlx::query(
            r#"
            UPDATE team_queue_items
            SET status = 'failed', error = $3, team_run_id = NULL, updated_at = now()
            WHERE org_id = $1 AND id = $2 AND status = 'running'
            "#,
        )
        .bind(org_id)
        .bind(item_id)
        .bind(error)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Items claimed (`running`) that never got a run attached, e.g. the process died
    /// between the claim and run creation. Returns them to `pending`.
    pub async fn release_stale_team_queue_claims(&self, older_than_secs: i64) -> StoreResult<u64> {
        let result = sqlx::query(
            r#"
            UPDATE team_queue_items
            SET status = 'pending', error = 'start interrupted; re-queued', updated_at = now()
            WHERE status = 'running' AND team_run_id IS NULL
              AND updated_at < now() - make_interval(secs => $1::double precision)
            "#,
        )
        .bind(older_than_secs as f64)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// Close a `running` item once its run's outcome has been applied.
    pub async fn close_team_queue_item_by_run(
        &self,
        team_run_id: Uuid,
        status: &str,
        error: Option<&str>,
    ) -> StoreResult<bool> {
        let result = sqlx::query(
            "UPDATE team_queue_items SET status = $2, error = $3, updated_at = now() WHERE team_run_id = $1 AND status = 'running'",
        )
        .bind(team_run_id)
        .bind(status)
        .bind(error)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// A run failed and attempts remain: back to `pending`, consuming an attempt.
    pub async fn release_team_queue_item_for_retry(
        &self,
        team_run_id: Uuid,
        error: Option<&str>,
    ) -> StoreResult<bool> {
        let result = sqlx::query(
            r#"
            UPDATE team_queue_items
            SET status = 'pending', error = $2, attempts = attempts + 1,
                team_run_id = NULL, updated_at = now()
            WHERE team_run_id = $1 AND status = 'running'
            "#,
        )
        .bind(team_run_id)
        .bind(error)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Terminal runs whose queue item is still `running`: the outcome was never
    /// (fully) applied and must be retried.
    pub async fn list_team_runs_pending_outcome(
        &self,
        limit: i64,
    ) -> StoreResult<Vec<crate::TeamRun>> {
        Ok(sqlx::query_as::<_, crate::TeamRun>(
            r#"
            SELECT r.* FROM team_runs r
            JOIN team_queue_items q ON q.team_run_id = r.id
            WHERE q.status = 'running' AND r.status IN ('succeeded', 'failed', 'cancelled')
            ORDER BY r.updated_at ASC
            LIMIT $1
            "#,
        )
        .bind(limit.clamp(1, 500))
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn set_team_queue_status(
        &self,
        org_id: Uuid,
        item_id: Uuid,
        status: &str,
        error: Option<&str>,
    ) -> StoreResult<()> {
        let result = sqlx::query(
            "UPDATE team_queue_items SET status = $3, error = $4, updated_at = now() WHERE org_id = $1 AND id = $2",
        )
        .bind(org_id)
        .bind(item_id)
        .bind(status)
        .bind(error)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(StoreError::NotFound("team queue item".into()));
        }
        Ok(())
    }

    /// Delete an item that is still pending (used to undo an enqueue whose source claim failed).
    pub async fn delete_pending_team_queue_item(
        &self,
        org_id: Uuid,
        item_id: Uuid,
    ) -> StoreResult<bool> {
        let result = sqlx::query(
            "DELETE FROM team_queue_items WHERE org_id = $1 AND id = $2 AND status = 'pending'",
        )
        .bind(org_id)
        .bind(item_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Manual retry of a `failed`/`cancelled` item, consuming an attempt.
    /// `None` when the item is not retryable (wrong status or attempts exhausted).
    pub async fn retry_team_queue_item(
        &self,
        org_id: Uuid,
        item_id: Uuid,
        error: Option<&str>,
    ) -> StoreResult<Option<TeamQueueItem>> {
        Ok(sqlx::query_as::<_, TeamQueueItem>(
            r#"
            UPDATE team_queue_items q
            SET status = 'pending', error = $3, attempts = attempts + 1,
                team_run_id = NULL, updated_at = now()
            WHERE q.org_id = $1 AND q.id = $2
              AND q.status IN ('failed', 'cancelled')
              AND q.attempts < q.max_attempts
              -- Never start a second run while the previous run's execution is live.
              AND NOT EXISTS (
                  SELECT 1 FROM executions e
                  WHERE q.team_run_id IS NOT NULL AND e.team_run_id = q.team_run_id
                    AND e.status IN ('queued', 'dispatched', 'running', 'awaiting_approval')
              )
            RETURNING q.*
            "#,
        )
        .bind(org_id)
        .bind(item_id)
        .bind(error)
        .fetch_optional(&self.pool)
        .await?)
    }

    /// Manual requeue: pending again without consuming an attempt. Never revives
    /// `succeeded` or `running` items.
    pub async fn requeue_team_queue_item(
        &self,
        org_id: Uuid,
        item_id: Uuid,
    ) -> StoreResult<Option<TeamQueueItem>> {
        Ok(sqlx::query_as::<_, TeamQueueItem>(
            r#"
            UPDATE team_queue_items q
            SET status = 'pending', error = NULL, team_run_id = NULL, updated_at = now()
            WHERE q.org_id = $1 AND q.id = $2 AND q.status IN ('pending', 'failed', 'cancelled')
              -- Never start a second run while the previous run's execution is live.
              AND NOT EXISTS (
                  SELECT 1 FROM executions e
                  WHERE q.team_run_id IS NOT NULL AND e.team_run_id = q.team_run_id
                    AND e.status IN ('queued', 'dispatched', 'running', 'awaiting_approval')
              )
            RETURNING q.*
            "#,
        )
        .bind(org_id)
        .bind(item_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    /// Cancel a pending/running/failed item. The run link is kept so that
    /// [`Self::requeue_team_queue_item`] can refuse while that run's execution is still
    /// stopping; outcome handling ignores items that are no longer `running`.
    /// `None` when it is already terminal (`succeeded`/`cancelled`).
    pub async fn cancel_team_queue_item(
        &self,
        org_id: Uuid,
        item_id: Uuid,
        error: Option<&str>,
    ) -> StoreResult<Option<TeamQueueItem>> {
        Ok(sqlx::query_as::<_, TeamQueueItem>(
            r#"
            UPDATE team_queue_items
            SET status = 'cancelled', error = $3, updated_at = now()
            WHERE org_id = $1 AND id = $2 AND status IN ('pending', 'running', 'failed')
            RETURNING *
            "#,
        )
        .bind(org_id)
        .bind(item_id)
        .bind(error)
        .fetch_optional(&self.pool)
        .await?)
    }
}
