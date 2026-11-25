//! Scoped worker tokens for the execution plane.
//!
//! Only a SHA-256 hash is stored; the plaintext is returned once at issuance.

use chrono::Utc;
use uuid::Uuid;

use crate::models::WorkerTokenRecord;
use crate::{Store, StoreError, StoreResult};

impl Store {
    #[allow(clippy::too_many_arguments)]
    pub async fn create_worker_token(
        &self,
        org_id: Uuid,
        profile_id: Option<Uuid>,
        name: &str,
        token_prefix: &str,
        token_hash: &str,
        harnesses: &[String],
    ) -> StoreResult<WorkerTokenRecord> {
        sqlx::query_as::<_, WorkerTokenRecord>(
            r#"
            INSERT INTO worker_tokens (id, org_id, profile_id, name, token_prefix, token_hash, harnesses)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            RETURNING *
            "#,
        )
        .bind(orbit_domain::new_id())
        .bind(org_id)
        .bind(profile_id)
        .bind(name)
        .bind(token_prefix)
        .bind(token_hash)
        .bind(harnesses)
        .fetch_one(&self.pool)
        .await
        .map_err(Into::into)
    }

    pub async fn find_worker_token_by_hash(
        &self,
        token_hash: &str,
    ) -> StoreResult<Option<WorkerTokenRecord>> {
        let record = sqlx::query_as::<_, WorkerTokenRecord>(
            "SELECT * FROM worker_tokens WHERE token_hash = $1 AND revoked_at IS NULL",
        )
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await?;
        if let Some(record) = &record {
            if record
                .expires_at
                .is_some_and(|expires| expires < Utc::now())
            {
                return Ok(None);
            }
        }
        Ok(record)
    }

    /// Record that the token was used. Written at most once a minute: every worker
    /// request authenticates, and a running execution polls about once a second, so an
    /// unconditional write made this one row the hottest in the database (#447).
    /// Returns whether the row was written.
    pub async fn touch_worker_token(&self, token_id: Uuid) -> StoreResult<bool> {
        let result = sqlx::query(
            r#"
            UPDATE worker_tokens SET last_used_at = now()
            WHERE id = $1
              AND (last_used_at IS NULL OR last_used_at < now() - interval '60 seconds')
            "#,
        )
        .bind(token_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn revoke_worker_token(&self, org_id: Uuid, token_id: Uuid) -> StoreResult<()> {
        let result = sqlx::query(
            "UPDATE worker_tokens SET revoked_at = now() WHERE org_id = $1 AND id = $2 AND revoked_at IS NULL",
        )
        .bind(org_id)
        .bind(token_id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(StoreError::NotFound("worker token".into()));
        }
        Ok(())
    }

    pub async fn revoke_profile_worker_tokens(
        &self,
        org_id: Uuid,
        profile_id: Uuid,
    ) -> StoreResult<()> {
        sqlx::query(
            "UPDATE worker_tokens SET revoked_at = now() WHERE org_id = $1 AND profile_id = $2 AND revoked_at IS NULL",
        )
        .bind(org_id)
        .bind(profile_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_worker_tokens(&self, org_id: Uuid) -> StoreResult<Vec<WorkerTokenRecord>> {
        Ok(sqlx::query_as::<_, WorkerTokenRecord>(
            "SELECT * FROM worker_tokens WHERE org_id = $1 ORDER BY created_at DESC",
        )
        .bind(org_id)
        .fetch_all(&self.pool)
        .await?)
    }
}
