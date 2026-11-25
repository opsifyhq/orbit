use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::models::{AuditLogEntry, AuditLogRecord};
use crate::{Store, StoreResult};

/// Rows deleted per transaction by [`Store::prune_audit_logs`].
const AUDIT_PRUNE_BATCH: i64 = 5000;
/// Advisory lock key ("audit" in ASCII) so only one replica prunes at a time.
const AUDIT_PRUNE_LOCK_KEY: i64 = 0x0061_7564_6974;

impl Store {
    #[allow(clippy::too_many_arguments)]
    pub async fn append_audit_log(
        &self,
        org_id: Uuid,
        actor_type: &str,
        actor_id: Option<Uuid>,
        actor_email: Option<&str>,
        action: &str,
        target_type: Option<&str>,
        target_id: Option<Uuid>,
        metadata: serde_json::Value,
        request_id: Option<&str>,
        ip_address: Option<&str>,
    ) -> StoreResult<()> {
        sqlx::query(
            r#"
            INSERT INTO audit_logs (org_id, actor_type, actor_id, actor_email, action,
                                    target_type, target_id, metadata, request_id, ip_address)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            "#,
        )
        .bind(org_id)
        .bind(actor_type)
        .bind(actor_id)
        .bind(actor_email)
        .bind(action)
        .bind(target_type)
        .bind(target_id)
        .bind(metadata)
        .bind(request_id)
        .bind(ip_address)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn record_audit(&self, org_id: Uuid, entry: AuditLogEntry) -> StoreResult<()> {
        self.append_audit_log(
            org_id,
            &entry.actor_type,
            entry.actor_id,
            entry.actor_email.as_deref(),
            &entry.action,
            entry.target_type.as_deref(),
            entry.target_id,
            entry.metadata,
            entry.request_id.as_deref(),
            entry.ip_address.as_deref(),
        )
        .await
    }

    /// Audit logs use a bigserial id cursor (append-only, monotonic).
    pub async fn list_audit_logs(
        &self,
        org_id: Uuid,
        limit: i64,
        before_id: Option<i64>,
        action: Option<&str>,
    ) -> StoreResult<(Vec<AuditLogRecord>, Option<i64>)> {
        self.list_audit_log_page(org_id, limit, before_id, action, None)
            .await
    }

    /// One page of audit rows, newest first. `action` matches exactly; `area` matches
    /// every action of one family (`agent` matches `agent.created`, not `agent_x.y`).
    /// The next cursor is the id of the last row when more rows follow.
    pub async fn list_audit_log_page(
        &self,
        org_id: Uuid,
        limit: i64,
        before_id: Option<i64>,
        action: Option<&str>,
        area: Option<&str>,
    ) -> StoreResult<(Vec<AuditLogRecord>, Option<i64>)> {
        let mut rows = sqlx::query_as::<_, AuditLogRecord>(
            r#"
            SELECT * FROM audit_logs
            WHERE org_id = $1
              AND ($2::text IS NULL OR action = $2)
              AND ($3::bigint IS NULL OR id < $3)
              AND ($5::text IS NULL OR split_part(action, '.', 1) = $5)
            ORDER BY id DESC
            LIMIT $4
            "#,
        )
        .bind(org_id)
        .bind(action)
        .bind(before_id)
        .bind(limit + 1)
        .bind(area)
        .fetch_all(&self.pool)
        .await?;
        let has_more = rows.len() as i64 > limit;
        if has_more {
            rows.truncate(limit as usize);
        }
        let next = if has_more {
            rows.last().map(|r| r.id)
        } else {
            None
        };
        Ok((rows, next))
    }

    /// Delete audit log rows created before `before`; returns the number removed.
    /// Driven by `ORBIT_AUDIT_RETENTION_DAYS` in the API background loop.
    ///
    /// Rows go in batches of [`AUDIT_PRUNE_BATCH`], one short transaction each, so a first
    /// run over a large table never holds one huge transaction. Each batch takes a
    /// transaction-scoped advisory lock: while another replica is pruning this call stops
    /// early (deletes are idempotent, and the next daily run continues). The lock is
    /// released with the transaction, so a cancelled or panicking task cannot leak it.
    pub async fn prune_audit_logs(&self, before: DateTime<Utc>) -> StoreResult<u64> {
        let mut total = 0u64;
        loop {
            let mut tx = self.pool.begin().await?;
            let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)")
                .bind(AUDIT_PRUNE_LOCK_KEY)
                .fetch_one(&mut *tx)
                .await?;
            if !locked {
                return Ok(total);
            }
            let removed = sqlx::query(
                r#"
                DELETE FROM audit_logs
                WHERE id IN (
                    SELECT id FROM audit_logs
                    WHERE created_at < $1
                    ORDER BY created_at
                    LIMIT $2
                )
                "#,
            )
            .bind(before)
            .bind(AUDIT_PRUNE_BATCH)
            .execute(&mut *tx)
            .await?
            .rows_affected();
            tx.commit().await?;
            total += removed;
            if removed < AUDIT_PRUNE_BATCH as u64 {
                return Ok(total);
            }
        }
    }
}
