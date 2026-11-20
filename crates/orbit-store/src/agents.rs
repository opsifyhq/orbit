use chrono::Utc;
use serde_json::Value;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use orbit_domain::{new_id, slugify, AgentDefinition, AgentStatus, ValidationReport};

use crate::models::{Agent, AgentVersion, AgentWithSpec};
use crate::triggers::sync_agent_triggers;
use crate::{decode_cursor, page_of, Store, StoreError, StoreResult};

impl Store {
    /// Create an agent and its first immutable version.
    pub async fn create_agent(
        &self,
        org_id: Uuid,
        actor: Option<Uuid>,
        definition: &AgentDefinition,
    ) -> StoreResult<(Agent, AgentVersion)> {
        let mut tx = self.begin().await?;
        let slug = self
            .unique_agent_slug(&mut tx, org_id, &slugify(&definition.metadata.name))
            .await?;
        let checksum = definition.checksum();
        let spec_json = serde_json::to_value(&definition.spec)
            .map_err(|e| StoreError::Internal(e.to_string()))?;

        let agent = sqlx::query_as::<_, Agent>(
            r#"
            INSERT INTO agents (id, org_id, slug, name, description, status, current_version, labels, created_by)
            VALUES ($1, $2, $3, $4, $5, 'draft', 1, $6, $7)
            RETURNING *
            "#,
        )
        .bind(new_id())
        .bind(org_id)
        .bind(&slug)
        .bind(&definition.metadata.name)
        .bind(&definition.metadata.description)
        .bind(serde_json::to_value(&definition.metadata.labels).unwrap_or_default())
        .bind(actor)
        .fetch_one(&mut *tx)
        .await?;

        let version = sqlx::query_as::<_, AgentVersion>(
            r#"
            INSERT INTO agent_versions (id, org_id, agent_id, version, spec, definition_yaml, checksum, created_by)
            VALUES ($1, $2, $3, 1, $4, $5, $6, $7)
            RETURNING *
            "#,
        )
        .bind(new_id())
        .bind(org_id)
        .bind(agent.id)
        .bind(&spec_json)
        .bind(definition.to_yaml().unwrap_or_default())
        .bind(&checksum)
        .bind(actor)
        .fetch_one(&mut *tx)
        .await?;

        sync_agent_triggers(&mut tx, org_id, agent.id, &definition.spec.triggers).await?;
        tx.commit().await?;
        Ok((agent, version))
    }

    async fn unique_agent_slug(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        org_id: Uuid,
        base: &str,
    ) -> StoreResult<String> {
        let mut candidate = base.to_string();
        for attempt in 2..100 {
            let exists: Option<(Uuid,)> =
                sqlx::query_as("SELECT id FROM agents WHERE org_id = $1 AND slug = $2")
                    .bind(org_id)
                    .bind(&candidate)
                    .fetch_optional(&mut **tx)
                    .await?;
            if exists.is_none() {
                return Ok(candidate);
            }
            candidate = format!("{base}-{attempt}");
        }
        Err(StoreError::Conflict(
            "could not allocate a unique agent slug".into(),
        ))
    }

    pub async fn get_agent(&self, org_id: Uuid, agent_id: Uuid) -> StoreResult<Agent> {
        sqlx::query_as::<_, Agent>("SELECT * FROM agents WHERE org_id = $1 AND id = $2")
            .bind(org_id)
            .bind(agent_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| StoreError::NotFound("agent".into()))
    }

    pub async fn get_agent_version(
        &self,
        org_id: Uuid,
        agent_id: Uuid,
        version: i32,
    ) -> StoreResult<AgentVersion> {
        sqlx::query_as::<_, AgentVersion>(
            "SELECT * FROM agent_versions WHERE org_id = $1 AND agent_id = $2 AND version = $3",
        )
        .bind(org_id)
        .bind(agent_id)
        .bind(version)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::NotFound("agent version".into()))
    }

    pub async fn get_current_version(
        &self,
        org_id: Uuid,
        agent: &Agent,
    ) -> StoreResult<AgentVersion> {
        self.get_agent_version(org_id, agent.id, agent.current_version)
            .await
    }

    /// List agents with their current spec in one query.
    #[allow(clippy::type_complexity)]
    pub async fn list_agents(
        &self,
        org_id: Uuid,
        limit: i64,
        cursor: Option<&str>,
        status: Option<&str>,
        query: Option<&str>,
        access: Option<&crate::access::ResourceAccess>,
    ) -> StoreResult<(Vec<AgentWithSpec>, Option<String>, i64)> {
        let (privileged, user_id, team_ids, isolation) = crate::access::binds(access);
        let (cursor_ts, cursor_id) = match cursor {
            Some(raw) => match decode_cursor(raw)? {
                Some((ts, id)) => (Some(ts), Some(id)),
                None => (None, None),
            },
            None => (None, None),
        };
        let rows = sqlx::query_as::<_, AgentWithSpec>(
            r#"
            SELECT a.*, av.spec
            FROM agents a
            LEFT JOIN agent_versions av ON av.agent_id = a.id AND av.version = a.current_version
            WHERE a.org_id = $1
              AND ($2::text IS NULL OR a.status = $2)
              AND ($3::text IS NULL OR a.name ILIKE '%' || $3 || '%' OR a.description ILIKE '%' || $3 || '%')
              AND ($4::timestamptz IS NULL OR (a.created_at, a.id) < ($4::timestamptz, $5::uuid))
              AND ($7::bool OR a.owner_user_id = $8
                   OR CASE WHEN $10::bool AND a.owner_team_id IS NOT NULL
                        THEN a.owner_team_id = ANY($9)
                        ELSE a.visibility = 'org'
                             OR (a.visibility = 'team' AND a.owner_team_id = ANY($9)) END)
            ORDER BY a.created_at DESC, a.id DESC
            LIMIT $6
            "#,
        )
        .bind(org_id)
        .bind(status)
        .bind(query)
        .bind(cursor_ts)
        .bind(cursor_id)
        .bind(limit + 1)
        .bind(privileged)
        .bind(user_id)
        .bind(&team_ids)
        .bind(isolation)
        .fetch_all(&self.pool)
        .await?;

        let total: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*) FROM agents a
            WHERE a.org_id = $1
              AND ($2::text IS NULL OR a.status = $2)
              AND ($3::text IS NULL OR a.name ILIKE '%' || $3 || '%' OR a.description ILIKE '%' || $3 || '%')
              AND ($4::bool OR a.owner_user_id = $5
                   OR CASE WHEN $7::bool AND a.owner_team_id IS NOT NULL
                        THEN a.owner_team_id = ANY($6)
                        ELSE a.visibility = 'org'
                             OR (a.visibility = 'team' AND a.owner_team_id = ANY($6)) END)
            "#,
        )
        .bind(org_id)
        .bind(status)
        .bind(query)
        .bind(privileged)
        .bind(user_id)
        .bind(&team_ids)
        .bind(isolation)
        .fetch_one(&self.pool)
        .await?;

        let (rows, next) = page_of(rows, limit, |row| (row.agent.created_at, row.agent.id));
        Ok((rows, next, total))
    }

    pub async fn update_agent_metadata(
        &self,
        org_id: Uuid,
        agent_id: Uuid,
        name: Option<&str>,
        description: Option<&str>,
        labels: Option<&Value>,
    ) -> StoreResult<Agent> {
        sqlx::query_as::<_, Agent>(
            r#"
            UPDATE agents
            SET name = COALESCE($3, name),
                description = COALESCE($4, description),
                labels = COALESCE($5, labels),
                updated_at = now()
            WHERE org_id = $1 AND id = $2 AND status <> 'archived'
            RETURNING *
            "#,
        )
        .bind(org_id)
        .bind(agent_id)
        .bind(name)
        .bind(description)
        .bind(labels)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::NotFound("agent".into()))
    }

    /// Append an immutable version. Rejects a spec identical to the current version
    /// (same checksum); one identical to an older version is appended as the next
    /// version, so an agent can return to an earlier configuration.
    pub async fn add_agent_version(
        &self,
        org_id: Uuid,
        agent_id: Uuid,
        actor: Option<Uuid>,
        definition: &AgentDefinition,
    ) -> StoreResult<(Agent, AgentVersion)> {
        let mut tx = self.begin().await?;
        let current = sqlx::query_as::<_, Agent>(
            "SELECT * FROM agents WHERE org_id = $1 AND id = $2 FOR UPDATE",
        )
        .bind(org_id)
        .bind(agent_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| StoreError::NotFound("agent".into()))?;
        if current.status == "archived" {
            return Err(StoreError::Invalid(
                "archived agents cannot be modified".into(),
            ));
        }

        let checksum = definition.checksum();
        let unchanged: Option<(Uuid,)> = sqlx::query_as(
            "SELECT id FROM agent_versions WHERE agent_id = $1 AND version = $2 AND checksum = $3",
        )
        .bind(agent_id)
        .bind(current.current_version)
        .bind(&checksum)
        .fetch_optional(&mut *tx)
        .await?;
        if unchanged.is_some() {
            return Err(StoreError::Conflict(
                "nothing changed: this configuration is already the current version".into(),
            ));
        }

        let next_version = current.current_version + 1;
        let spec_json = serde_json::to_value(&definition.spec)
            .map_err(|e| StoreError::Internal(e.to_string()))?;

        let version = sqlx::query_as::<_, AgentVersion>(
            r#"
            INSERT INTO agent_versions (id, org_id, agent_id, version, spec, definition_yaml, checksum, created_by)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            RETURNING *
            "#,
        )
        .bind(new_id())
        .bind(org_id)
        .bind(agent_id)
        .bind(next_version)
        .bind(&spec_json)
        .bind(definition.to_yaml().unwrap_or_default())
        .bind(&checksum)
        .bind(actor)
        .fetch_one(&mut *tx)
        .await?;

        let agent = sqlx::query_as::<_, Agent>(
            r#"
            UPDATE agents
            SET current_version = $3, name = $4, description = $5, updated_at = now()
            WHERE org_id = $1 AND id = $2
            RETURNING *
            "#,
        )
        .bind(org_id)
        .bind(agent_id)
        .bind(next_version)
        .bind(&definition.metadata.name)
        .bind(&definition.metadata.description)
        .fetch_one(&mut *tx)
        .await?;

        sync_agent_triggers(&mut tx, org_id, agent_id, &definition.spec.triggers).await?;
        tx.commit().await?;
        Ok((agent, version))
    }

    pub async fn list_agent_versions(
        &self,
        org_id: Uuid,
        agent_id: Uuid,
    ) -> StoreResult<Vec<AgentVersion>> {
        Ok(sqlx::query_as::<_, AgentVersion>(
            "SELECT * FROM agent_versions WHERE org_id = $1 AND agent_id = $2 ORDER BY version DESC",
        )
        .bind(org_id)
        .bind(agent_id)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn set_agent_status(
        &self,
        org_id: Uuid,
        agent_id: Uuid,
        target: AgentStatus,
    ) -> StoreResult<Agent> {
        let current = self.get_agent(org_id, agent_id).await?;
        let current_status = AgentStatus::parse(&current.status).unwrap_or(AgentStatus::Draft);
        // Archiving an already-archived agent is idempotent (no error).
        if current_status == AgentStatus::Archived && target == AgentStatus::Archived {
            return Ok(current);
        }
        let allowed = match target {
            AgentStatus::Active => current_status.can_activate(),
            AgentStatus::Paused => current_status.can_pause(),
            AgentStatus::Archived => current_status.can_archive(),
            AgentStatus::Draft => false,
        };
        if !allowed {
            return Err(StoreError::Invalid(format!(
                "cannot transition agent from {} to {}",
                current_status.as_str(),
                target.as_str()
            )));
        }
        if target == AgentStatus::Active && current.current_version < 1 {
            return Err(StoreError::Invalid(
                "agent has no configuration version to activate".into(),
            ));
        }
        let archived_at = (target == AgentStatus::Archived).then(Utc::now);
        sqlx::query_as::<_, Agent>(
            r#"
            UPDATE agents
            SET status = $3, archived_at = COALESCE($4, archived_at), updated_at = now()
            WHERE org_id = $1 AND id = $2 AND status = $5
            RETURNING *
            "#,
        )
        .bind(org_id)
        .bind(agent_id)
        .bind(target.as_str())
        .bind(archived_at)
        .bind(current_status.as_str())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::Conflict("agent status changed concurrently; retry".into()))
    }

    /// Permanently delete an agent together with its versions, executions and other
    /// agent-owned history. Refused with a `Conflict` while team run steps still reference
    /// the agent: that history belongs to the team, not to the agent.
    pub async fn delete_agent(&self, org_id: Uuid, agent_id: Uuid) -> StoreResult<()> {
        let result = sqlx::query("DELETE FROM agents WHERE org_id = $1 AND id = $2")
            .bind(org_id)
            .bind(agent_id)
            .execute(&self.pool)
            .await
            .map_err(agent_delete_error)?;
        if result.rows_affected() == 0 {
            return Err(StoreError::NotFound("agent".into()));
        }
        Ok(())
    }

    /// Check delete eligibility using the same SQL (and therefore the same FK rules) as
    /// `delete_agent`, then roll back. Used by the bulk lifecycle preview so "cannot be
    /// deleted" reflects the real database constraints, not a hand-maintained copy.
    pub async fn check_delete_agent(&self, org_id: Uuid, agent_id: Uuid) -> StoreResult<()> {
        let mut tx = self.begin().await?;
        let result = sqlx::query("DELETE FROM agents WHERE org_id = $1 AND id = $2")
            .bind(org_id)
            .bind(agent_id)
            .execute(&mut *tx)
            .await
            .map_err(agent_delete_error)?;
        if result.rows_affected() == 0 {
            return Err(StoreError::NotFound("agent".into()));
        }
        tx.rollback().await?;
        Ok(())
    }

    /// Validation report combining domain rules with store-level checks (provider and
    /// connector existence, connection ownership).
    pub async fn validate_agent_references(
        &self,
        org_id: Uuid,
        definition: &AgentDefinition,
    ) -> StoreResult<ValidationReport> {
        let mut report = definition.validate();
        for (idx, binding) in definition.spec.connectors.iter().enumerate() {
            let connector = self
                .get_connector_by_slug(org_id, &binding.connector)
                .await?;
            if connector.is_none() {
                report.push(
                    format!("spec.connectors[{idx}].connector"),
                    format!("unknown connector '{}'", binding.connector),
                );
                continue;
            }
            if let Some(connection_id) = binding.connection_id {
                match self.get_connection(org_id, connection_id).await {
                    Ok(connection) => {
                        if Some(connection.connector_id) != connector.map(|c| c.id) {
                            report.push(
                                format!("spec.connectors[{idx}].connectionId"),
                                "connection does not belong to this connector",
                            );
                        }
                    }
                    Err(StoreError::NotFound(_)) => report.push(
                        format!("spec.connectors[{idx}].connectionId"),
                        "connection not found in this organization",
                    ),
                    Err(err) => return Err(err),
                }
            }
        }
        for (idx, skill_slug) in definition.spec.skills.iter().enumerate() {
            if self.get_skill_by_slug(org_id, skill_slug).await?.is_none() {
                report.push(
                    format!("spec.skills[{idx}]"),
                    format!("unknown skill '{skill_slug}'"),
                );
            }
        }
        if definition.spec.model.management == orbit_domain::ModelManagement::Platform {
            if let Some(provider_name) = &definition.spec.model.provider {
                if self
                    .get_provider_by_name(org_id, provider_name)
                    .await?
                    .is_none()
                {
                    report.push(
                        "spec.model.provider",
                        format!("unknown provider '{provider_name}'"),
                    );
                }
            }
        }
        Ok(report)
    }
}

impl Store {
    pub async fn list_agent_instances(
        &self,
        org_id: Uuid,
    ) -> StoreResult<Vec<crate::models::AgentInstance>> {
        Ok(sqlx::query_as::<_, crate::models::AgentInstance>(
            "SELECT * FROM agent_instances WHERE org_id = $1 ORDER BY created_at DESC LIMIT 100",
        )
        .bind(org_id)
        .fetch_all(&self.pool)
        .await?)
    }
}

impl Store {
    pub async fn get_agent_version_by_id(
        &self,
        org_id: Uuid,
        version_id: Uuid,
    ) -> StoreResult<AgentVersion> {
        sqlx::query_as::<_, AgentVersion>(
            "SELECT * FROM agent_versions WHERE org_id = $1 AND id = $2",
        )
        .bind(org_id)
        .bind(version_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::NotFound("agent version".into()))
    }
}

impl Store {
    pub async fn count_agents(&self, org_id: Uuid) -> StoreResult<i64> {
        Ok(sqlx::query_scalar(
            "SELECT COUNT(*) FROM agents WHERE org_id = $1 AND status <> 'archived'",
        )
        .bind(org_id)
        .fetch_one(&self.pool)
        .await?)
    }
}

/// Translate the FK violation raised by an agent delete into an actionable conflict.
fn agent_delete_error(error: sqlx::Error) -> StoreError {
    let constraint = error
        .as_database_error()
        .filter(|db| db.is_foreign_key_violation())
        .and_then(|db| db.constraint().map(str::to_owned));
    match constraint.as_deref() {
        Some(name) if name.starts_with("team_steps_") => StoreError::Conflict(
            "agent is used by agent team runs; delete those runs or archive the agent instead"
                .into(),
        ),
        Some(_) => StoreError::Conflict(
            "agent is still referenced by other records; archive it instead".into(),
        ),
        None => error.into(),
    }
}
