use std::sync::Arc;

use chrono::Utc;
use orbit_events::EventBus;
use orbit_harness::HarnessRegistry;
use orbit_security::{AuthConfig, JwtValidator, LicenseHandle, LicenseStatus};
use orbit_store::executions::NewExecution;
use orbit_store::{ExecutionRecord, LicenseLimitGuard, Store};
use uuid::Uuid;

use crate::metrics::Metrics;
use crate::rate_limit::RateLimiter;

#[derive(Debug, Clone)]
pub struct DevIdentity {
    pub user_id: Uuid,
    pub org_id: Uuid,
    pub email: String,
    pub display_name: String,
    pub role: orbit_domain::UserRole,
}

/// Exposure of the Prometheus endpoint (see `docs/development/observability.md`).
#[derive(Clone, Default)]
pub struct MetricsAccess {
    /// Bearer token a scraper must present. `None` leaves the endpoint open, which is
    /// only appropriate on a network that untrusted clients cannot reach.
    pub token: Option<String>,
    /// True when `/metrics` is served on its own listener (`ORBIT_METRICS_ADDR`); the
    /// main listener then does not serve it at all.
    pub dedicated_listener: bool,
}

impl std::fmt::Debug for MetricsAccess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the token.
        f.debug_struct("MetricsAccess")
            .field("token", &self.token.as_ref().map(|_| "<set>"))
            .field("dedicated_listener", &self.dedicated_listener)
            .finish()
    }
}

#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub bus: Arc<dyn EventBus>,
    pub registry: Arc<HarnessRegistry>,
    pub auth: Arc<AuthConfig>,
    pub jwt: Option<Arc<JwtValidator>>,
    pub default_org_id: Uuid,
    pub dev_identity: Option<Arc<DevIdentity>>,
    pub environment: String,
    pub events_driver: String,
    pub public_base_url: String,
    pub metrics: Arc<Metrics>,
    /// Who may read `/metrics`, and on which listener it is served.
    pub metrics_access: MetricsAccess,
    pub max_body_bytes: usize,
    /// Per-source-IP limiter for public/unauthenticated routes only.
    pub rate_limiter: Arc<RateLimiter>,
    pub cors_origins: Vec<String>,
    pub launcher_url: Option<String>,
    pub launcher_token: Option<String>,
    pub worker_api_url: String,
    /// Secret name/key mounted into launcher-created workers for private CAs.
    pub ca_secret_name: Option<String>,
    pub ca_secret_key: String,
    /// Archive terminal runs older than this many days. `None` disables retention.
    pub archive_after_days: Option<u32>,
    /// Prune terminal output deliveries older than this many days. `None` keeps them.
    pub output_delivery_retention_days: Option<u32>,
    /// Offline enterprise license (hot reloadable).
    pub license: LicenseHandle,
    /// Cancelled on SIGTERM/SIGINT: stops background loops and ends SSE streams.
    pub shutdown: tokio_util::sync::CancellationToken,
}

/// Guard held while an execution is counted and inserted. `Unrestricted` is returned when
/// the installed license has no daily execution limit: no lock is taken and no count runs.
pub enum ExecutionLimit<'a> {
    Unrestricted,
    Enforced(LicenseLimitGuard<'a>),
}

impl AppState {
    pub fn is_production(&self) -> bool {
        self.environment == "production"
    }

    pub fn auth_cors_origins(&self) -> &[String] {
        &self.cors_origins
    }

    pub fn license_status(&self) -> LicenseStatus {
        self.license.status()
    }

    /// Guard + installation-wide seat count. Keep the returned guard alive until the user
    /// row is written so concurrent provisions cannot race past the seat limit.
    pub async fn seat_limit_guard(&self) -> Result<LicenseLimitGuard<'_>, crate::error::ApiError> {
        let guard = self.store.license_limit_guard("seats").await?;
        self.enforce_limit("seats", self.store.count_active_users_installation().await?)?;
        Ok(guard)
    }

    /// Guard + installation-wide agent count. Hold until the agent row is written. Returns
    /// `None` without touching the database when the license has no agent limit.
    pub async fn agent_limit_guard(
        &self,
    ) -> Result<Option<LicenseLimitGuard<'_>>, crate::error::ApiError> {
        if self.license_status().limit("agents").is_none() {
            return Ok(None);
        }
        let guard = self.store.license_limit_guard("agents").await?;
        self.enforce_limit("agents", self.store.count_agents_installation().await?)?;
        Ok(Some(guard))
    }

    /// Guard + installation-wide rolling 24h execution count. Hold until the execution
    /// is inserted. Without a configured limit this performs no lock and no count.
    pub async fn execution_limit_guard(
        &self,
    ) -> Result<ExecutionLimit<'_>, crate::error::ApiError> {
        if self.license_status().limit("executions_per_day").is_none() {
            return Ok(ExecutionLimit::Unrestricted);
        }
        let guard = self.store.license_limit_guard("executions_per_day").await?;
        let since = Utc::now() - chrono::Duration::hours(24);
        let count = self
            .store
            .count_all_executions_since_installation(since)
            .await?;
        self.enforce_limit("executions_per_day", count)?;
        Ok(ExecutionLimit::Enforced(guard))
    }

    /// The single insertion point for licensed executions. Every manual run, rerun, team
    /// step, scheduled trigger and runtime tick goes through here so none can skip the
    /// `executions_per_day` guard. An idempotent replay returns the existing execution
    /// without consuming a slot or taking the lock.
    pub async fn create_execution_with_limit(
        &self,
        new: NewExecution<'_>,
    ) -> Result<ExecutionRecord, crate::error::ApiError> {
        if let Some(key) = new.idempotency_key {
            if let Some(existing) = self
                .store
                .get_execution_by_idempotency_key(new.org_id, key)
                .await?
            {
                return Ok(existing);
            }
        }
        let _limit = self.execution_limit_guard().await?;
        Ok(self.store.create_execution(new).await?)
    }

    /// Guard + installation-wide running-worker count. Hold until the launch is inserted.
    pub async fn worker_limit_guard(
        &self,
    ) -> Result<LicenseLimitGuard<'_>, crate::error::ApiError> {
        let guard = self.store.license_limit_guard("workers").await?;
        self.enforce_limit(
            "workers",
            self.store
                .count_active_worker_launches_installation()
                .await?,
        )?;
        Ok(guard)
    }

    /// Reject a create when the license limit for that resource is reached.
    pub fn enforce_limit(&self, limit: &str, current: i64) -> Result<(), crate::error::ApiError> {
        let status = self.license_status();
        let Some(maximum) = status.limit(limit) else {
            return Ok(());
        };
        if current >= maximum as i64 {
            return Err(crate::error::ApiError::new(
                axum::http::StatusCode::PAYMENT_REQUIRED,
                "license_limit",
                format!(
                    "license limit reached: {limit} {current}/{maximum}. Renew or upgrade the license."
                ),
            ));
        }
        Ok(())
    }

    /// Organization default harness, when configured.
    pub async fn default_harness(&self) -> Option<orbit_domain::HarnessKind> {
        self.store
            .get_platform_setting("harness_default")
            .await
            .ok()
            .flatten()
            .and_then(|value| {
                value
                    .get("default")
                    .and_then(|default| default.as_str())
                    .and_then(|name| name.parse().ok())
            })
    }

    /// Reject a premium action when the license does not grant the feature.
    pub fn require_feature(&self, feature: &str) -> Result<(), crate::error::ApiError> {
        self.license.require_feature(feature).map_err(|message| {
            crate::error::ApiError::new(
                axum::http::StatusCode::PAYMENT_REQUIRED,
                "license_required",
                message,
            )
        })
    }
}
