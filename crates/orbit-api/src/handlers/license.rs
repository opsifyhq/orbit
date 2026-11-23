//! License status and usage overview.

use axum::extract::State;
use axum::Extension;
use axum::Json;
use serde::Serialize;

use orbit_domain::UserRole;
use orbit_security::{AuthContext, LicenseStatus, Permission, RESERVED_FEATURES};

use crate::error::ApiError;
use crate::state::AppState;

/// Counts are installation-wide (all organizations) because licenses are installed once
/// per ORBIT installation, not per organization. They are returned to Owners and Admins
/// only (#325).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LicenseUsage {
    pub seats: i64,
    pub agents: i64,
    pub workers: i64,
    pub executions_today: i64,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LicenseOverview {
    pub license: LicenseStatus,
    /// Installation-wide usage; omitted for roles other than Owner and Admin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<LicenseUsage>,
    pub warnings: Vec<String>,
}

#[utoipa::path(get, path = "/api/v1/license", tag = "license", responses((status = 200, body = LicenseOverview)))]
pub async fn overview(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> Result<Json<LicenseOverview>, ApiError> {
    auth.require(Permission::SettingsRead)?;
    let license = state.license_status();
    let mut warnings = Vec::new();
    if license.valid
        && license
            .features
            .iter()
            .any(|feature| RESERVED_FEATURES.contains(&feature.as_str()))
    {
        warnings.push(
            "license contains flags for features that are not available in this release".into(),
        );
    }
    // Usage spans every organization of the installation: only Owners and Admins see
    // it, and the near-limit warnings that quote it.
    if !matches!(auth.role, UserRole::Owner | UserRole::Admin) {
        return Ok(Json(LicenseOverview {
            license,
            usage: None,
            warnings,
        }));
    }
    let since = chrono::Utc::now() - chrono::Duration::hours(24);
    let usage = LicenseUsage {
        seats: state.store.count_active_users_installation().await?,
        agents: state.store.count_agents_installation().await?,
        workers: state
            .store
            .count_active_worker_launches_installation()
            .await?,
        executions_today: state
            .store
            .count_all_executions_since_installation(since)
            .await?,
    };
    for (key, current) in [
        ("seats", usage.seats),
        ("agents", usage.agents),
        ("workers", usage.workers),
        ("executions_per_day", usage.executions_today),
    ] {
        if let Some(limit) = license.limit(key) {
            if limit > 0 && current * 100 >= limit as i64 * 80 {
                warnings.push(format!("{key}: {current}/{limit} used"));
            }
        }
    }
    Ok(Json(LicenseOverview {
        license,
        usage: Some(usage),
        warnings,
    }))
}
