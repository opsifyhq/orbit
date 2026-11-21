use std::sync::Arc;

use orbit_events::EventBus;
use orbit_harness::HarnessRegistry;
use orbit_security::{AuthConfig, JwtValidator, LicenseHandle, LicenseStatus};
use orbit_store::Store;
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
    /// Offline enterprise license (hot reloadable).
    pub license: LicenseHandle,
    /// Cancelled on SIGTERM/SIGINT: stops background loops and ends SSE streams.
    pub shutdown: tokio_util::sync::CancellationToken,
}

impl AppState {
    pub fn is_production(&self) -> bool {
        self.environment == "production"
    }

    pub fn is_dev_mode(&self) -> bool {
        matches!(self.auth.mode, orbit_security::AuthMode::Dev)
    }

    pub fn auth_cors_origins(&self) -> &[String] {
        &self.cors_origins
    }

    pub fn license_status(&self) -> LicenseStatus {
        self.license.status()
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
