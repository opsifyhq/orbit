use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use orbit_domain::{new_id, TriggerSpec};

use crate::models::Trigger;
use crate::{Store, StoreError, StoreResult};

/// Next fire time for a schedule trigger config (`cron` + optional IANA `timezone`),
/// strictly after `after`. `None` when the config is invalid or never fires again.
pub fn schedule_next_run(
    config: &serde_json::Value,
    after: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    let cron = config.get("cron")?.as_str()?;
    let timezone = config.get("timezone").and_then(|value| value.as_str());
    orbit_domain::next_occurrence(cron, timezone, after)
        .ok()
        .flatten()
}

/// Reconcile the spec-managed triggers for an agent with the ones declared in its spec.
///
/// Called inside the agent creation/version transaction. Only rows with `managed = true`
/// are ever touched; triggers created through `POST /api/v1/triggers` are preserved.
/// A spec trigger that is unchanged (same kind and config) keeps its row, id and firing
/// history; it is re-enabled (an edit is a fresh start) and its failure counter resets.
/// Changed or removed spec triggers are replaced or deleted. Manual executions are not
/// triggers.
pub(crate) async fn sync_agent_triggers(
    tx: &mut Transaction<'_, Postgres>,
    org_id: Uuid,
    agent_id: Uuid,
    triggers: &[TriggerSpec],
) -> StoreResult<()> {
    let mut existing: Vec<(Uuid, String, serde_json::Value)> = sqlx::query_as(
        "SELECT id, kind, config FROM triggers \
         WHERE org_id = $1 AND agent_id = $2 AND managed \
         ORDER BY created_at, id FOR UPDATE",
    )
    .bind(org_id)
    .bind(agent_id)
    .fetch_all(&mut **tx)
    .await?;

    let now = Utc::now();
    for spec in triggers {
        let (kind, config) = match spec {
            TriggerSpec::Schedule {
                cron,
                timezone,
                task,
            } => (
                "schedule",
                // The task travels in the trigger config, where the scheduler reads it.
                match task
                    .as_deref()
                    .map(str::trim)
                    .filter(|task| !task.is_empty())
                {
                    Some(task) => json!({ "cron": cron, "timezone": timezone, "task": task }),
                    None => json!({ "cron": cron, "timezone": timezone }),
                },
            ),
            TriggerSpec::Webhook { secret_ref } => ("webhook", json!({ "secretRef": secret_ref })),
            TriggerSpec::Event { subject } => ("event", json!({ "subject": subject })),
            TriggerSpec::Manual => continue,
        };
        let next_run_at = if kind == "schedule" {
            schedule_next_run(&config, now)
        } else {
            None
        };

        if let Some(pos) = existing
            .iter()
            .position(|(_, k, c)| k == kind && *c == config)
        {
            let (id, _, _) = existing.remove(pos);
            sqlx::query(
                r#"
                UPDATE triggers
                SET enabled = true, consecutive_failures = 0,
                    next_run_at = CASE WHEN kind = 'schedule' AND next_run_at IS NULL
                                       THEN $2 ELSE next_run_at END,
                    updated_at = now()
                WHERE id = $1
                "#,
            )
            .bind(id)
            .bind(next_run_at)
            .execute(&mut **tx)
            .await?;
            continue;
        }

        sqlx::query(
            r#"
            INSERT INTO triggers (id, org_id, agent_id, kind, enabled, config, next_run_at, managed)
            VALUES ($1, $2, $3, $4, true, $5, $6, true)
            "#,
        )
        .bind(new_id())
        .bind(org_id)
        .bind(agent_id)
        .bind(kind)
        .bind(config)
        .bind(next_run_at)
        .execute(&mut **tx)
        .await?;
    }

    // Managed rows no longer declared by the spec.
    let stale: Vec<Uuid> = existing.into_iter().map(|(id, _, _)| id).collect();
    if !stale.is_empty() {
        sqlx::query("DELETE FROM triggers WHERE org_id = $1 AND id = ANY($2) AND managed")
            .bind(org_id)
            .bind(&stale)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

impl Store {
    pub async fn list_triggers(
        &self,
        org_id: Uuid,
        agent_id: Option<Uuid>,
        access: Option<&crate::access::ResourceAccess>,
    ) -> StoreResult<Vec<Trigger>> {
        let (privileged, user_id, team_ids, isolation) = crate::access::binds(access);
        Ok(sqlx::query_as::<_, Trigger>(
            r#"
            SELECT * FROM triggers
            WHERE org_id = $1 AND ($2::uuid IS NULL OR agent_id = $2)
              AND ($3::bool OR owner_user_id = $4
                   OR CASE WHEN $6::bool AND owner_team_id IS NOT NULL
                        THEN owner_team_id = ANY($5)
                        ELSE visibility = 'org'
                             OR (visibility = 'team' AND owner_team_id = ANY($5)) END)
            ORDER BY created_at DESC
            "#,
        )
        .bind(org_id)
        .bind(agent_id)
        .bind(privileged)
        .bind(user_id)
        .bind(&team_ids)
        .bind(isolation)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn get_trigger(&self, org_id: Uuid, trigger_id: Uuid) -> StoreResult<Trigger> {
        sqlx::query_as::<_, Trigger>("SELECT * FROM triggers WHERE org_id = $1 AND id = $2")
            .bind(org_id)
            .bind(trigger_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| StoreError::NotFound("trigger".into()))
    }

    /// Add a trigger outside of an agent version (e.g. an operator-managed webhook).
    pub async fn create_trigger(
        &self,
        org_id: Uuid,
        agent_id: Uuid,
        kind: &str,
        harness_kind: Option<&str>,
        config: serde_json::Value,
        enabled: bool,
    ) -> StoreResult<Trigger> {
        // Ensure the agent belongs to the org before creating the trigger.
        self.get_agent(org_id, agent_id).await?;
        if !matches!(kind, "schedule" | "webhook" | "event" | "manual") {
            return Err(StoreError::Invalid(format!("unknown trigger kind: {kind}")));
        }
        let next_run_at = if kind == "schedule" {
            schedule_next_run(&config, Utc::now())
        } else {
            None
        };
        Ok(sqlx::query_as::<_, Trigger>(
            r#"
            INSERT INTO triggers (id, org_id, agent_id, kind, harness_kind, enabled, config, next_run_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            RETURNING *
            "#,
        )
        .bind(new_id())
        .bind(org_id)
        .bind(agent_id)
        .bind(kind)
        .bind(harness_kind)
        .bind(enabled)
        .bind(config)
        .bind(next_run_at)
        .fetch_one(&self.pool)
        .await?)
    }

    /// Enable or disable a trigger. Enabling a schedule trigger recomputes `next_run_at`
    /// from now (a stale past or NULL value would otherwise fire a burst or never fire)
    /// and clears the failure counter.
    pub async fn set_trigger_enabled(
        &self,
        org_id: Uuid,
        trigger_id: Uuid,
        enabled: bool,
    ) -> StoreResult<Trigger> {
        let current = self.get_trigger(org_id, trigger_id).await?;
        let next_run_at = if enabled && current.kind == "schedule" {
            schedule_next_run(&current.config, Utc::now())
        } else {
            current.next_run_at
        };
        sqlx::query_as::<_, Trigger>(
            r#"
            UPDATE triggers
            SET enabled = $3,
                next_run_at = $4,
                consecutive_failures = CASE WHEN $3 THEN 0 ELSE consecutive_failures END,
                updated_at = now()
            WHERE org_id = $1 AND id = $2
            RETURNING *
            "#,
        )
        .bind(org_id)
        .bind(trigger_id)
        .bind(enabled)
        .bind(next_run_at)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::NotFound("trigger".into()))
    }

    pub async fn delete_trigger(&self, org_id: Uuid, trigger_id: Uuid) -> StoreResult<()> {
        let result = sqlx::query("DELETE FROM triggers WHERE org_id = $1 AND id = $2")
            .bind(org_id)
            .bind(trigger_id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(StoreError::NotFound("trigger".into()));
        }
        Ok(())
    }

    /// Claim due schedule triggers.
    ///
    /// The claim and the next `next_run_at` are written in one transaction, so a crash can
    /// never leave an enabled trigger with a NULL `next_run_at`: either the claim rolled
    /// back (the trigger is still due and is retried) or it committed with the next
    /// occurrence already stored. The next occurrence is computed from now, so missed runs
    /// are not replayed. `FOR UPDATE SKIP LOCKED` keeps multiple replicas from claiming
    /// the same row.
    pub async fn claim_due_triggers(&self, limit: i64) -> StoreResult<Vec<Trigger>> {
        let mut tx = self.pool.begin().await?;
        let due = sqlx::query_as::<_, Trigger>(
            r#"
            SELECT * FROM triggers
            WHERE enabled AND kind = 'schedule' AND next_run_at IS NOT NULL AND next_run_at <= now()
            ORDER BY next_run_at ASC
            LIMIT $1
            FOR UPDATE SKIP LOCKED
            "#,
        )
        .bind(limit)
        .fetch_all(&mut *tx)
        .await?;

        let now = Utc::now();
        let mut claimed = Vec::with_capacity(due.len());
        for trigger in due {
            let next = schedule_next_run(&trigger.config, now);
            if next.is_none() {
                tracing::warn!(trigger_id = %trigger.id, "schedule trigger has no next occurrence");
            }
            claimed.push(
                sqlx::query_as::<_, Trigger>(
                    r#"
                    UPDATE triggers
                    SET next_run_at = $2, last_fired_at = now(), updated_at = now()
                    WHERE id = $1
                    RETURNING *
                    "#,
                )
                .bind(trigger.id)
                .bind(next)
                .fetch_one(&mut *tx)
                .await?,
            );
        }
        tx.commit().await?;
        Ok(claimed)
    }

    /// Repair enabled schedule triggers stuck with `next_run_at IS NULL` (written by
    /// versions before #111 when a crash hit between claim and reschedule). Recomputes
    /// them from now; rows whose cron/timezone is invalid or never fires stay NULL.
    /// Returns the number of repaired rows. Safe to run on every replica.
    pub async fn repair_stuck_schedule_triggers(&self) -> StoreResult<u64> {
        let mut tx = self.pool.begin().await?;
        let stuck: Vec<(Uuid, serde_json::Value)> = sqlx::query_as(
            r#"
            SELECT id, config FROM triggers
            WHERE enabled AND kind = 'schedule' AND next_run_at IS NULL
            FOR UPDATE SKIP LOCKED
            "#,
        )
        .fetch_all(&mut *tx)
        .await?;
        let now = Utc::now();
        let mut repaired = 0;
        for (id, config) in stuck {
            let Some(next) = schedule_next_run(&config, now) else {
                continue;
            };
            sqlx::query(
                "UPDATE triggers SET next_run_at = $2, updated_at = now() WHERE id = $1 AND next_run_at IS NULL",
            )
            .bind(id)
            .bind(next)
            .execute(&mut *tx)
            .await?;
            repaired += 1;
        }
        tx.commit().await?;
        Ok(repaired)
    }
}

impl Store {
    /// Record a successful trigger fire.
    pub async fn record_trigger_success(&self, trigger_id: Uuid) -> StoreResult<()> {
        sqlx::query(
            "UPDATE triggers SET consecutive_failures = 0, updated_at = now() WHERE id = $1",
        )
        .bind(trigger_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Record a failed trigger fire. Auto-disables after max_failures consecutive failures.
    pub async fn record_trigger_failure(&self, trigger_id: Uuid) -> StoreResult<bool> {
        let result = sqlx::query(
            r#"
            UPDATE triggers
            SET consecutive_failures = consecutive_failures + 1,
                enabled = CASE WHEN consecutive_failures + 1 >= max_failures THEN false ELSE enabled END,
                updated_at = now()
            WHERE id = $1
            RETURNING enabled
            "#,
        )
        .bind(trigger_id)
        .fetch_optional(&self.pool)
        .await?;
        match result {
            Some(row) => {
                let enabled: bool = sqlx::Row::get(&row, "enabled");
                Ok(!enabled) // true if disabled
            }
            None => Ok(false),
        }
    }
}
