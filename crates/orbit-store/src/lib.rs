//! PostgreSQL persistence for ORBIT.
//!
//! The store is deliberately thin: SQL and row mapping only. Business rules live in
//! `orbit-domain`; HTTP concerns live in `orbit-api`. Every tenant-scoped query takes an
//! `org_id` and filters on it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Postgres, Transaction};
use tokio::sync::{Mutex, OwnedMutexGuard};
use uuid::Uuid;

use orbit_security::SecretCipher;

/// The database for integration tests (`ORBIT_TEST_DATABASE_URL`). Without it the
/// database-backed tests skip themselves, which is right on a laptop but would hide a
/// broken CI setup behind a green run, so in CI (`CI=true`) a missing URL panics (#445).
pub fn test_database_url() -> Option<String> {
    match std::env::var("ORBIT_TEST_DATABASE_URL") {
        Ok(url) if !url.trim().is_empty() => Some(url),
        _ if std::env::var("CI").is_ok_and(|value| value == "true") => panic!(
            "ORBIT_TEST_DATABASE_URL is not set in CI: the database-backed tests would be skipped"
        ),
        _ => None,
    }
}

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
pub mod metrics;
pub mod models;
pub mod observe;
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

const LICENSE_LOCK_RETRY_BASE: Duration = Duration::from_millis(2);
const LICENSE_LOCK_RETRY_MAX: Duration = Duration::from_millis(25);
const LICENSE_LOCK_WAIT: Duration = Duration::from_secs(10);

/// Serialises a check-then-create sequence until dropped. See [`Store::license_limit_guard`].
pub struct LicenseLimitGuard<'a> {
    // Queues local callers before they take a database connection. The advisory lock uses a
    // try-lock loop, so only the winner keeps a transaction (and pool connection) open.
    _local: OwnedMutexGuard<()>,
    // Kept alive for its drop side effect (rollback releases the advisory lock).
    _tx: Transaction<'a, Postgres>,
}

/// Shared database handle. Cloning is cheap (`PgPool` and the lock map are `Arc`s internally).
#[derive(Clone)]
pub struct Store {
    pool: PgPool,
    cipher: Option<Arc<SecretCipher>>,
    license_limit_locks: Arc<StdMutex<HashMap<String, Arc<Mutex<()>>>>>,
    observer: observe::SharedObserver,
}

impl Store {
    pub fn new(pool: PgPool, cipher: Option<Arc<SecretCipher>>) -> Self {
        Self {
            pool,
            cipher,
            license_limit_locks: Arc::new(StdMutex::new(HashMap::new())),
            observer: None,
        }
    }

    pub async fn connect(database_url: &str, max_connections: u32) -> StoreResult<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .acquire_timeout(Duration::from_secs(10))
            .connect(database_url)
            .await?;
        Ok(Self {
            pool,
            cipher: None,
            license_limit_locks: Arc::new(StdMutex::new(HashMap::new())),
            observer: None,
        })
    }

    pub fn with_cipher(mut self, cipher: Arc<SecretCipher>) -> Self {
        self.cipher = Some(cipher);
        self
    }

    /// Report execution lifecycle transitions to `observer` (see [`observe`]).
    pub fn with_execution_observer(
        mut self,
        observer: Arc<dyn observe::ExecutionObserver>,
    ) -> Self {
        self.observer = Some(observer);
        self
    }

    pub(crate) fn observe_created(&self, execution: &ExecutionRecord) {
        if let Some(observer) = &self.observer {
            observer.created(execution);
        }
    }

    pub(crate) fn observe_started(&self, execution: &ExecutionRecord) {
        if let Some(observer) = &self.observer {
            observer.started(execution);
        }
    }

    pub(crate) fn observe_finished(
        &self,
        execution: &ExecutionRecord,
        cause: observe::FinishCause,
    ) {
        if let Some(observer) = &self.observer {
            observer.finished(execution, cause);
        }
    }

    /// Connections currently open in the pool and how many of them are idle.
    pub fn pool_stats(&self) -> (u32, u32) {
        (self.pool.size(), self.pool.num_idle() as u32)
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
    /// A process-local mutex queues callers before they take a database connection. The
    /// PostgreSQL advisory lock is acquired with `pg_try_advisory_xact_lock`: a losing
    /// transaction is rolled back immediately and the caller sleeps before retrying, so
    /// contenders never hold pooled connections while waiting. The advisory lock still
    /// serialises callers across API replicas. Dropping the guard rolls the (otherwise
    /// empty) winning transaction back and releases both locks, including on early returns
    /// and errors. Every path that creates a resource counted by a license limit must hold
    /// the matching guard across both the count and the insert.
    pub async fn license_limit_guard(&self, limit: &str) -> StoreResult<LicenseLimitGuard<'_>> {
        let lock = {
            let mut locks = self
                .license_limit_locks
                .lock()
                .map_err(|_| StoreError::Internal("license limit lock is poisoned".into()))?;
            locks
                .entry(limit.to_owned())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        // The local queue counts toward the same deadline as the advisory lock, so a slow
        // holder in this process cannot make callers wait without bound (#396).
        let deadline = Instant::now() + LICENSE_LOCK_WAIT;
        let _local = tokio::time::timeout_at(deadline.into(), lock.lock_owned())
            .await
            .map_err(|_| StoreError::Conflict("license limit is busy; retry".into()))?;
        let mut retry_delay = LICENSE_LOCK_RETRY_BASE;

        loop {
            let mut tx = self.begin().await?;
            let acquired: bool =
                sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtext($1), 0)")
                    .bind(limit)
                    .fetch_one(&mut *tx)
                    .await?;
            if acquired {
                return Ok(LicenseLimitGuard { _local, _tx: tx });
            }

            // Do not keep a pool connection while another replica holds the lock.
            tx.rollback().await?;
            if Instant::now() >= deadline {
                return Err(StoreError::Conflict("license limit is busy; retry".into()));
            }
            tokio::time::sleep(retry_delay).await;
            retry_delay = std::cmp::min(retry_delay * 2, LICENSE_LOCK_RETRY_MAX);
        }
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
