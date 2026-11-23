use uuid::Uuid;

use crate::models::WorkerProfile;
use crate::{Store, StoreError, StoreResult};

impl Store {
    pub async fn list_worker_profiles(
        &self,
        org_id: Uuid,
        access: Option<&crate::access::ResourceAccess>,
    ) -> StoreResult<Vec<WorkerProfile>> {
        let (privileged, user_id, team_ids, isolation) = crate::access::binds(access);
        Ok(sqlx::query_as::<_, WorkerProfile>(
            r#"
            SELECT * FROM worker_profiles
            WHERE org_id = $1
              AND ($2::bool OR owner_user_id = $3
                   OR CASE WHEN $5::bool AND owner_team_id IS NOT NULL
                        THEN owner_team_id = ANY($4)
                        ELSE visibility = 'org'
                             OR (visibility = 'team' AND owner_team_id = ANY($4)) END)
            ORDER BY name ASC
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

    pub async fn get_worker_profile(
        &self,
        org_id: Uuid,
        profile_id: Uuid,
    ) -> StoreResult<WorkerProfile> {
        sqlx::query_as::<_, WorkerProfile>(
            "SELECT * FROM worker_profiles WHERE org_id = $1 AND id = $2",
        )
        .bind(org_id)
        .bind(profile_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::NotFound("worker profile".into()))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_worker_profile(
        &self,
        org_id: Uuid,
        name: &str,
        harness_kind: &str,
        target: &str,
        execution_driver: &str,
        image: &str,
        image_tag: &str,
        agent_scope: &str,
        agent_ids: &[Uuid],
        skill_slugs: &[String],
        provider: Option<&str>,
        model: Option<&str>,
        auth_secret_name: Option<&str>,
        env: &serde_json::Value,
        env_mappings: &serde_json::Value,
        resources: &serde_json::Value,
        allow_web_search: bool,
        allow_web_fetch: bool,
        allow_glab: bool,
        allow_gh: bool,
        native_tool_permissions: &serde_json::Value,
    ) -> StoreResult<WorkerProfile> {
        sqlx::query_as::<_, WorkerProfile>(
            r#"
            INSERT INTO worker_profiles (id, org_id, name, harness_kind, target, execution_driver,
                image, image_tag, agent_scope, agent_ids, skill_slugs, provider, model, auth_secret_name, env, env_mappings, resources,
                allow_web_search, allow_web_fetch, allow_glab, allow_gh, native_tool_permissions)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22)
            RETURNING *
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(org_id)
        .bind(name)
        .bind(harness_kind)
        .bind(target)
        .bind(execution_driver)
        .bind(image)
        .bind(image_tag)
        .bind(agent_scope)
        .bind(agent_ids)
        .bind(skill_slugs)
        .bind(provider)
        .bind(model)
        .bind(auth_secret_name)
        .bind(env)
        .bind(env_mappings)
        .bind(resources)
        .bind(allow_web_search)
        .bind(allow_web_fetch)
        .bind(allow_glab)
        .bind(allow_gh)
        .bind(native_tool_permissions)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            if e.as_database_error().map(|d| d.is_unique_violation()).unwrap_or(false) {
                StoreError::Conflict(format!("worker profile '{name}' already exists"))
            } else {
                e.into()
            }
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_worker_profile(
        &self,
        org_id: Uuid,
        profile_id: Uuid,
        name: &str,
        harness_kind: &str,
        target: &str,
        execution_driver: &str,
        image: &str,
        image_tag: &str,
        agent_scope: &str,
        agent_ids: &[Uuid],
        skill_slugs: &[String],
        provider: Option<&str>,
        model: Option<&str>,
        auth_secret_name: Option<&str>,
        env: &serde_json::Value,
        env_mappings: &serde_json::Value,
        resources: &serde_json::Value,
        allow_web_search: bool,
        allow_web_fetch: bool,
        allow_glab: bool,
        allow_gh: bool,
        native_tool_permissions: &serde_json::Value,
    ) -> StoreResult<WorkerProfile> {
        sqlx::query_as::<_, WorkerProfile>(
            r#"
            UPDATE worker_profiles
            SET name = $3, harness_kind = $4, target = $5, execution_driver = $6,
                image = $7, image_tag = $8, agent_scope = $9, agent_ids = $10,
                skill_slugs = $11, provider = $12, model = $13, auth_secret_name = $14,
                env = $15, env_mappings = $16, resources = $17,
                allow_web_search = $18, allow_web_fetch = $19,
                allow_glab = $20, allow_gh = $21,
                native_tool_permissions = $22, updated_at = now()
            WHERE org_id = $1 AND id = $2
            RETURNING *
            "#,
        )
        .bind(org_id)
        .bind(profile_id)
        .bind(name)
        .bind(harness_kind)
        .bind(target)
        .bind(execution_driver)
        .bind(image)
        .bind(image_tag)
        .bind(agent_scope)
        .bind(agent_ids)
        .bind(skill_slugs)
        .bind(provider)
        .bind(model)
        .bind(auth_secret_name)
        .bind(env)
        .bind(env_mappings)
        .bind(resources)
        .bind(allow_web_search)
        .bind(allow_web_fetch)
        .bind(allow_glab)
        .bind(allow_gh)
        .bind(native_tool_permissions)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::NotFound("worker profile".into()))
    }

    pub async fn set_worker_profile_enabled(
        &self,
        org_id: Uuid,
        profile_id: Uuid,
        enabled: bool,
    ) -> StoreResult<WorkerProfile> {
        sqlx::query_as::<_, WorkerProfile>(
            "UPDATE worker_profiles SET enabled = $3, updated_at = now() WHERE org_id = $1 AND id = $2 RETURNING *",
        )
        .bind(org_id)
        .bind(profile_id)
        .bind(enabled)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::NotFound("worker profile".into()))
    }

    pub async fn delete_worker_profile(&self, org_id: Uuid, profile_id: Uuid) -> StoreResult<()> {
        let result = sqlx::query("DELETE FROM worker_profiles WHERE org_id = $1 AND id = $2")
            .bind(org_id)
            .bind(profile_id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(StoreError::NotFound("worker profile".into()));
        }
        Ok(())
    }
}

impl Store {
    /// Enabled profiles that follow the platform image tag (empty imageTag).
    pub async fn list_platform_follow_profiles(&self) -> StoreResult<Vec<WorkerProfile>> {
        Ok(sqlx::query_as::<_, WorkerProfile>(
            "SELECT * FROM worker_profiles WHERE enabled AND image_tag = '' ORDER BY created_at ASC",
        )
        .fetch_all(&self.pool)
        .await?)
    }
}
