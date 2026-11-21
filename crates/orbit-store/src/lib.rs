//! PostgreSQL persistence for ORBIT.
//!
//! The store is deliberately thin: SQL and row mapping only. Business rules live in
//! `orbit-domain`; HTTP concerns live in `orbit-api`. Every tenant-scoped query takes an
//! `org_id` and filters on it.

use std::sync::Arc;

use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use orbit_security::SecretCipher;

pub mod access;
pub mod access_teams;
pub mod agent_memory;
pub mod agent_runtimes;
pub mod agent_teams;
pub mod agents;
pub mod approvals;
pub mod audit;
pub mod catalog;
pub mod custom_roles;
pub mod dashboard;
pub mod env_mappings;
pub mod executions;
pub mod identity;
pub mod models;
pub mod outputs;
pub mod platform_settings;
pub mod portable;
pub mod secrets;
pub mod sharing;
pub mod skills;
pub mod team_queue;
pub mod tool_policies;
pub mod triggers;
pub mod worker_launches;
pub mod worker_profiles;
pub mod worker_tokens;
pub mod workers;

pub use models::*;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("invalid: {0}")]
    Invalid(String),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("internal error: {0}")]
    Internal(String),
}

impl From<orbit_security::SecurityError> for StoreError {
    fn from(err: orbit_security::SecurityError) -> Self {
        StoreError::Internal(err.to_string())
    }
}

impl StoreError {
    pub fn is_unique_violation(&self) -> bool {
        match self {
            Self::Database(err) => err
                .as_database_error()
                .map(|db| db.is_unique_violation())
                .unwrap_or(false),
            _ => false,
        }
    }

    pub fn is_foreign_key_violation(&self) -> bool {
        match self {
            Self::Database(err) => err
                .as_database_error()
                .map(|db| db.is_foreign_key_violation())
                .unwrap_or(false),
            _ => false,
        }
    }
}

pub type StoreResult<T> = Result<T, StoreError>;

/// Holds an advisory lock until dropped. See [`Store::license_limit_guard`].
pub struct LicenseLimitGuard<'a> {
    // Kept alive for its drop side effect (rollback releases the advisory lock).
    _tx: Transaction<'a, Postgres>,
}

/// Shared database handle. Cloning is cheap (`PgPool` is an `Arc` internally).
#[derive(Clone)]
pub struct Store {
    pool: PgPool,
    cipher: Option<Arc<SecretCipher>>,
}

impl Store {
    pub fn new(pool: PgPool, cipher: Option<Arc<SecretCipher>>) -> Self {
        Self { pool, cipher }
    }

    pub async fn connect(database_url: &str, max_connections: u32) -> StoreResult<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .acquire_timeout(std::time::Duration::from_secs(10))
            .connect(database_url)
            .await?;
        Ok(Self { pool, cipher: None })
    }

    pub fn with_cipher(mut self, cipher: Arc<SecretCipher>) -> Self {
        self.cipher = Some(cipher);
        self
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn migrate(&self) -> StoreResult<()> {
        sqlx::migrate!("./migrations")
            .run(&self.pool)
            .await
            .map_err(|e| StoreError::Internal(format!("migration failed: {e}")))?;
        Ok(())
    }

    pub async fn health(&self) -> StoreResult<()> {
        sqlx::query_scalar::<_, i32>("SELECT 1")
            .fetch_one(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn begin(&self) -> StoreResult<Transaction<'_, Postgres>> {
        Ok(self.pool.begin().await?)
    }

    /// Serialise a check-then-create sequence for one installation-wide license limit.
    ///
    /// The guard holds a transaction-scoped advisory lock; dropping it rolls the (otherwise
    /// empty) transaction back and releases the lock, including on early returns and errors.
    /// Every path that creates a resource counted by a license limit must hold the matching
    /// guard across both the count and the insert.
    pub async fn license_limit_guard(&self, limit: &str) -> StoreResult<LicenseLimitGuard<'_>> {
        let mut tx = self.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1), 0)")
            .bind(limit)
            .execute(&mut *tx)
            .await?;
        Ok(LicenseLimitGuard { _tx: tx })
    }

    pub(crate) fn cipher(&self) -> StoreResult<&SecretCipher> {
        self.cipher
            .as_deref()
            .ok_or_else(|| StoreError::Internal("secret cipher is not configured".into()))
    }
}

/// Decode an opaque cursor supplied by a client into `(timestamp, id)`.
pub fn decode_cursor(cursor: &str) -> StoreResult<Option<(chrono::DateTime<chrono::Utc>, Uuid)>> {
    match orbit_domain::Cursor::decode(cursor) {
        Some(c) => Ok(Some((c.created_at, c.id))),
        None => Err(StoreError::Invalid("invalid pagination cursor".into())),
    }
}

pub fn encode_cursor(created_at: chrono::DateTime<chrono::Utc>, id: Uuid) -> String {
    orbit_domain::Cursor::new(created_at, id).encode()
}

/// Helper for `LIMIT`-style pagination that fetches `limit + 1` rows.
pub(crate) fn page_of<T>(
    mut items: Vec<T>,
    limit: i64,
    cursor_of: impl Fn(&T) -> (chrono::DateTime<chrono::Utc>, Uuid),
) -> (Vec<T>, Option<String>) {
    let has_more = items.len() as i64 > limit;
    if has_more {
        items.truncate(limit as usize);
    }
    let next = if has_more {
        items.last().map(|item| {
            let (created_at, id) = cursor_of(item);
            encode_cursor(created_at, id)
        })
    } else {
        None
    };
    (items, next)
}
