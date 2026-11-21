use serde_json::Value;
use uuid::Uuid;

use orbit_contracts::{ConnectorManifest, HarnessDescriptor};
use orbit_domain::new_id;

use crate::models::{
    Connection, ConnectionWithConnector, ConnectorRecord, HarnessRecord, Provider,
};
use crate::{Store, StoreError, StoreResult};

impl Store {
    // -------------------------------------------------------------------- harnesses

    /// Upsert the built-in harness catalog from the adapter registry.
    pub async fn upsert_harness_descriptor(
        &self,
        descriptor: &HarnessDescriptor,
    ) -> StoreResult<HarnessRecord> {
        let capabilities = serde_json::to_value(&descriptor.capabilities)
            .map_err(|e| StoreError::Internal(e.to_string()))?;
        let config_schema = descriptor.config_schema.clone();
        sqlx::query_as::<_, HarnessRecord>(
            r#"
            INSERT INTO harnesses (id, org_id, kind, name, description, enabled, implementation_status, capabilities, config)
            VALUES ($1, NULL, $2, $3, $4, true, $5, $6, $7)
            ON CONFLICT (kind) WHERE org_id IS NULL
            DO UPDATE SET
                name = EXCLUDED.name,
                description = EXCLUDED.description,
                implementation_status = EXCLUDED.implementation_status,
                capabilities = EXCLUDED.capabilities,
                config = EXCLUDED.config,
                updated_at = now()
            RETURNING *
            "#,
        )
        .bind(new_id())
        .bind(descriptor.kind.as_str())
        .bind(&descriptor.name)
        .bind(&descriptor.description)
        .bind(descriptor.implementation_status.as_str())
        .bind(capabilities)
        .bind(config_schema)
        .fetch_one(&self.pool)
        .await
        .map_err(Into::into)
    }

    pub async fn list_harnesses(&self, org_id: Uuid) -> StoreResult<Vec<HarnessRecord>> {
        Ok(sqlx::query_as::<_, HarnessRecord>(
            r#"
            SELECT * FROM harnesses
            WHERE org_id IS NULL OR org_id = $1
            ORDER BY kind ASC
            "#,
        )
        .bind(org_id)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn get_harness_by_kind(
        &self,
        org_id: Uuid,
        kind: &str,
    ) -> StoreResult<Option<HarnessRecord>> {
        Ok(sqlx::query_as::<_, HarnessRecord>(
            r#"
            SELECT * FROM harnesses
            WHERE (org_id IS NULL OR org_id = $1) AND kind = $2
            ORDER BY org_id NULLS LAST
            LIMIT 1
            "#,
        )
        .bind(org_id)
        .bind(kind)
        .fetch_optional(&self.pool)
        .await?)
    }

    // ------------------------------------------------------------------- providers

    pub async fn create_provider(
        &self,
        org_id: Uuid,
        name: &str,
        kind: &str,
        config: &Value,
        secret_ref: Option<&str>,
        enabled: bool,
    ) -> StoreResult<Provider> {
        sqlx::query_as::<_, Provider>(
            r#"
            INSERT INTO providers (id, org_id, name, kind, config, secret_ref, enabled)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            RETURNING *
            "#,
        )
        .bind(new_id())
        .bind(org_id)
        .bind(name)
        .bind(kind)
        .bind(config)
        .bind(secret_ref)
        .bind(enabled)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            if e.as_database_error()
                .map(|d| d.is_unique_violation())
                .unwrap_or(false)
            {
                StoreError::Conflict(format!("provider '{name}' already exists"))
            } else {
                e.into()
            }
        })
    }

    pub async fn list_providers(
        &self,
        org_id: Uuid,
        access: Option<&crate::access::ResourceAccess>,
    ) -> StoreResult<Vec<Provider>> {
        let (privileged, user_id, team_ids, isolation) = crate::access::binds(access);
        Ok(sqlx::query_as::<_, Provider>(
            r#"
            SELECT * FROM providers
            WHERE org_id = $1
              AND ($2::bool OR owner_user_id = $3
                   OR CASE WHEN $5::bool AND owner_team_id IS NOT NULL
                        THEN owner_team_id = ANY($4)
                        ELSE visibility = 'org'
                             OR (visibility = 'team' AND owner_team_id = ANY($4)) END)
            ORDER BY created_at DESC
            "#,
        )
        .bind(org_id)
        .bind(privileged)
        .bind(user_id)
        .bind(&team_ids)
        .bind(isolation)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn get_provider(&self, org_id: Uuid, provider_id: Uuid) -> StoreResult<Provider> {
        sqlx::query_as::<_, Provider>("SELECT * FROM providers WHERE org_id = $1 AND id = $2")
            .bind(org_id)
            .bind(provider_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| StoreError::NotFound("provider".into()))
    }

    pub async fn get_provider_by_name(
        &self,
        org_id: Uuid,
        name: &str,
    ) -> StoreResult<Option<Provider>> {
        Ok(
            sqlx::query_as::<_, Provider>(
                "SELECT * FROM providers WHERE org_id = $1 AND name = $2",
            )
            .bind(org_id)
            .bind(name)
            .fetch_optional(&self.pool)
            .await?,
        )
    }

    pub async fn update_provider(
        &self,
        org_id: Uuid,
        provider_id: Uuid,
        name: Option<&str>,
        config: Option<&Value>,
        secret_ref: Option<&str>,
        enabled: Option<bool>,
    ) -> StoreResult<Provider> {
        sqlx::query_as::<_, Provider>(
            r#"
            UPDATE providers
            SET name = COALESCE($3, name),
                config = COALESCE($4, config),
                secret_ref = COALESCE($5, secret_ref),
                enabled = COALESCE($6, enabled),
                updated_at = now()
            WHERE org_id = $1 AND id = $2
            RETURNING *
            "#,
        )
        .bind(org_id)
        .bind(provider_id)
        .bind(name)
        .bind(config)
        .bind(secret_ref)
        .bind(enabled)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::NotFound("provider".into()))
    }

    pub async fn delete_provider(&self, org_id: Uuid, provider_id: Uuid) -> StoreResult<()> {
        let result = sqlx::query("DELETE FROM providers WHERE org_id = $1 AND id = $2")
            .bind(org_id)
            .bind(provider_id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(StoreError::NotFound("provider".into()));
        }
        Ok(())
    }

    // ------------------------------------------------------------------ connectors

    pub async fn upsert_connector_manifest(
        &self,
        manifest: &ConnectorManifest,
    ) -> StoreResult<ConnectorRecord> {
        let auth = serde_json::to_value(&manifest.auth).unwrap_or_default();
        let capabilities = serde_json::to_value(&manifest.capabilities).unwrap_or_default();
        sqlx::query_as::<_, ConnectorRecord>(
            r#"
            INSERT INTO connectors (id, org_id, slug, name, version, description, auth, capabilities,
                                    config_schema, webhooks, builtin, docs_url)
            VALUES ($1, NULL, $2, $3, $4, $5, $6, $7, $8, $9, true, $10)
            ON CONFLICT (slug) WHERE org_id IS NULL
            DO UPDATE SET
                name = EXCLUDED.name,
                version = EXCLUDED.version,
                description = EXCLUDED.description,
                auth = EXCLUDED.auth,
                capabilities = EXCLUDED.capabilities,
                config_schema = EXCLUDED.config_schema,
                webhooks = EXCLUDED.webhooks,
                docs_url = EXCLUDED.docs_url
            RETURNING *
            "#,
        )
        .bind(new_id())
        .bind(&manifest.slug)
        .bind(&manifest.name)
        .bind(&manifest.version)
        .bind(&manifest.description)
        .bind(auth)
        .bind(capabilities)
        .bind(&manifest.config_schema)
        .bind(manifest.webhooks)
        .bind(&manifest.docs_url)
        .fetch_one(&self.pool)
        .await
        .map_err(Into::into)
    }

    pub async fn list_connectors(&self, org_id: Uuid) -> StoreResult<Vec<ConnectorRecord>> {
        Ok(sqlx::query_as::<_, ConnectorRecord>(
            r#"
            SELECT * FROM connectors
            WHERE org_id IS NULL OR org_id = $1
            ORDER BY name ASC
            "#,
        )
        .bind(org_id)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn get_connector_by_slug(
        &self,
        org_id: Uuid,
        slug: &str,
    ) -> StoreResult<Option<ConnectorRecord>> {
        Ok(sqlx::query_as::<_, ConnectorRecord>(
            r#"
            SELECT * FROM connectors
            WHERE (org_id IS NULL OR org_id = $1) AND slug = $2
            ORDER BY org_id NULLS LAST
            LIMIT 1
            "#,
        )
        .bind(org_id)
        .bind(slug)
        .fetch_optional(&self.pool)
        .await?)
    }

    // ----------------------------------------------------------------- connections

    #[allow(clippy::too_many_arguments)]
    pub async fn create_connection(
        &self,
        org_id: Uuid,
        connector_id: Uuid,
        name: &str,
        config: &Value,
        scopes: &[String],
        credential_ref: Option<&str>,
        created_by: Option<Uuid>,
    ) -> StoreResult<Connection> {
        sqlx::query_as::<_, Connection>(
            r#"
            INSERT INTO connections (id, org_id, connector_id, name, config, scopes, credential_ref, created_by)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            RETURNING *
            "#,
        )
        .bind(new_id())
        .bind(org_id)
        .bind(connector_id)
        .bind(name)
        .bind(config)
        .bind(scopes)
        .bind(credential_ref)
        .bind(created_by)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            if e.as_database_error().map(|d| d.is_unique_violation()).unwrap_or(false) {
                StoreError::Conflict(format!("connection '{name}' already exists"))
            } else {
                e.into()
            }
        })
    }

    #[allow(clippy::type_complexity)]
    pub async fn list_connections(
        &self,
        org_id: Uuid,
        access: Option<&crate::access::ResourceAccess>,
    ) -> StoreResult<Vec<ConnectionWithConnector>> {
        let (privileged, user_id, team_ids, isolation) = crate::access::binds(access);
        Ok(sqlx::query_as::<_, ConnectionWithConnector>(
            r#"
            SELECT c.*, conn.slug AS connector_slug, conn.name AS connector_name
            FROM connections c
            JOIN connectors conn ON conn.id = c.connector_id
            WHERE c.org_id = $1
              AND ($2::bool OR c.owner_user_id = $3
                   OR CASE WHEN $5::bool AND c.owner_team_id IS NOT NULL
                        THEN c.owner_team_id = ANY($4)
                        ELSE c.visibility = 'org'
                             OR (c.visibility = 'team' AND c.owner_team_id = ANY($4)) END)
            ORDER BY c.created_at DESC
            "#,
        )
        .bind(org_id)
        .bind(privileged)
        .bind(user_id)
        .bind(&team_ids)
        .bind(isolation)
        .fetch_all(&self.pool)
        .await?)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_connection(
        &self,
        org_id: Uuid,
        connection_id: Uuid,
        name: Option<&str>,
        config: Option<&Value>,
        scopes: Option<&[String]>,
        credential_ref: Option<&str>,
    ) -> StoreResult<Connection> {
        sqlx::query_as::<_, Connection>(
            r#"
            UPDATE connections
            SET name = COALESCE($3, name),
                config = COALESCE($4, config),
                scopes = COALESCE($5, scopes),
                credential_ref = COALESCE($6, credential_ref),
                updated_at = now()
            WHERE org_id = $1 AND id = $2
            RETURNING *
            "#,
        )
        .bind(org_id)
        .bind(connection_id)
        .bind(name)
        .bind(config)
        .bind(scopes)
        .bind(credential_ref)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::NotFound("connection".into()))
    }

    pub async fn get_connection(
        &self,
        org_id: Uuid,
        connection_id: Uuid,
    ) -> StoreResult<Connection> {
        sqlx::query_as::<_, Connection>("SELECT * FROM connections WHERE org_id = $1 AND id = $2")
            .bind(org_id)
            .bind(connection_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| StoreError::NotFound("connection".into()))
    }

    pub async fn get_connection_with_connector(
        &self,
        org_id: Uuid,
        connection_id: Uuid,
    ) -> StoreResult<ConnectionWithConnector> {
        sqlx::query_as::<_, ConnectionWithConnector>(
            r#"
            SELECT c.*, conn.slug AS connector_slug, conn.name AS connector_name
            FROM connections c
            JOIN connectors conn ON conn.id = c.connector_id
            WHERE c.org_id = $1 AND c.id = $2
            "#,
        )
        .bind(org_id)
        .bind(connection_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::NotFound("connection".into()))
    }

    pub async fn delete_connection(&self, org_id: Uuid, connection_id: Uuid) -> StoreResult<()> {
        let result = sqlx::query("DELETE FROM connections WHERE org_id = $1 AND id = $2")
            .bind(org_id)
            .bind(connection_id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(StoreError::NotFound("connection".into()));
        }
        Ok(())
    }
}

impl Store {
    /// Record the outcome of a connection test. A pass returns the connection to
    /// `active` (the only status workers accept), so a failed check is recoverable
    /// by testing again; `expired` and `revoked` are never overwritten by a probe.
    pub async fn set_connection_check(
        &self,
        org_id: Uuid,
        connection_id: Uuid,
        ok: bool,
    ) -> StoreResult<()> {
        sqlx::query(
            "UPDATE connections
             SET status = CASE WHEN status IN ('active', 'error') THEN $3 ELSE status END,
                 last_checked_at = now(), updated_at = now()
             WHERE org_id = $1 AND id = $2",
        )
        .bind(org_id)
        .bind(connection_id)
        .bind(if ok { "active" } else { "error" })
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
