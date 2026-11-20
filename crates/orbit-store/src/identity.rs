use chrono::Utc;
use uuid::Uuid;

use orbit_domain::{new_id, OrganizationMode, UserRole};

use crate::models::{ApiTokenRecord, ApiTokenView, Organization, User};
use crate::{Store, StoreError, StoreResult};

/// Refusal messages for OIDC email linking (surfaced to the user as a 403).
pub const EMAIL_LINKED_ELSEWHERE: &str = "this email is already linked to another sign-in identity";
pub const EMAIL_NOT_VERIFIED: &str =
    "this email is not verified by the identity provider, so it cannot be linked to an existing user";

impl Store {
    // ---------------------------------------------------------------- organizations

    pub async fn create_organization(
        &self,
        slug: &str,
        name: &str,
        mode: OrganizationMode,
    ) -> StoreResult<Organization> {
        let id = new_id();
        let org = sqlx::query_as::<_, Organization>(
            r#"
            INSERT INTO organizations (id, slug, name, mode)
            VALUES ($1, $2, $3, $4)
            RETURNING *
            "#,
        )
        .bind(id)
        .bind(slug)
        .bind(name)
        .bind(mode.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            if is_unique(&e) {
                StoreError::Conflict(format!("organization slug '{slug}' already exists"))
            } else {
                e.into()
            }
        })?;
        self.ensure_default_access_team(org.id).await?;
        Ok(org)
    }

    pub async fn get_organization(&self, org_id: Uuid) -> StoreResult<Organization> {
        sqlx::query_as::<_, Organization>("SELECT * FROM organizations WHERE id = $1")
            .bind(org_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| StoreError::NotFound("organization".into()))
    }

    /// The first organization is the default tenant in single-tenant self-hosted setups.
    pub async fn first_organization(&self) -> StoreResult<Option<Organization>> {
        Ok(sqlx::query_as::<_, Organization>(
            "SELECT * FROM organizations ORDER BY created_at ASC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?)
    }

    // ---------------------------------------------------------------------- users

    pub async fn create_user(
        &self,
        org_id: Uuid,
        email: &str,
        display_name: &str,
        role: UserRole,
    ) -> StoreResult<User> {
        let user = sqlx::query_as::<_, User>(
            r#"
            INSERT INTO users (id, org_id, email, display_name, role)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING *
            "#,
        )
        .bind(new_id())
        .bind(org_id)
        .bind(email.trim().to_lowercase())
        .bind(display_name)
        .bind(role.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            if is_unique(&e) {
                StoreError::Conflict(format!("user '{email}' already exists in organization"))
            } else {
                e.into()
            }
        })?;
        Ok(user)
    }

    /// Idempotent bootstrap used in dev mode and for the initial owner account.
    pub async fn ensure_user(
        &self,
        org_id: Uuid,
        email: &str,
        display_name: &str,
        role: UserRole,
    ) -> StoreResult<User> {
        let email = email.trim().to_lowercase();
        if let Some(existing) = self.get_user_by_email(org_id, &email).await? {
            return Ok(existing);
        }
        match self.create_user(org_id, &email, display_name, role).await {
            Ok(user) => Ok(user),
            Err(StoreError::Conflict(_)) => self
                .get_user_by_email(org_id, &email)
                .await?
                .ok_or_else(|| StoreError::Internal("user upsert race".into())),
            Err(other) => Err(other),
        }
    }

    pub async fn get_user(&self, org_id: Uuid, user_id: Uuid) -> StoreResult<User> {
        sqlx::query_as::<_, User>("SELECT * FROM users WHERE org_id = $1 AND id = $2")
            .bind(org_id)
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| StoreError::NotFound("user".into()))
    }

    pub async fn get_user_by_email(&self, org_id: Uuid, email: &str) -> StoreResult<Option<User>> {
        Ok(sqlx::query_as::<_, User>(
            "SELECT * FROM users WHERE org_id = $1 AND lower(email) = lower($2)",
        )
        .bind(org_id)
        .bind(email)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn get_user_by_oidc(&self, issuer: &str, subject: &str) -> StoreResult<Option<User>> {
        Ok(sqlx::query_as::<_, User>(
            "SELECT * FROM users WHERE oidc_issuer = $1 AND oidc_subject = $2",
        )
        .bind(issuer)
        .bind(subject)
        .fetch_optional(&self.pool)
        .await?)
    }

    /// Provision or update an OIDC user in the default organization.
    ///
    /// An unknown `(issuer, subject)` is linked to an existing user with the same email only
    /// when that user has no OIDC identity yet (e.g. invited by an admin) and
    /// `link_by_email` is true. A user already bound to another identity, or a link that the
    /// caller disallowed (unverified email), yields [`StoreError::Conflict`]; nothing changes.
    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_oidc_user(
        &self,
        org_id: Uuid,
        issuer: &str,
        subject: &str,
        email: Option<&str>,
        name: Option<&str>,
        default_role: UserRole,
        link_by_email: bool,
    ) -> StoreResult<User> {
        if let Some(user) = self.get_user_by_oidc(issuer, subject).await? {
            return Self::active_oidc_user(user);
        }
        let email = email
            .filter(|e| !e.trim().is_empty())
            .unwrap_or(subject)
            .to_lowercase();
        let display_name = name.unwrap_or(&email).to_string();

        // If a matching email already exists (e.g. invited by an admin), link the identity.
        if let Some(existing) = self.get_user_by_email(org_id, &email).await? {
            if existing.oidc_subject.is_some() {
                return Err(StoreError::Conflict(EMAIL_LINKED_ELSEWHERE.into()));
            }
            if !link_by_email {
                return Err(StoreError::Conflict(EMAIL_NOT_VERIFIED.into()));
            }
            let linked = sqlx::query_as::<_, User>(
                r#"
                UPDATE users SET oidc_issuer = $3, oidc_subject = $4, updated_at = now()
                WHERE org_id = $1 AND id = $2 AND oidc_subject IS NULL
                RETURNING *
                "#,
            )
            .bind(org_id)
            .bind(existing.id)
            .bind(issuer)
            .bind(subject)
            .fetch_optional(&self.pool)
            .await?;
            // No row: another login bound this user between our read and the update.
            return match linked {
                Some(user) => Ok(user),
                None => match self.get_user_by_oidc(issuer, subject).await? {
                    Some(user) => Self::active_oidc_user(user),
                    None => Err(StoreError::Conflict(EMAIL_LINKED_ELSEWHERE.into())),
                },
            };
        }

        let inserted = sqlx::query_as::<_, User>(
            r#"
            INSERT INTO users (id, org_id, email, display_name, role, oidc_issuer, oidc_subject)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            RETURNING *
            "#,
        )
        .bind(new_id())
        .bind(org_id)
        .bind(&email)
        .bind(&display_name)
        .bind(default_role.as_str())
        .bind(issuer)
        .bind(subject)
        .fetch_one(&self.pool)
        .await
        .map_err(StoreError::from);
        match inserted {
            Ok(user) => Ok(user),
            // A concurrent first login for the same identity won the race: use its row.
            // The unique violation is reported after the other insert commits, but a
            // separate pool connection can briefly take a snapshot before that row is
            // visible. Retry the lookup for a few milliseconds instead of turning the
            // losing side of a legitimate concurrent login into a Conflict.
            Err(err) if err.is_unique_violation() => {
                for attempt in 0..5 {
                    if let Some(user) = self.get_user_by_oidc(issuer, subject).await? {
                        return Self::active_oidc_user(user);
                    }
                    if attempt < 4 {
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    }
                }
                // The collision was on the email (another identity or invite created it).
                Err(StoreError::Conflict(EMAIL_LINKED_ELSEWHERE.into()))
            }
            Err(err) => Err(err),
        }
    }

    fn active_oidc_user(user: User) -> StoreResult<User> {
        if user.status != "active" {
            return Err(StoreError::Invalid("user is disabled".into()));
        }
        Ok(user)
    }

    pub async fn list_users(&self, org_id: Uuid) -> StoreResult<Vec<User>> {
        Ok(sqlx::query_as::<_, User>(
            "SELECT * FROM users WHERE org_id = $1 ORDER BY created_at ASC",
        )
        .bind(org_id)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn set_user_role(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        role: UserRole,
    ) -> StoreResult<User> {
        sqlx::query_as::<_, User>(
            "UPDATE users SET role = $3, updated_at = now() WHERE org_id = $1 AND id = $2 RETURNING *",
        )
        .bind(org_id)
        .bind(user_id)
        .bind(role.as_str())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::NotFound("user".into()))
    }

    // ----------------------------------------------------------------- api tokens

    #[allow(clippy::too_many_arguments)]
    pub async fn create_api_token(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        name: &str,
        token_prefix: &str,
        token_hash: &str,
        scopes: &[String],
        expires_at: Option<chrono::DateTime<Utc>>,
    ) -> StoreResult<ApiTokenView> {
        let record = sqlx::query_as::<_, ApiTokenRecord>(
            r#"
            INSERT INTO api_tokens (id, org_id, user_id, name, token_prefix, token_hash, scopes, expires_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            RETURNING *
            "#,
        )
        .bind(new_id())
        .bind(org_id)
        .bind(user_id)
        .bind(name)
        .bind(token_prefix)
        .bind(token_hash)
        .bind(scopes)
        .bind(expires_at)
        .fetch_one(&self.pool)
        .await?;
        Ok(record.into())
    }

    pub async fn find_api_token_by_hash(
        &self,
        token_hash: &str,
    ) -> StoreResult<Option<ApiTokenRecord>> {
        Ok(sqlx::query_as::<_, ApiTokenRecord>(
            "SELECT * FROM api_tokens WHERE token_hash = $1 AND revoked_at IS NULL",
        )
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn touch_api_token(&self, id: Uuid) -> StoreResult<()> {
        sqlx::query("UPDATE api_tokens SET last_used_at = now() WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn list_api_tokens(&self, org_id: Uuid) -> StoreResult<Vec<ApiTokenView>> {
        let records = sqlx::query_as::<_, ApiTokenRecord>(
            "SELECT * FROM api_tokens WHERE org_id = $1 ORDER BY created_at DESC",
        )
        .bind(org_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(records.into_iter().map(Into::into).collect())
    }

    pub async fn revoke_api_token(&self, org_id: Uuid, token_id: Uuid) -> StoreResult<()> {
        let result = sqlx::query(
            "UPDATE api_tokens SET revoked_at = now() WHERE org_id = $1 AND id = $2 AND revoked_at IS NULL",
        )
        .bind(org_id)
        .bind(token_id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(StoreError::NotFound("api token".into()));
        }
        Ok(())
    }
}

fn is_unique(err: &sqlx::Error) -> bool {
    err.as_database_error()
        .map(|db| db.is_unique_violation())
        .unwrap_or(false)
}

impl Store {
    pub async fn count_active_users(&self, org_id: Uuid) -> StoreResult<i64> {
        Ok(
            sqlx::query_scalar(
                "SELECT COUNT(*) FROM users WHERE org_id = $1 AND status = 'active'",
            )
            .bind(org_id)
            .fetch_one(&self.pool)
            .await?,
        )
    }
}
