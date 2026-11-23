use uuid::Uuid;

use crate::models::Skill;
use crate::{Store, StoreError, StoreResult};

impl Store {
    pub async fn list_skills(
        &self,
        org_id: Uuid,
        access: Option<&crate::access::ResourceAccess>,
    ) -> StoreResult<Vec<Skill>> {
        let (privileged, user_id, team_ids, isolation) = crate::access::binds(access);
        Ok(sqlx::query_as::<_, Skill>(
            r#"
            SELECT * FROM skills
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

    pub async fn get_skill(&self, org_id: Uuid, skill_id: Uuid) -> StoreResult<Skill> {
        sqlx::query_as::<_, Skill>("SELECT * FROM skills WHERE org_id = $1 AND id = $2")
            .bind(org_id)
            .bind(skill_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| StoreError::NotFound("skill".into()))
    }

    pub async fn get_skill_by_slug(&self, org_id: Uuid, slug: &str) -> StoreResult<Option<Skill>> {
        Ok(
            sqlx::query_as::<_, Skill>("SELECT * FROM skills WHERE org_id = $1 AND slug = $2")
                .bind(org_id)
                .bind(slug)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    pub async fn skills_by_slugs(&self, org_id: Uuid, slugs: &[String]) -> StoreResult<Vec<Skill>> {
        if slugs.is_empty() {
            return Ok(Vec::new());
        }
        Ok(sqlx::query_as::<_, Skill>(
            "SELECT * FROM skills WHERE org_id = $1 AND slug = ANY($2) ORDER BY name ASC",
        )
        .bind(org_id)
        .bind(slugs)
        .fetch_all(&self.pool)
        .await?)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_skill(
        &self,
        org_id: Uuid,
        slug: &str,
        name: &str,
        description: &str,
        content: &str,
        files: &serde_json::Value,
    ) -> StoreResult<Skill> {
        sqlx::query_as::<_, Skill>(
            r#"
            INSERT INTO skills (id, org_id, slug, name, description, content, files)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            RETURNING *
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(org_id)
        .bind(slug)
        .bind(name)
        .bind(description)
        .bind(content)
        .bind(files)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            if e.as_database_error()
                .map(|d| d.is_unique_violation())
                .unwrap_or(false)
            {
                StoreError::Conflict(format!("skill '{slug}' already exists"))
            } else {
                e.into()
            }
        })
    }

    pub async fn update_skill(
        &self,
        org_id: Uuid,
        skill_id: Uuid,
        name: Option<&str>,
        description: Option<&str>,
        content: Option<&str>,
        files: Option<&serde_json::Value>,
    ) -> StoreResult<Skill> {
        sqlx::query_as::<_, Skill>(
            r#"
            UPDATE skills
            SET name = COALESCE($3, name),
                description = COALESCE($4, description),
                content = COALESCE($5, content),
                files = COALESCE($6, files),
                version = version + 1,
                updated_at = now()
            WHERE org_id = $1 AND id = $2
            RETURNING *
            "#,
        )
        .bind(org_id)
        .bind(skill_id)
        .bind(name)
        .bind(description)
        .bind(content)
        .bind(files)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::NotFound("skill".into()))
    }

    /// Delete a skill that nothing references. Agents and worker profiles name skills
    /// by slug, so a deleted slug could be registered again by someone else and silently
    /// load their content into those agents (#322); a referenced skill is a conflict.
    pub async fn delete_skill(&self, org_id: Uuid, skill_id: Uuid) -> StoreResult<()> {
        let mut tx = self.begin().await?;
        Self::refuse_referenced_skill(&mut tx, org_id, skill_id).await?;
        let result = sqlx::query("DELETE FROM skills WHERE org_id = $1 AND id = $2")
            .bind(org_id)
            .bind(skill_id)
            .execute(&mut *tx)
            .await?;
        if result.rows_affected() == 0 {
            return Err(StoreError::NotFound("skill".into()));
        }
        tx.commit().await?;
        Ok(())
    }

    /// Agents (current version), agent teams and worker profiles that reference the
    /// skill's slug.
    async fn refuse_referenced_skill(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        org_id: Uuid,
        skill_id: Uuid,
    ) -> StoreResult<()> {
        let referrers: Vec<String> = sqlx::query_scalar(
            r#"
            WITH target AS (SELECT slug FROM skills WHERE org_id = $1 AND id = $2)
            SELECT 'agent ' || a.name
            FROM agents a
            JOIN agent_versions v ON v.agent_id = a.id AND v.version = a.current_version
            JOIN target t ON v.spec -> 'skills' ? t.slug
            WHERE a.org_id = $1
            UNION ALL
            SELECT 'agent team ' || tm.name
            FROM agent_teams tm
            JOIN target t ON tm.definition -> 'skills' ? t.slug
            WHERE tm.org_id = $1
            UNION ALL
            SELECT 'worker profile ' || p.name
            FROM worker_profiles p
            JOIN target t ON t.slug = ANY(p.skill_slugs)
            WHERE p.org_id = $1
            ORDER BY 1
            LIMIT 5
            "#,
        )
        .bind(org_id)
        .bind(skill_id)
        .fetch_all(&mut **tx)
        .await?;
        if referrers.is_empty() {
            return Ok(());
        }
        Err(StoreError::Conflict(format!(
            "skill is still used by {}; remove it there first",
            referrers.join(", ")
        )))
    }

    /// Check delete eligibility with the same SQL as `delete_skill`, then roll back.
    pub async fn check_delete_skill(&self, org_id: Uuid, skill_id: Uuid) -> StoreResult<()> {
        let mut tx = self.begin().await?;
        Self::refuse_referenced_skill(&mut tx, org_id, skill_id).await?;
        let result = sqlx::query("DELETE FROM skills WHERE org_id = $1 AND id = $2")
            .bind(org_id)
            .bind(skill_id)
            .execute(&mut *tx)
            .await?;
        if result.rows_affected() == 0 {
            return Err(StoreError::NotFound("skill".into()));
        }
        tx.rollback().await?;
        Ok(())
    }
}
