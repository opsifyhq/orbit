use axum::extract::{Request, State};
use axum::http::header::AUTHORIZATION;
use axum::middleware::Next;
use axum::response::Response;

use orbit_domain::UserRole;
use orbit_security::{bearer_token, hash_api_token, highest_mapped_role, AuthContext, AuthMethod};

use crate::error::ApiError;
use crate::state::AppState;

/// Authenticate the request and insert an [`AuthContext`] into extensions.
pub async fn require_auth(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let header = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    let token = bearer_token(header);
    let context = resolve(&state, token).await?;
    request.extensions_mut().insert(context);
    Ok(next.run(request).await)
}

async fn resolve(state: &AppState, token: Option<&str>) -> Result<AuthContext, ApiError> {
    // 1) ORBIT API tokens (works in every auth mode).
    if let Some(token) = token {
        if state.auth.allow_api_tokens && token.starts_with("orbit_sk_") {
            return resolve_api_token(state, token).await;
        }
    }

    match state.auth.mode {
        orbit_security::AuthMode::Dev => resolve_dev(state, token),
        orbit_security::AuthMode::Oidc => resolve_oidc(state, token).await,
    }
}

fn resolve_dev(state: &AppState, token: Option<&str>) -> Result<AuthContext, ApiError> {
    let dev = state
        .dev_identity
        .as_ref()
        .ok_or_else(|| ApiError::internal("dev identity was not initialized"))?;
    let dev_token = token.is_some_and(|t| constant_time_eq(t, &state.auth.dev.dev_token));
    let anonymous = state.auth.dev.allow_anonymous;
    // With anonymous access disabled only the exact dev token authenticates (a missing
    // or wrong token is rejected). With anonymous access enabled any request, including
    // one carrying a non-dev token, is accepted so the web app works without token
    // plumbing.
    if !dev_token && !anonymous {
        return Err(ApiError::unauthorized("authentication required"));
    }
    Ok(AuthContext {
        user_id: dev.user_id,
        org_id: dev.org_id,
        email: dev.email.clone(),
        display_name: dev.display_name.clone(),
        role: dev.role,
        method: if token.is_some() {
            AuthMethod::Dev
        } else {
            AuthMethod::Anonymous
        },
        scopes: vec![],
        token_id: None,
    })
}

/// Constant-time string equality (length is not hidden; the dev token length is not secret).
fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn resolve_api_token(state: &AppState, token: &str) -> Result<AuthContext, ApiError> {
    let hash = hash_api_token(token);
    let record = state
        .store
        .find_api_token_by_hash(&hash)
        .await?
        .ok_or_else(|| ApiError::unauthorized("invalid API token"))?;
    if let Some(expires_at) = record.expires_at {
        if expires_at < chrono::Utc::now() {
            return Err(ApiError::unauthorized("API token has expired"));
        }
    }
    let user = state.store.get_user(record.org_id, record.user_id).await?;
    if user.status != "active" {
        return Err(ApiError::forbidden("user is disabled"));
    }
    let (role, role_scopes) = resolve_role(state, user.org_id, &user.role).await?;
    // Token scopes are bounded by the owner's CURRENT role so demotion takes effect
    // immediately. An empty stored list keeps meaning "whatever the role grants".
    let scopes = if record.scopes.is_empty() {
        role_scopes
    } else {
        let held: Vec<String> = if role_scopes.is_empty() {
            orbit_security::Permission::all_for(role)
                .into_iter()
                .map(str::to_string)
                .collect()
        } else {
            role_scopes
        };
        let bounded = orbit_security::intersect_scopes(&record.scopes, &held);
        if bounded.is_empty() {
            // An empty list would fall back to the full role set, so deny explicitly.
            return Err(ApiError::forbidden(
                "API token has no permissions under the owner's current role",
            ));
        }
        bounded
    };
    let store = state.store.clone();
    let token_id = record.id;
    tokio::spawn(async move {
        let _ = store.touch_api_token(token_id).await;
    });
    Ok(AuthContext {
        user_id: user.id,
        org_id: user.org_id,
        email: user.email,
        display_name: user.display_name,
        role,
        method: AuthMethod::ApiToken,
        scopes,
        token_id: Some(record.id),
    })
}

async fn resolve_oidc(state: &AppState, token: Option<&str>) -> Result<AuthContext, ApiError> {
    let validator = state
        .jwt
        .as_ref()
        .ok_or_else(|| ApiError::internal("OIDC validator is not configured"))?;
    let token = token.ok_or_else(|| ApiError::unauthorized("bearer token required"))?;
    let identity = validator.validate(token).await?;
    let oidc = state
        .auth
        .oidc
        .as_ref()
        .ok_or_else(|| ApiError::internal("OIDC config missing"))?;
    // Only mappings that are explicitly configured can grant a role. With no match we
    // fall back to the configured default role.
    let mapped_role = highest_mapped_role(&identity.groups, &oidc.group_roles);
    let mapped_team = identity.groups.iter().any(|group| {
        oidc.team_groups
            .iter()
            .any(|(mapped, _, _)| mapped == group)
    });
    if oidc.require_mapped_group && mapped_role.is_none() && !mapped_team {
        return Err(ApiError::forbidden(
            "your OIDC account is not authorized for this ORBIT installation",
        ));
    }
    let target_role = mapped_role.unwrap_or(oidc.default_role);

    let user = match state
        .store
        .get_user_by_oidc(&identity.issuer, &identity.subject)
        .await?
    {
        Some(user) => {
            // Only a matched group mapping may change an existing user's role; unmapped
            // groups or a missing claim keep manually assigned (and custom) roles.
            if let Some(new_role) = synced_role(
                &user.role,
                mapped_role,
                oidc.sync_roles,
                !identity.groups.is_empty(),
            ) {
                tracing::info!(
                    user_id = %user.id,
                    from = %user.role,
                    to = %new_role.as_str(),
                    "syncing OIDC role from group mapping"
                );
                state
                    .store
                    .set_user_role(user.org_id, user.id, new_role)
                    .await?
            } else {
                user
            }
        }
        None => {
            if !oidc.auto_provision {
                return Err(ApiError::forbidden(
                    "your identity is not provisioned in this ORBIT instance",
                ));
            }
            // The only API path that provisions new users. Count all orgs and hold the
            // lock until the insert so concurrent first logins cannot exceed the seats.
            let _seat_limit = state.seat_limit_guard().await?;
            let link_by_email =
                may_link_by_email(identity.email_verified, oidc.require_verified_email);
            match state
                .store
                .upsert_oidc_user(
                    state.default_org_id,
                    &identity.issuer,
                    &identity.subject,
                    identity.email.as_deref(),
                    identity.name.as_deref(),
                    target_role,
                    link_by_email,
                )
                .await
            {
                Ok(user) => user,
                Err(orbit_store::StoreError::Conflict(message)) => {
                    tracing::warn!(
                        issuer = %identity.issuer,
                        subject = %identity.subject,
                        "refused OIDC login: {message}"
                    );
                    return Err(ApiError::forbidden(message));
                }
                Err(error) => return Err(error.into()),
            }
        }
    };
    if user.status != "active" {
        return Err(ApiError::forbidden("user is disabled"));
    }
    if !identity.groups.is_empty() && !oidc.team_groups.is_empty() {
        if let Err(error) = sync_oidc_teams(state, user.id, user.org_id, &identity.groups).await {
            tracing::warn!(%error, "OIDC access-team sync failed");
        }
    }
    let (role, scopes) = resolve_role(state, user.org_id, &user.role).await?;
    Ok(AuthContext {
        user_id: user.id,
        org_id: user.org_id,
        email: user.email,
        display_name: user.display_name,
        role,
        method: AuthMethod::Oidc,
        scopes,
        token_id: None,
    })
}

fn team_role_rank(role: &str) -> u8 {
    match role {
        "lead" => 3,
        "member" => 2,
        "viewer" => 1,
        _ => 0,
    }
}

/// For each mapped team slug the user's groups reach: the highest role any of those
/// groups grants and the group that grants it (the name an auto-created team gets).
/// Independent of the order of the mapping and of the groups claim (#437).
fn mapped_team_roles<'a>(
    team_groups: &'a [(String, String, String)],
    groups: &[String],
) -> std::collections::BTreeMap<&'a str, (&'a str, &'a str)> {
    let mut roles: std::collections::BTreeMap<&str, (&str, &str)> = Default::default();
    for (group, slug, role) in team_groups {
        if !groups.iter().any(|value| value == group) {
            continue;
        }
        roles
            .entry(slug.as_str())
            .and_modify(|best| {
                if team_role_rank(role) > team_role_rank(best.1) {
                    *best = (group.as_str(), role.as_str());
                }
            })
            .or_insert((group.as_str(), role.as_str()));
    }
    roles
}

/// A mapped group grants at least `target`: write only when the user has no membership or
/// a lower role. A role raised by hand stays; one lowered below the mapping comes back.
fn team_sync_needs_write(current: Option<&str>, target: &str) -> bool {
    current.is_none_or(|current| team_role_rank(current) < team_role_rank(target))
}

/// Additive OIDC group -> access-team membership sync. Never removes a membership and
/// never lowers a role; a request whose memberships already satisfy the mapping writes
/// nothing.
async fn sync_oidc_teams(
    state: &AppState,
    user_id: uuid::Uuid,
    org_id: uuid::Uuid,
    groups: &[String],
) -> Result<(), ApiError> {
    if !state
        .license_status()
        .has_feature(orbit_security::FEATURE_TEAMS)
    {
        return Ok(());
    }
    let Some(oidc) = state.auth.oidc.as_ref() else {
        return Ok(());
    };
    let mapped = mapped_team_roles(&oidc.team_groups, groups);
    if mapped.is_empty() {
        return Ok(());
    }
    let teams = state.store.list_access_teams(org_id).await?;
    let current = state.store.list_user_team_roles(org_id, user_id).await?;
    for (team_slug, (group, team_role)) in mapped {
        let team = match teams.iter().find(|team| team.slug == team_slug) {
            Some(team) => team.clone(),
            None if oidc.auto_create_teams && orbit_domain::is_valid_slug(team_slug) => match state
                .store
                .create_access_team(org_id, group, team_slug, None)
                .await
            {
                Ok(team) => {
                    tracing::info!(%group, team = %team_slug, "created access team from OIDC group");
                    team
                }
                Err(error) => {
                    tracing::warn!(%error, %group, team = %team_slug, "could not create OIDC access team");
                    continue;
                }
            },
            None => {
                tracing::warn!(%group, team = %team_slug, "OIDC team mapping references a missing team");
                continue;
            }
        };
        let role_now = current
            .iter()
            .find(|(team_id, _)| *team_id == team.id)
            .map(|(_, role)| role.as_str());
        if team_sync_needs_write(role_now, team_role) {
            state
                .store
                .upsert_team_member(team.id, user_id, team_role)
                .await?;
        }
    }
    Ok(())
}

/// Resolve a stored role name to a built-in role or a custom permission set.
async fn resolve_role(
    state: &AppState,
    org_id: uuid::Uuid,
    role_name: &str,
) -> Result<(UserRole, Vec<String>), ApiError> {
    if let Ok(role) = role_name.parse::<UserRole>() {
        return Ok((role, Vec::new()));
    }
    match state
        .store
        .custom_role_permissions(org_id, role_name)
        .await?
    {
        Some(permissions) => Ok((UserRole::Viewer, permissions)),
        None => Err(ApiError::forbidden(format!(
            "user role '{role_name}' no longer exists"
        ))),
    }
}

/// Expose OIDC metadata to the web app without leaking secrets.
pub fn oidc_public_config(state: &AppState) -> serde_json::Value {
    match &state.auth.oidc {
        Some(oidc) => serde_json::json!({
            "mode": "oidc",
            "issuer": oidc.issuer,
            "clientId": oidc.client_id,
            "redirectUri": oidc.redirect_uri,
            "scopes": oidc.scopes,
        }),
        None => serde_json::json!({
            "mode": "dev",
            "devUser": state.auth.dev.email,
            "anonymousAllowed": state.auth.dev.allow_anonymous,
        }),
    }
}

/// Role an existing user should be changed to on OIDC login, if any. Only a matched group
/// mapping may change the stored role; unmapped groups, an absent claim or sync being off
/// leave it (including admin-assigned and custom roles) untouched.
/// Email-based linking to an existing user needs a non-false `email_verified` claim; with
/// `require_verified_email` the claim must be present and true.
fn may_link_by_email(email_verified: Option<bool>, require_verified_email: bool) -> bool {
    match email_verified {
        Some(verified) => verified,
        None => !require_verified_email,
    }
}

fn synced_role(
    current: &str,
    mapped: Option<UserRole>,
    sync_roles: bool,
    has_groups: bool,
) -> Option<UserRole> {
    let mapped = mapped?;
    (sync_roles && has_groups && current != mapped.as_str()).then_some(mapped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn team_sync_takes_the_highest_role_in_any_order() {
        let mapping = |pairs: &[(&str, &str, &str)]| -> Vec<(String, String, String)> {
            pairs
                .iter()
                .map(|(g, t, r)| (g.to_string(), t.to_string(), r.to_string()))
                .collect()
        };
        let groups = vec!["admins".to_string(), "devs".to_string()];
        for team_groups in [
            mapping(&[("admins", "core", "lead"), ("devs", "core", "member")]),
            mapping(&[("devs", "core", "member"), ("admins", "core", "lead")]),
        ] {
            let roles = mapped_team_roles(&team_groups, &groups);
            assert_eq!(roles.get("core"), Some(&("admins", "lead")));
        }
        let team_groups = mapping(&[("ops", "core", "lead"), ("devs", "web", "viewer")]);
        let roles = mapped_team_roles(&team_groups, &groups);
        assert_eq!(roles.len(), 1);
        assert_eq!(roles.get("web"), Some(&("devs", "viewer")));
    }

    #[test]
    fn team_sync_writes_only_to_add_or_raise() {
        assert!(team_sync_needs_write(None, "member"));
        assert!(team_sync_needs_write(Some("viewer"), "member"));
        // Unchanged claims: no write on every request.
        assert!(!team_sync_needs_write(Some("member"), "member"));
        // A lead promoted by hand keeps the role.
        assert!(!team_sync_needs_write(Some("lead"), "member"));
    }

    #[test]
    fn email_linking_needs_a_non_false_verified_claim() {
        assert!(may_link_by_email(Some(true), false));
        assert!(may_link_by_email(Some(true), true));
        assert!(!may_link_by_email(Some(false), false));
        assert!(!may_link_by_email(Some(false), true));
        assert!(may_link_by_email(None, false));
        assert!(!may_link_by_email(None, true));
    }

    #[test]
    fn unmapped_groups_keep_existing_role() {
        assert_eq!(synced_role("operator", None, true, true), None);
        assert_eq!(synced_role("custom-role", None, true, true), None);
    }

    #[test]
    fn mapped_group_changes_role_up_and_down() {
        assert_eq!(
            synced_role("viewer", Some(UserRole::Admin), true, true),
            Some(UserRole::Admin)
        );
        assert_eq!(
            synced_role("admin", Some(UserRole::Viewer), true, true),
            Some(UserRole::Viewer)
        );
        assert_eq!(
            synced_role("custom-role", Some(UserRole::Operator), true, true),
            Some(UserRole::Operator)
        );
    }

    #[test]
    fn matching_role_is_not_rewritten() {
        assert_eq!(
            synced_role("admin", Some(UserRole::Admin), true, true),
            None
        );
    }

    #[test]
    fn no_groups_or_sync_off_changes_nothing() {
        assert_eq!(
            synced_role("viewer", Some(UserRole::Admin), true, false),
            None
        );
        assert_eq!(
            synced_role("viewer", Some(UserRole::Admin), false, true),
            None
        );
    }
}
