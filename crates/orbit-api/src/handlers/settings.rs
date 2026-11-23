//! Platform settings (OIDC, etc.) manageable from the UI.

use axum::extract::State;
use axum::Extension;
use axum::Json;
use serde::{Deserialize, Serialize};

use orbit_security::{AuthContext, Permission};

use orbit_domain::ValidationIssue;

use crate::audit;
use crate::error::ApiError;
use crate::extract::ApiJson;
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OidcSettings {
    #[serde(default)]
    pub issuer: String,
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub redirect_uri: String,
    #[serde(default)]
    pub scopes: String,
    #[serde(default)]
    pub audience: String,
    #[serde(default)]
    pub jwks_url: String,
    #[serde(default)]
    pub auto_provision: bool,
    #[serde(default)]
    pub default_role: String,
    /// Claim carrying IdP groups/roles (default `groups`).
    #[serde(default)]
    pub role_claim: String,
    /// Claim used as the unique OIDC identity/email (default `email`).
    #[serde(default)]
    pub email_claim: String,
    /// IdP group/claim value -> ORBIT role name.
    #[serde(default)]
    pub group_roles: std::collections::BTreeMap<String, String>,
    /// IdP group -> `team-slug:role` (access team membership).
    #[serde(default)]
    pub team_groups: std::collections::BTreeMap<String, String>,
    /// Create missing access teams for mapped groups.
    #[serde(default)]
    pub auto_create_teams: bool,
    /// Deny users whose groups match no role/team mapping.
    #[serde(default)]
    pub require_mapped_group: bool,
    /// Re-evaluate the stored role on every login when a mapping matches.
    #[serde(default)]
    pub sync_roles: bool,
    /// Link an incoming identity to an existing user by email only when the token says
    /// `email_verified: true` (a missing claim also blocks linking). This setting can
    /// only turn the requirement on: it is already on unless
    /// `ORBIT_OIDC_REQUIRE_VERIFIED_EMAIL=false` turned it off (the default is on).
    #[serde(default)]
    pub require_verified_email: bool,
}

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateOidcRequest {
    /// Partial or full OIDC settings. Fields that are absent keep their stored value.
    /// Kept as raw JSON so "absent" can be told apart from "empty"; the schema is
    /// still `OidcSettings`.
    #[schema(value_type = OidcSettings)]
    pub settings: serde_json::Value,
}

/// Overlay the keys present in `patch` onto the stored settings object and parse the
/// result. Absent keys keep their stored value (or the serde default when nothing is
/// stored yet). Errors describe the first malformed field.
fn merge_oidc_settings(
    stored: Option<serde_json::Value>,
    patch: serde_json::Value,
) -> Result<OidcSettings, ValidationIssue> {
    let serde_json::Value::Object(patch) = patch else {
        return Err(ValidationIssue::new("settings", "must be an object"));
    };
    let mut merged = match stored {
        Some(serde_json::Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    };
    for (key, value) in patch {
        merged.insert(key, value);
    }
    serde_json::from_value(serde_json::Value::Object(merged))
        .map_err(|e| ValidationIssue::new("settings", e.to_string()))
}

#[utoipa::path(get, path = "/api/v1/settings/oidc", tag = "settings", responses((status = 200, body = OidcSettings)))]
pub async fn get_oidc(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> Result<Json<OidcSettings>, ApiError> {
    auth.require(Permission::SettingsWrite)?;
    let value = state
        .store
        .get_platform_setting("oidc")
        .await?
        .unwrap_or_else(|| serde_json::json!({}));
    let settings: OidcSettings = serde_json::from_value(value).unwrap_or(OidcSettings {
        issuer: String::new(),
        client_id: String::new(),
        redirect_uri: String::new(),
        scopes: "openid profile email".into(),
        audience: String::new(),
        jwks_url: String::new(),
        auto_provision: true,
        default_role: "viewer".into(),
        role_claim: "groups".into(),
        email_claim: "email".into(),
        group_roles: Default::default(),
        team_groups: Default::default(),
        auto_create_teams: false,
        require_mapped_group: false,
        sync_roles: false,
        require_verified_email: orbit_security::DEFAULT_REQUIRE_VERIFIED_EMAIL,
    });
    Ok(Json(settings))
}

/// Validate OIDC settings. Returns field-level issues; `None`-free means valid.
fn validate_oidc(state: &AppState, settings: &OidcSettings) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    for (path, value) in [
        ("settings.issuer", &settings.issuer),
        ("settings.jwksUrl", &settings.jwks_url),
    ] {
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        match reqwest::Url::parse(value) {
            Ok(url) if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() => {
                issues.push(ValidationIssue::new(path, "must be an http(s) URL"));
            }
            Ok(url) if url.scheme() != "https" && state.is_production() => {
                issues.push(ValidationIssue::new(
                    path,
                    "must use https when ORBIT_ENV=production",
                ));
            }
            Ok(_) => {}
            Err(_) => issues.push(ValidationIssue::new(path, "must be a valid URL")),
        }
    }
    let redirect = settings.redirect_uri.trim();
    if !redirect.is_empty() && reqwest::Url::parse(redirect).is_err() {
        issues.push(ValidationIssue::new(
            "settings.redirectUri",
            "must be a valid URL",
        ));
    }

    let mut role_refs: Vec<(String, &str)> = Vec::new();
    if !settings.default_role.trim().is_empty() {
        role_refs.push(("settings.defaultRole".into(), settings.default_role.trim()));
    }
    for (group, role) in &settings.group_roles {
        role_refs.push((format!("settings.groupRoles.{group}"), role.trim()));
    }
    // Only built-in roles take effect at OIDC login (`apply_stored_oidc` parses them as
    // `UserRole`), so accepting a custom role name here would be saved and then ignored.
    for (path, role) in role_refs {
        if role.parse::<orbit_domain::UserRole>().is_err() {
            issues.push(ValidationIssue::new(
                path,
                format!(
                    "unknown role '{role}': OIDC mappings support the built-in roles \
                     (owner, admin, operator, viewer)"
                ),
            ));
        }
    }
    for (group, value) in &settings.team_groups {
        let valid = value.split_once(':').is_some_and(|(team, role)| {
            !team.trim().is_empty() && matches!(role.trim(), "lead" | "member" | "viewer")
        });
        if !valid {
            issues.push(ValidationIssue::new(
                format!("settings.teamGroups.{group}"),
                "must be '<team-slug>:<lead|member|viewer>'",
            ));
        }
    }
    issues
}

/// Audit-safe summary of OIDC settings. The settings hold no client secret; only
/// non-sensitive configuration is summarised (mappings by count, not content).
fn oidc_audit_summary(settings: &OidcSettings) -> serde_json::Value {
    serde_json::json!({
        "issuer": settings.issuer,
        "clientId": settings.client_id,
        "jwksUrl": settings.jwks_url,
        "audience": settings.audience,
        "autoProvision": settings.auto_provision,
        "defaultRole": settings.default_role,
        "roleClaim": settings.role_claim,
        "groupRoles": settings.group_roles,
        "teamGroups": settings.team_groups,
        "autoCreateTeams": settings.auto_create_teams,
        "requireMappedGroup": settings.require_mapped_group,
        "syncRoles": settings.sync_roles,
        "requireVerifiedEmail": settings.require_verified_email,
    })
}

/// Update the OIDC sign-in settings.
///
/// This is a merge: only the fields present in `settings` change, and absent fields
/// (including `roleClaim`, `groupRoles`, `teamGroups`, `autoCreateTeams`,
/// `requireMappedGroup` and `syncRoles`) are preserved. To clear a mapping, send it
/// explicitly (for example `"groupRoles": {}`). The merged result is validated before
/// it is stored.
#[utoipa::path(put, path = "/api/v1/settings/oidc", tag = "settings", request_body = UpdateOidcRequest, responses((status = 200, body = OidcSettings), (status = 422, body = crate::error::ErrorEnvelope)))]
pub async fn update_oidc(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    ApiJson(body): ApiJson<UpdateOidcRequest>,
) -> Result<Json<OidcSettings>, ApiError> {
    auth.require(Permission::SettingsWrite)?;
    let stored_before = state.store.get_platform_setting("oidc").await?;
    let previous = stored_before
        .clone()
        .and_then(|value| serde_json::from_value::<OidcSettings>(value).ok());
    let before = previous.as_ref().map(oidc_audit_summary);
    let merged = merge_oidc_settings(stored_before, body.settings)
        .map_err(|issue| ApiError::validation(vec![issue]))?;
    let issues = validate_oidc(&state, &merged);
    if !issues.is_empty() {
        return Err(ApiError::validation(issues));
    }
    require_oidc_role_ceiling(&auth, previous.as_ref(), &merged)?;
    let value = serde_json::to_value(&merged).map_err(|e| ApiError::bad_request(e.to_string()))?;
    let stored = state
        .store
        .set_platform_setting("oidc", &value, Some(auth.user_id))
        .await?;
    let settings: OidcSettings = serde_json::from_value(stored).unwrap_or(merged);
    audit::record(
        &state,
        &auth,
        "settings.oidc.update",
        Some("platform_setting"),
        None,
        serde_json::json!({ "before": before, "after": oidc_audit_summary(&settings) }),
    )
    .await;
    Ok(Json(settings))
}

/// Grant ceiling for OIDC role mappings (#324). A user with `settings:write` must not be
/// able to give sign-ins a role with permissions they do not hold themselves, for example
/// by mapping their own IdP group to `owner`. Only mappings that change are checked, so
/// editing other fields keeps working when an Owner configured higher roles earlier.
fn require_oidc_role_ceiling(
    auth: &AuthContext,
    previous: Option<&OidcSettings>,
    next: &OidcSettings,
) -> Result<(), ApiError> {
    let mut changed: Vec<&str> = Vec::new();
    let default_role = next.default_role.trim();
    if !default_role.is_empty()
        && previous.is_none_or(|previous| previous.default_role.trim() != default_role)
    {
        changed.push(default_role);
    }
    for (group, role) in &next.group_roles {
        let unchanged = previous
            .and_then(|previous| previous.group_roles.get(group))
            .is_some_and(|before| before.trim() == role.trim());
        if !unchanged {
            changed.push(role.trim());
        }
    }
    let held = auth.permissions();
    for role in changed {
        let Ok(role) = role.parse::<orbit_domain::UserRole>() else {
            continue; // validate_oidc reported it
        };
        let granted: Vec<String> = Permission::all_for(role)
            .into_iter()
            .map(str::to_string)
            .collect();
        if !orbit_security::ungranted_permissions(&held, &granted).is_empty() {
            return Err(ApiError::forbidden(format!(
                "you cannot map sign-ins to the {} role, which has permissions you do not hold",
                role.as_str()
            )));
        }
    }
    Ok(())
}

/// Organization-wide switches that weaken isolation or approval controls need every
/// permission (an Owner, or a custom role that grants all of them), not just
/// `settings:write` (#324).
fn require_full_authority(auth: &AuthContext, what: &str) -> Result<(), ApiError> {
    let all: Vec<String> = Permission::all_for(orbit_domain::UserRole::Owner)
        .into_iter()
        .map(str::to_string)
        .collect();
    if orbit_security::ungranted_permissions(&auth.permissions(), &all).is_empty() {
        Ok(())
    } else {
        Err(ApiError::forbidden(format!(
            "changing {what} requires every permission (an Owner)"
        )))
    }
}

/// Merge OIDC settings stored via the Settings UI into the environment-derived auth
/// config. Only applies in OIDC mode; environment values remain the fallback for any
/// field left empty in the UI.
pub async fn apply_stored_oidc(
    store: &orbit_store::Store,
    auth: orbit_security::AuthConfig,
) -> orbit_security::AuthConfig {
    if !matches!(auth.mode, orbit_security::AuthMode::Oidc) {
        return auth;
    }
    let Ok(Some(value)) = store.get_platform_setting("oidc").await else {
        return auth;
    };
    let Ok(stored) = serde_json::from_value::<OidcSettings>(value) else {
        return auth;
    };
    merge_stored_oidc(auth, stored)
}

fn merge_stored_oidc(
    mut auth: orbit_security::AuthConfig,
    stored: OidcSettings,
) -> orbit_security::AuthConfig {
    if stored.issuer.trim().is_empty() {
        return auth;
    }
    let mut oidc = auth.oidc.clone().unwrap_or(orbit_security::OidcConfig {
        issuer: String::new(),
        audience: String::new(),
        jwks_url: String::new(),
        client_id: String::new(),
        client_secret: None,
        redirect_uri: String::new(),
        scopes: vec!["openid".into(), "profile".into(), "email".into()],
        auto_provision: true,
        default_role: orbit_domain::UserRole::Viewer,
        role_claim: "groups".into(),
        email_claim: "email".into(),
        group_roles: Vec::new(),
        team_groups: Vec::new(),
        auto_create_teams: false,
        require_mapped_group: false,
        sync_roles: false,
        require_verified_email: orbit_security::DEFAULT_REQUIRE_VERIFIED_EMAIL,
        clock_skew_seconds: 60,
    });
    oidc.issuer = stored.issuer.trim().to_string();
    if !stored.audience.trim().is_empty() {
        oidc.audience = stored.audience.trim().to_string();
    }
    if oidc.audience.is_empty() {
        oidc.audience = oidc.issuer.clone();
    }
    if !stored.client_id.trim().is_empty() {
        oidc.client_id = stored.client_id.trim().to_string();
    }
    if oidc.client_id.is_empty() {
        oidc.client_id = oidc.audience.clone();
    }
    if !stored.redirect_uri.trim().is_empty() {
        oidc.redirect_uri = stored.redirect_uri.trim().to_string();
    }
    if !stored.scopes.trim().is_empty() {
        oidc.scopes = stored
            .scopes
            .split_whitespace()
            .map(str::to_string)
            .collect();
    }
    if !stored.jwks_url.trim().is_empty() {
        oidc.jwks_url = stored.jwks_url.trim().to_string();
    }
    if oidc.jwks_url.is_empty() {
        oidc.jwks_url = format!(
            "{}/.well-known/jwks.json",
            oidc.issuer.trim_end_matches('/')
        );
    }
    oidc.auto_provision = stored.auto_provision;
    oidc.default_role = stored
        .default_role
        .parse::<orbit_domain::UserRole>()
        .unwrap_or(orbit_domain::UserRole::Viewer);
    if !stored.role_claim.trim().is_empty() {
        oidc.role_claim = stored.role_claim.trim().to_string();
    }
    if !stored.email_claim.trim().is_empty() {
        oidc.email_claim = stored.email_claim.trim().to_string();
    }
    if !stored.group_roles.is_empty() {
        let mut mappings = Vec::new();
        for (group, role) in &stored.group_roles {
            match role.parse::<orbit_domain::UserRole>() {
                Ok(role) => mappings.push((group.clone(), role)),
                Err(_) => tracing::warn!(
                    group = %group,
                    role = %role,
                    "ignoring OIDC group mapping with an unknown role"
                ),
            }
        }
        oidc.group_roles = mappings;
    }
    if !stored.team_groups.is_empty() {
        let mut mappings = Vec::new();
        for (group, value) in &stored.team_groups {
            match value.split_once(':') {
                Some((team, role))
                    if !team.trim().is_empty()
                        && matches!(role.trim(), "lead" | "member" | "viewer") =>
                {
                    mappings.push((
                        group.clone(),
                        team.trim().to_string(),
                        role.trim().to_string(),
                    ));
                }
                _ => tracing::warn!(group = %group, "ignoring invalid OIDC team mapping"),
            }
        }
        oidc.team_groups = mappings;
    }
    oidc.auto_create_teams = stored.auto_create_teams;
    oidc.require_mapped_group = stored.require_mapped_group;
    if stored.sync_roles {
        oidc.sync_roles = true;
    }
    // Tighten only: a stored `false` means "not required by the Settings page" and
    // never lifts the environment requirement, which defaults to on (#325).
    if stored.require_verified_email {
        oidc.require_verified_email = true;
    }
    tracing::info!(issuer = %oidc.issuer, "applied OIDC settings from the database");
    auth.oidc = Some(oidc);
    auth
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccessSettings {
    /// Non-admins only see resources owned by their access teams.
    pub team_isolation: bool,
}

fn access_settings(value: serde_json::Value) -> AccessSettings {
    AccessSettings {
        team_isolation: value
            .get("enabled")
            .and_then(|enabled| enabled.as_bool())
            .unwrap_or(true),
    }
}

#[utoipa::path(get, path = "/api/v1/settings/access", tag = "settings", responses((status = 200, body = AccessSettings)))]
pub async fn get_access(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> Result<Json<AccessSettings>, ApiError> {
    auth.require(Permission::SettingsRead)?;
    Ok(Json(access_settings(
        state
            .store
            .get_platform_setting("team_isolation")
            .await?
            .unwrap_or_else(|| serde_json::json!({ "enabled": true })),
    )))
}

#[utoipa::path(put, path = "/api/v1/settings/access", tag = "settings", request_body = AccessSettings, responses((status = 200, body = AccessSettings)))]
pub async fn update_access(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    ApiJson(body): ApiJson<AccessSettings>,
) -> Result<Json<AccessSettings>, ApiError> {
    auth.require(Permission::SettingsWrite)?;
    require_full_authority(&auth, "team isolation")?;
    let before = state
        .store
        .get_platform_setting("team_isolation")
        .await?
        .map(|value| access_settings(value).team_isolation);
    let value = serde_json::json!({ "enabled": body.team_isolation });
    state
        .store
        .set_platform_setting("team_isolation", &value, Some(auth.user_id))
        .await?;
    audit::record(
        &state,
        &auth,
        "settings.access.update",
        Some("platform_setting"),
        None,
        serde_json::json!({
            "before": { "teamIsolation": before },
            "after": { "teamIsolation": body.team_isolation },
        }),
    )
    .await;
    Ok(Json(body))
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HarnessSettings {
    /// Harness used when an execution/trigger does not select one.
    pub default_harness: Option<String>,
}

#[utoipa::path(get, path = "/api/v1/settings/harness", tag = "settings", responses((status = 200, body = HarnessSettings)))]
pub async fn get_harness(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> Result<Json<HarnessSettings>, ApiError> {
    auth.require(Permission::SettingsRead)?;
    let value = state
        .store
        .get_platform_setting("harness_default")
        .await?
        .unwrap_or_else(|| serde_json::json!({}));
    Ok(Json(HarnessSettings {
        default_harness: value
            .get("default")
            .and_then(|value| value.as_str())
            .map(str::to_string),
    }))
}

#[utoipa::path(put, path = "/api/v1/settings/harness", tag = "settings", request_body = HarnessSettings, responses((status = 200, body = HarnessSettings)))]
pub async fn update_harness(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    ApiJson(body): ApiJson<HarnessSettings>,
) -> Result<Json<HarnessSettings>, ApiError> {
    auth.require(Permission::SettingsWrite)?;
    let before = state
        .store
        .get_platform_setting("harness_default")
        .await?
        .and_then(|value| {
            value
                .get("default")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        });
    if let Some(harness) = &body.default_harness {
        harness.parse::<orbit_domain::HarnessKind>().map_err(|_| {
            ApiError::validation(vec![orbit_domain::ValidationIssue::new(
                "defaultHarness",
                "unknown harness",
            )])
        })?;
    }
    let value = serde_json::json!({ "default": body.default_harness });
    state
        .store
        .set_platform_setting("harness_default", &value, Some(auth.user_id))
        .await?;
    audit::record(
        &state,
        &auth,
        "settings.harness.update",
        Some("platform_setting"),
        None,
        serde_json::json!({
            "before": { "defaultHarness": before },
            "after": { "defaultHarness": body.default_harness },
        }),
    )
    .await;
    Ok(Json(body))
}

/// Platform-setting key holding the approvals policy.
pub const APPROVALS_SETTING_KEY: &str = "approvals";

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalsSettings {
    /// Four-eyes: the user who requested an execution cannot approve or reject the
    /// approvals that execution raises. Default `false`.
    #[serde(default)]
    pub approvals_require_different_approver: bool,
}

/// Read the approvals policy; missing or malformed values fall back to the default (off).
pub async fn load_approvals_settings(
    store: &orbit_store::Store,
) -> Result<ApprovalsSettings, ApiError> {
    let value = store.get_platform_setting(APPROVALS_SETTING_KEY).await?;
    Ok(value
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or(ApprovalsSettings {
            approvals_require_different_approver: false,
        }))
}

#[utoipa::path(get, path = "/api/v1/settings/approvals", tag = "settings", responses((status = 200, body = ApprovalsSettings)))]
pub async fn get_approvals(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> Result<Json<ApprovalsSettings>, ApiError> {
    auth.require(Permission::SettingsRead)?;
    Ok(Json(load_approvals_settings(&state.store).await?))
}

#[utoipa::path(put, path = "/api/v1/settings/approvals", tag = "settings", request_body = ApprovalsSettings, responses((status = 200, body = ApprovalsSettings)))]
pub async fn update_approvals(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<ApprovalsSettings>,
) -> Result<Json<ApprovalsSettings>, ApiError> {
    auth.require(Permission::SettingsWrite)?;
    require_full_authority(&auth, "approval settings")?;
    let value =
        serde_json::to_value(&body).map_err(|error| ApiError::bad_request(error.to_string()))?;
    state
        .store
        .set_platform_setting(APPROVALS_SETTING_KEY, &value, Some(auth.user_id))
        .await?;
    crate::audit::record(
        &state,
        &auth,
        "settings.approvals.updated",
        Some("settings"),
        None,
        serde_json::json!({
            "approvalsRequireDifferentApprover": body.approvals_require_different_approver,
        }),
    )
    .await;
    Ok(Json(body))
}

#[cfg(test)]
mod verified_email_tests {
    use super::*;
    use orbit_security::{AuthConfig, AuthMode, DevAuthConfig, OidcConfig};

    fn stored(require_verified_email: Option<bool>) -> OidcSettings {
        let mut value = serde_json::json!({ "issuer": "https://idp.example" });
        if let Some(flag) = require_verified_email {
            value["requireVerifiedEmail"] = flag.into();
        }
        serde_json::from_value(value).unwrap()
    }

    fn auth(env_requires: Option<bool>) -> AuthConfig {
        AuthConfig {
            mode: AuthMode::Oidc,
            dev: DevAuthConfig {
                email: String::new(),
                display_name: String::new(),
                org_slug: String::new(),
                org_name: String::new(),
                role: orbit_domain::UserRole::Viewer,
                allow_anonymous: false,
                dev_token: String::new(),
            },
            oidc: env_requires.map(|require_verified_email| OidcConfig {
                issuer: "https://idp.example".into(),
                audience: "orbit".into(),
                jwks_url: "https://idp.example/jwks".into(),
                client_id: "orbit".into(),
                client_secret: None,
                redirect_uri: "http://localhost/auth/callback".into(),
                scopes: vec!["openid".into()],
                auto_provision: true,
                default_role: orbit_domain::UserRole::Viewer,
                role_claim: "groups".into(),
                email_claim: "email".into(),
                group_roles: Vec::new(),
                team_groups: Vec::new(),
                auto_create_teams: false,
                require_mapped_group: false,
                sync_roles: false,
                require_verified_email,
                clock_skew_seconds: 60,
            }),
            allow_api_tokens: true,
        }
    }

    fn effective(env_requires: Option<bool>, stored_flag: Option<bool>) -> bool {
        merge_stored_oidc(auth(env_requires), stored(stored_flag))
            .oidc
            .unwrap()
            .require_verified_email
    }

    #[test]
    fn verified_email_is_required_by_default() {
        const { assert!(orbit_security::DEFAULT_REQUIRE_VERIFIED_EMAIL) };
        // Configured only from the Settings page: the built-in default applies.
        assert!(effective(None, None));
        assert!(effective(None, Some(false)));
    }

    #[test]
    fn stored_settings_only_tighten_the_requirement() {
        // A stored `false` (including settings saved before this default) never lifts
        // the environment requirement.
        assert!(effective(Some(true), Some(false)));
        assert!(effective(Some(true), None));
        // Only the environment can turn it off; the Settings page can turn it on.
        assert!(!effective(Some(false), Some(false)));
        assert!(effective(Some(false), Some(true)));
    }
}

#[cfg(test)]
mod ceiling_tests {
    use super::*;

    fn settings(default_role: &str, groups: &[(&str, &str)]) -> OidcSettings {
        let mut value = serde_json::json!({
            "defaultRole": default_role,
            "groupRoles": groups.iter().map(|(g, r)| (g.to_string(), serde_json::json!(r))).collect::<serde_json::Map<_, _>>(),
        });
        // Every other field takes its serde default.
        value["issuer"] = serde_json::json!("https://idp.example.com");
        serde_json::from_value(value).unwrap()
    }

    fn delegated_admin() -> AuthContext {
        AuthContext {
            user_id: uuid::Uuid::now_v7(),
            org_id: uuid::Uuid::now_v7(),
            email: "delegate@test.local".into(),
            display_name: "Delegate".into(),
            role: orbit_domain::UserRole::Viewer,
            method: orbit_security::AuthMethod::Dev,
            // A custom role: settings administration without everything else.
            scopes: vec![
                "settings:read".into(),
                "settings:write".into(),
                "members:read".into(),
            ],
            token_id: None,
        }
    }

    fn owner() -> AuthContext {
        AuthContext {
            scopes: vec![],
            role: orbit_domain::UserRole::Owner,
            ..delegated_admin()
        }
    }

    /// #324: settings:write alone cannot map sign-ins to a role above the caller's own.
    #[test]
    fn delegated_admins_cannot_raise_oidc_roles() {
        let before = settings("viewer", &[]);
        let raise_default = settings("owner", &[]);
        assert!(
            require_oidc_role_ceiling(&delegated_admin(), Some(&before), &raise_default).is_err()
        );
        let raise_group = settings("viewer", &[("my-group", "admin")]);
        assert!(
            require_oidc_role_ceiling(&delegated_admin(), Some(&before), &raise_group).is_err()
        );
        assert!(require_oidc_role_ceiling(&owner(), Some(&before), &raise_group).is_ok());
    }

    #[test]
    fn unchanged_mappings_do_not_block_other_edits() {
        // An Owner configured these; a delegated admin may still edit other fields.
        let before = settings("operator", &[("admins", "owner")]);
        let next = settings("operator", &[("admins", "owner")]);
        assert!(require_oidc_role_ceiling(&delegated_admin(), Some(&before), &next).is_ok());
        // Adding a new owner mapping is still refused.
        let next = settings("operator", &[("admins", "owner"), ("mine", "owner")]);
        assert!(require_oidc_role_ceiling(&delegated_admin(), Some(&before), &next).is_err());
    }

    #[test]
    fn organization_toggles_need_every_permission() {
        assert!(require_full_authority(&delegated_admin(), "team isolation").is_err());
        assert!(require_full_authority(&owner(), "team isolation").is_ok());
    }
}
