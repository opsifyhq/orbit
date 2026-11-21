use chrono::{DateTime, Utc};
use sqlx::pool::PoolConnection;
use sqlx::Postgres;
use uuid::Uuid;

use crate::models::{AuditLogEntry, AuditLogRecord};
use crate::{Store, StoreResult};

/// Bounded batch size for audit retention deletes.
pub const AUDIT_PRUNE_BATCH_SIZE: i64 = 5000;
/// Stable cluster-wide advisory lock key (the bytes of `ORIBAUDT`).
const AUDIT_RETENTION_LOCK_KEY: i64 = i64::from_be_bytes(*b"ORIBAUDT");

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
        let mut rows = sqlx::query_as::<_, AuditLogRecord>(
            r#"
            SELECT * FROM audit_logs
            WHERE org_id = $1
              AND ($2::text IS NULL OR action = $2)
              AND ($3::bigint IS NULL OR id < $3)
            ORDER BY id DESC
            LIMIT $4
            "#,
        )
        .bind(org_id)
        .bind(action)
        .bind(before_id)
        .bind(limit + 1)
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
    pub async fn prune_audit_logs(&self, before: DateTime<Utc>) -> StoreResult<u64> {
        self.prune_audit_logs_in_batches(before, AUDIT_PRUNE_BATCH_SIZE)
            .await
    }

    /// Same as [`Store::prune_audit_logs`] with an explicit batch size (used by tests).
    /// Every batch is a separate autocommit, so the first run on a large table never
    /// holds one huge transaction or IO burst.
    pub async fn prune_audit_logs_in_batches(
        &self,
        before: DateTime<Utc>,
        batch_size: i64,
    ) -> StoreResult<u64> {
        let mut conn = self.pool.acquire().await?;
        prune_batches(&mut conn, before, batch_size).await
    }

    /// Try to become the single audit retention runner in the cluster. The lock is
    /// session-scoped on the returned connection; dropping the guard releases it even if
    /// the supervised loop was cancelled or panicked.
    pub async fn try_lock_audit_retention(&self) -> StoreResult<Option<AuditRetentionLock>> {
        let mut conn = self.pool.acquire().await?;
        let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
            .bind(AUDIT_RETENTION_LOCK_KEY)
            .fetch_one(&mut *conn)
            .await?;
        Ok(locked.then_some(AuditRetentionLock { conn: Some(conn) }))
    }
}

async fn prune_batches(
    conn: &mut PoolConnection<Postgres>,
    before: DateTime<Utc>,
    batch_size: i64,
) -> StoreResult<u64> {
    let batch_size = batch_size.max(1);
    let mut removed_total = 0;
    loop {
        let removed = sqlx::query(
            r#"
            DELETE FROM audit_logs
            WHERE id IN (
                SELECT id FROM audit_logs
                WHERE created_at < $1
                ORDER BY created_at, id
                LIMIT $2
            )
            "#,
        )
        .bind(before)
        .bind(batch_size)
        .execute(&mut **conn)
        .await?
        .rows_affected();
        removed_total += removed;
        // A short final batch means there is nothing left to delete.
        if removed < batch_size as u64 {
            return Ok(removed_total);
        }
    }
}

/// Owns the cluster-wide audit retention advisory lock. Use [`Self::prune`] to delete on
/// the lock's own connection; the lock stays held for the whole retention tick.
pub struct AuditRetentionLock {
    conn: Option<PoolConnection<Postgres>>,
}

impl AuditRetentionLock {
    pub async fn prune(&mut self, before: DateTime<Utc>) -> StoreResult<u64> {
        let Some(conn) = self.conn.as_mut() else {
            return Ok(0);
        };
        prune_batches(conn, before, AUDIT_PRUNE_BATCH_SIZE).await
    }
}

impl Drop for AuditRetentionLock {
    fn drop(&mut self) {
        let Some(mut conn) = self.conn.take() else {
            return;
        };
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        // `Drop` cannot await. Release on a spawned task so cancellation does not strand
        // a session lock on a pooled connection; closing the connection is the fallback.
        handle.spawn(async move {
            let unlocked: Result<bool, _> = sqlx::query_scalar("SELECT pg_advisory_unlock($1)")
                .bind(AUDIT_RETENTION_LOCK_KEY)
                .fetch_one(&mut *conn)
                .await;
            if !matches!(unlocked, Ok(true)) {
                let _ = conn.close().await;
            }
        });
    }
}
