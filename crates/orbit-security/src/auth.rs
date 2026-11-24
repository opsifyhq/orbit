use std::collections::BTreeSet;
use std::str::FromStr;
use std::time::{Duration, Instant};

use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use orbit_domain::{OrganizationId, UserId, UserRole};

use crate::{Permission, SecurityError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    /// Self-hosted development: a seeded org/user, optional anonymous access.
    Dev,
    /// OIDC bearer tokens validated against the provider's JWKS.
    Oidc,
}

impl AuthMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Oidc => "oidc",
        }
    }
}

#[derive(Debug, Clone)]
pub struct DevAuthConfig {
    pub email: String,
    pub display_name: String,
    pub org_slug: String,
    pub org_name: String,
    pub role: UserRole,
    pub allow_anonymous: bool,
    /// Convenience bearer token accepted in dev mode (`orbit-dev-token` by default).
    pub dev_token: String,
}

impl Default for DevAuthConfig {
    fn default() -> Self {
        Self {
            email: "dev@orbit.local".into(),
            display_name: "ORBIT Developer".into(),
            org_slug: "orbit-dev".into(),
            org_name: "ORBIT Development".into(),
            role: UserRole::Owner,
            allow_anonymous: true,
            dev_token: "orbit-dev-token".into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct OidcConfig {
    pub issuer: String,
    pub audience: String,
    pub jwks_url: String,
    /// OAuth/OIDC client id used by the web app. Defaults to the audience.
    pub client_id: String,
    pub client_secret: Option<String>,
    pub redirect_uri: String,
    pub scopes: Vec<String>,
    pub auto_provision: bool,
    pub default_role: UserRole,
    /// Claim that carries IdP groups/roles (default `groups`).
    pub role_claim: String,
    /// Claim used as the unique OIDC identity/email (default `email`). Set to `name`
    /// when multiple IdP accounts share an email address.
    pub email_claim: String,
    /// Claim values mapped to ORBIT roles. Order is irrelevant; the most privileged
    /// matching role wins.
    pub group_roles: Vec<(String, UserRole)>,
    /// IdP group -> (access team slug, team role).
    pub team_groups: Vec<(String, String, String)>,
    /// Create access teams for mapped groups when the team slug does not exist.
    pub auto_create_teams: bool,
    /// Deny users whose IdP groups do not match any configured role/team mapping.
    pub require_mapped_group: bool,
    /// Re-evaluate the stored role on login, only when a group mapping matches. Without this,
    /// the mapped role is only used at auto-provisioning.
    pub sync_roles: bool,
    /// Only link an incoming identity to an existing (e.g. invited) user by email when the
    /// token carries `email_verified: true`. A missing claim then also blocks linking.
    /// An explicit `email_verified: false` always blocks linking, regardless of this flag.
    pub require_verified_email: bool,
    pub clock_skew_seconds: u64,
}

#[derive(Debug, Clone)]
pub struct AuthConfig {
    pub mode: AuthMode,
    pub dev: DevAuthConfig,
    pub oidc: Option<OidcConfig>,
    pub allow_api_tokens: bool,
}

impl AuthConfig {
    /// Build from environment. Fails closed: OIDC mode without issuer/JWKS is an error.
    pub fn from_env() -> Result<Self, SecurityError> {
        let mode = match std::env::var("ORBIT_AUTH_MODE")
            .unwrap_or_else(|_| "dev".into())
            .as_str()
        {
            "dev" => AuthMode::Dev,
            "oidc" => AuthMode::Oidc,
            other => {
                return Err(SecurityError::Config(format!(
                    "unsupported ORBIT_AUTH_MODE: {other}"
                )))
            }
        };

        let dev = DevAuthConfig {
            email: env_or("ORBIT_DEV_USER_EMAIL", "dev@orbit.local"),
            display_name: env_or("ORBIT_DEV_USER_NAME", "ORBIT Developer"),
            org_slug: env_or("ORBIT_DEV_ORG_SLUG", "orbit-dev"),
            org_name: env_or("ORBIT_DEV_ORG_NAME", "ORBIT Development"),
            role: UserRole::from_str(&env_or("ORBIT_DEV_USER_ROLE", "owner"))
                .map_err(SecurityError::Config)?,
            allow_anonymous: env_bool("ORBIT_DEV_AUTH_ALLOW_ANONYMOUS", true),
            dev_token: env_or("ORBIT_DEV_TOKEN", "orbit-dev-token"),
        };

        let oidc = match mode {
            AuthMode::Dev => None,
            AuthMode::Oidc => {
                let issuer = std::env::var("ORBIT_OIDC_ISSUER").map_err(|_| {
                    SecurityError::Config("ORBIT_OIDC_ISSUER is required in oidc mode".into())
                })?;
                let audience = std::env::var("ORBIT_OIDC_AUDIENCE").map_err(|_| {
                    SecurityError::Config("ORBIT_OIDC_AUDIENCE is required in oidc mode".into())
                })?;
                let jwks_url = std::env::var("ORBIT_OIDC_JWKS_URL").map_err(|_| {
                    SecurityError::Config("ORBIT_OIDC_JWKS_URL is required in oidc mode".into())
                })?;
                let client_id = env_or("ORBIT_OIDC_CLIENT_ID", &audience);
                let redirect_uri = env_or(
                    "ORBIT_OIDC_REDIRECT_URI",
                    "http://localhost:3000/auth/callback",
                );
                Some(OidcConfig {
                    issuer,
                    audience,
                    jwks_url,
                    client_id,
                    client_secret: std::env::var("ORBIT_OIDC_CLIENT_SECRET")
                        .ok()
                        .filter(|value| !value.trim().is_empty()),
                    redirect_uri,
                    scopes: env_or("ORBIT_OIDC_SCOPES", "openid profile email")
                        .split_whitespace()
                        .map(str::to_string)
                        .collect(),
                    auto_provision: env_bool("ORBIT_OIDC_AUTO_PROVISION", true),
                    default_role: UserRole::from_str(&env_or("ORBIT_OIDC_DEFAULT_ROLE", "viewer"))
                        .map_err(SecurityError::Config)?,
                    role_claim: env_or("ORBIT_OIDC_ROLE_CLAIM", "groups"),
                    email_claim: env_or("ORBIT_OIDC_EMAIL_CLAIM", "email"),
                    group_roles: parse_group_roles(&env_or("ORBIT_OIDC_GROUP_ROLES", ""))?,
                    team_groups: parse_team_groups(&env_or("ORBIT_OIDC_TEAM_GROUPS", ""))?,
                    auto_create_teams: env_bool("ORBIT_OIDC_AUTO_CREATE_TEAMS", false),
                    require_mapped_group: env_bool("ORBIT_OIDC_REQUIRE_MAPPED_GROUP", false),
                    sync_roles: env_bool("ORBIT_OIDC_SYNC_ROLES", false),
                    require_verified_email: env_bool(
                        "ORBIT_OIDC_REQUIRE_VERIFIED_EMAIL",
                        DEFAULT_REQUIRE_VERIFIED_EMAIL,
                    ),
                    clock_skew_seconds: env_or("ORBIT_OIDC_CLOCK_SKEW_SECONDS", "60")
                        .parse()
                        .unwrap_or(60),
                })
            }
        };

        Ok(Self {
            mode,
            dev,
            oidc,
            allow_api_tokens: env_bool("ORBIT_ALLOW_API_TOKENS", true),
        })
    }
}

/// Extract claim values from a string or array claim.
fn claim_values(value: Option<&serde_json::Value>) -> Vec<String> {
    match value {
        Some(serde_json::Value::String(value)) => vec![value.clone()],
        Some(serde_json::Value::Array(values)) => values
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

/// Pick the most privileged role whose mapped group is present.
pub fn highest_mapped_role(groups: &[String], mapping: &[(String, UserRole)]) -> Option<UserRole> {
    fn rank(role: UserRole) -> u8 {
        match role {
            UserRole::Owner => 4,
            UserRole::Admin => 3,
            UserRole::Operator => 2,
            UserRole::Viewer => 1,
        }
    }
    mapping
        .iter()
        .filter(|(group, _)| groups.iter().any(|value| value == group))
        .map(|(_, role)| *role)
        .max_by_key(|role| rank(*role))
}

/// Parse `group=role,group2=admin` (or a JSON object) into role mappings.
fn parse_group_roles(value: &str) -> Result<Vec<(String, UserRole)>, SecurityError> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(Vec::new());
    }
    if value.starts_with('{') {
        let map: std::collections::BTreeMap<String, String> = serde_json::from_str(value)
            .map_err(|error| SecurityError::Config(format!("ORBIT_OIDC_GROUP_ROLES: {error}")))?;
        return map
            .into_iter()
            .map(|(group, role)| {
                let parsed = UserRole::from_str(&role).map_err(|error| {
                    SecurityError::Config(format!(
                        "ORBIT_OIDC_GROUP_ROLES: invalid role for '{group}': {error}"
                    ))
                })?;
                Ok((group, parsed))
            })
            .collect();
    }
    value
        .split(',')
        .filter(|pair| !pair.trim().is_empty())
        .map(|pair| {
            let (group, role) = pair.split_once('=').ok_or_else(|| {
                SecurityError::Config(
                    "ORBIT_OIDC_GROUP_ROLES must be 'group=role,group=role'".into(),
                )
            })?;
            let role = UserRole::from_str(role.trim()).map_err(|error| {
                SecurityError::Config(format!(
                    "ORBIT_OIDC_GROUP_ROLES: invalid role for '{}': {error}",
                    group.trim()
                ))
            })?;
            Ok((group.trim().to_string(), role))
        })
        .collect()
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// Default for `ORBIT_OIDC_REQUIRE_VERIFIED_EMAIL`: email-based linking to an existing
/// user needs `email_verified: true` unless an operator turns this off (#325).
pub const DEFAULT_REQUIRE_VERIFIED_EMAIL: bool = true;

fn env_bool(key: &str, default: bool) -> bool {
    std::env::var(key)
        .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes"))
        .unwrap_or(default)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    Dev,
    Oidc,
    ApiToken,
    Anonymous,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthContext {
    pub user_id: UserId,
    pub org_id: OrganizationId,
    pub email: String,
    pub display_name: String,
    pub role: UserRole,
    pub method: AuthMethod,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_id: Option<Uuid>,
}

impl AuthContext {
    pub fn can(&self, permission: Permission) -> bool {
        if self.scopes.is_empty() {
            return Permission::allows(self.role, permission);
        }
        let scopes: BTreeSet<&str> = self.scopes.iter().map(String::as_str).collect();
        scopes.contains(permission.as_str()) || scopes.contains("*")
    }

    /// Effective permissions: explicit token/role scopes when present, otherwise the
    /// built-in role's permission set.
    pub fn permissions(&self) -> Vec<String> {
        if self.scopes.is_empty() {
            Permission::all_for(self.role)
                .into_iter()
                .map(str::to_string)
                .collect()
        } else {
            self.scopes.clone()
        }
    }

    pub fn require(&self, permission: Permission) -> Result<(), SecurityError> {
        if self.can(permission) {
            Ok(())
        } else {
            Err(SecurityError::Forbidden(format!(
                "missing permission {}",
                permission.as_str()
            )))
        }
    }
}

/// Claims extracted from a validated OIDC access token / ID token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcIdentity {
    pub subject: String,
    pub issuer: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    /// The `email_verified` claim when the token carries one (`None` when absent).
    #[serde(default)]
    pub email_verified: Option<bool>,
    /// Values of the configured role claim (groups/roles).
    #[serde(default)]
    pub groups: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RawClaims {
    sub: String,
    iss: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    preferred_username: Option<String>,
    #[serde(default)]
    email_verified: Option<serde_json::Value>,
    #[serde(flatten)]
    extra: std::collections::BTreeMap<String, serde_json::Value>,
}

/// Parse `email_verified`. Some IdPs send the string `"true"`/`"false"`; anything else
/// (absent, null, unrecognised) counts as not asserted.
fn parse_email_verified(value: Option<&serde_json::Value>) -> Option<bool> {
    match value? {
        serde_json::Value::Bool(flag) => Some(*flag),
        serde_json::Value::String(text) => match text.trim().to_ascii_lowercase().as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// JWKS cache state. The async mutex around it is held across a refresh, so concurrent
/// requests share one fetch instead of each starting their own.
#[derive(Default)]
struct JwksCache {
    set: Option<JwkSet>,
    fetched: Option<Instant>,
    /// Last time a fetch was started, successful or not.
    last_attempt: Option<Instant>,
}

enum KeySource {
    Jwks {
        url: String,
        client: reqwest::Client,
        cache: tokio::sync::Mutex<JwksCache>,
        /// How long a fetched key set is considered fresh.
        ttl: Duration,
        /// Minimum time between two fetches, whatever the reason (expiry, unknown `kid`,
        /// previous failure), so unauthenticated bad tokens cannot drive traffic to the IdP.
        min_refresh: Duration,
    },
}

const JWKS_TTL: Duration = Duration::from_secs(600);
const JWKS_MIN_REFRESH: Duration = Duration::from_secs(30);

/// Validates OIDC JWTs. The JWKS is cached for [`JWKS_TTL`]; an unknown `kid` or an expired
/// cache triggers at most one refresh per [`JWKS_MIN_REFRESH`], and a failed refresh keeps
/// serving the cached keys.
pub struct JwtValidator {
    issuer: String,
    audience: String,
    role_claim: String,
    email_claim: String,
    clock_skew: u64,
    keys: KeySource,
}

const ALLOWED_ALGORITHMS: [Algorithm; 6] = [
    Algorithm::RS256,
    Algorithm::RS384,
    Algorithm::RS512,
    Algorithm::ES256,
    Algorithm::ES384,
    Algorithm::PS256,
];

impl JwtValidator {
    pub fn new(config: &OidcConfig) -> Result<Self, SecurityError> {
        Self::with_jwks_timing(config, JWKS_TTL, JWKS_MIN_REFRESH)
    }

    /// Like [`JwtValidator::new`] with explicit JWKS cache timing.
    pub fn with_jwks_timing(
        config: &OidcConfig,
        ttl: Duration,
        min_refresh: Duration,
    ) -> Result<Self, SecurityError> {
        let client = crate::http::client_with_custom_ca(Duration::from_secs(10));
        Ok(Self {
            issuer: config.issuer.clone(),
            audience: config.audience.clone(),
            role_claim: config.role_claim.clone(),
            email_claim: config.email_claim.clone(),
            clock_skew: config.clock_skew_seconds,
            keys: KeySource::Jwks {
                url: config.jwks_url.clone(),
                client,
                cache: tokio::sync::Mutex::new(JwksCache::default()),
                ttl,
                min_refresh,
            },
        })
    }

    pub async fn validate(&self, token: &str) -> Result<OidcIdentity, SecurityError> {
        let header = decode_header(token).map_err(|_| SecurityError::Unauthorized)?;
        if !ALLOWED_ALGORITHMS.contains(&header.alg) {
            return Err(SecurityError::Unauthorized);
        }
        let key = self.resolve_key(&header).await?;

        let mut validation = Validation::new(header.alg);
        validation.set_issuer(&[self.issuer.as_str()]);
        validation.set_audience(&[self.audience.as_str()]);
        validation.leeway = self.clock_skew;
        validation.validate_exp = true;

        let data = decode::<RawClaims>(token, &key, &validation)
            .map_err(|_| SecurityError::Unauthorized)?;
        let claims = data.claims;
        let groups = claim_values(claims.extra.get(&self.role_claim));
        let email = match self.email_claim.as_str() {
            "email" => claims.email.clone(),
            "name" => claims.name.clone(),
            "preferred_username" => claims.preferred_username.clone(),
            other => claim_values(claims.extra.get(other)).into_iter().next(),
        }
        .or(claims.email);
        Ok(OidcIdentity {
            subject: claims.sub,
            issuer: claims.iss,
            email,
            name: claims.name.or(claims.preferred_username),
            email_verified: parse_email_verified(claims.email_verified.as_ref()),
            groups,
        })
    }

    async fn resolve_key(
        &self,
        header: &jsonwebtoken::Header,
    ) -> Result<DecodingKey, SecurityError> {
        match &self.keys {
            KeySource::Jwks {
                url,
                client,
                cache,
                ttl,
                min_refresh,
            } => {
                let kid = header.kid.clone().ok_or(SecurityError::Unauthorized)?;
                let mut guard = cache.lock().await;
                let lookup = |guard: &JwksCache| {
                    guard
                        .set
                        .as_ref()
                        .and_then(|set| set.find(&kid))
                        .and_then(|jwk| DecodingKey::from_jwk(jwk).ok())
                };
                let fresh = guard.fetched.is_some_and(|at| at.elapsed() < *ttl);
                if fresh {
                    if let Some(key) = lookup(&guard) {
                        return Ok(key);
                    }
                }
                // Expired cache or unknown `kid`: refresh, but never more often than
                // `min_refresh`, so a stream of bad tokens is not a stream of IdP requests.
                let may_refresh = guard
                    .last_attempt
                    .is_none_or(|at| at.elapsed() >= *min_refresh);
                if may_refresh {
                    guard.last_attempt = Some(Instant::now());
                    let fetched: Result<JwkSet, ()> = async {
                        client
                            .get(url)
                            .send()
                            .await
                            .and_then(|r| r.error_for_status())
                            .map_err(|_| ())?
                            .json()
                            .await
                            .map_err(|_| ())
                    }
                    .await;
                    match fetched {
                        Ok(set) => {
                            guard.set = Some(set);
                            guard.fetched = Some(Instant::now());
                        }
                        Err(()) => {
                            tracing::warn!("JWKS refresh failed; serving cached keys if any");
                        }
                    }
                }
                // Also covers a failed refresh: keep serving whatever copy is cached.
                match lookup(&guard) {
                    Some(key) => Ok(key),
                    None if guard.set.is_none() => Err(SecurityError::Unavailable(
                        "JWKS endpoint unreachable".into(),
                    )),
                    None => Err(SecurityError::Unauthorized),
                }
            }
        }
    }
}

/// Hash an API token for storage/lookup. Tokens are high-entropy; SHA-256 is sufficient.
pub fn hash_api_token(token: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

/// Generate `(plaintext, hash)`. The plaintext is shown to the user exactly once.
pub fn generate_api_token() -> (String, String) {
    use base64::Engine;
    use rand_core::RngCore;
    let mut bytes = [0u8; 32];
    rand_core::OsRng.fill_bytes(&mut bytes);
    let token = format!(
        "orbit_sk_{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    );
    let hash = hash_api_token(&token);
    (token, hash)
}

/// One-time token material returned on creation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssuedApiToken {
    pub token: String,
    pub hash: String,
    pub prefix: String,
}

pub fn issue_api_token() -> IssuedApiToken {
    let (token, hash) = generate_api_token();
    let prefix = token.chars().take(16).collect();
    IssuedApiToken {
        token,
        hash,
        prefix,
    }
}

/// Worker tokens are separate from user API tokens so the API can reject them on
/// control-plane routes and accept them only on the worker surface.
pub fn issue_worker_token() -> IssuedApiToken {
    use base64::Engine;
    use rand_core::RngCore;
    let mut bytes = [0u8; 32];
    rand_core::OsRng.fill_bytes(&mut bytes);
    let token = format!(
        "orbit_wk_{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    );
    let hash = hash_api_token(&token);
    let prefix = token.chars().take(16).collect();
    IssuedApiToken {
        token,
        hash,
        prefix,
    }
}

/// Extract a bearer token from an Authorization header value.
pub fn bearer_token(header: Option<&str>) -> Option<&str> {
    let value = header?.trim();
    let (scheme, token) = value.split_once(' ')?;
    if scheme.eq_ignore_ascii_case("bearer") && !token.trim().is_empty() {
        Some(token.trim())
    } else {
        None
    }
}

/// Parse `group=team-slug:role,group2=platform:member` or JSON
/// `{"group":"team-slug:role"}`. Roles are lead/member/viewer.
fn parse_team_groups(value: &str) -> Result<Vec<(String, String, String)>, SecurityError> {
    fn parse_one(group: &str, value: &str) -> Result<(String, String, String), SecurityError> {
        let (team, role) = value.split_once(':').ok_or_else(|| {
            SecurityError::Config(format!(
                "ORBIT_OIDC_TEAM_GROUPS: expected team-slug:role for '{group}'"
            ))
        })?;
        let team = team.trim();
        let role = role.trim();
        if team.is_empty() || !matches!(role, "lead" | "member" | "viewer") {
            return Err(SecurityError::Config(format!(
                "ORBIT_OIDC_TEAM_GROUPS: invalid mapping for '{group}'"
            )));
        }
        Ok((group.trim().to_string(), team.to_string(), role.to_string()))
    }
    let value = value.trim();
    if value.is_empty() {
        return Ok(Vec::new());
    }
    if value.starts_with('{') {
        let map: std::collections::BTreeMap<String, String> = serde_json::from_str(value)
            .map_err(|error| SecurityError::Config(format!("ORBIT_OIDC_TEAM_GROUPS: {error}")))?;
        return map
            .iter()
            .map(|(group, value)| parse_one(group, value))
            .collect();
    }
    value
        .split(',')
        .filter(|pair| !pair.trim().is_empty())
        .map(|pair| {
            let (group, value) = pair.split_once('=').ok_or_else(|| {
                SecurityError::Config(
                    "ORBIT_OIDC_TEAM_GROUPS must be 'group=team-slug:role'".into(),
                )
            })?;
            parse_one(group, value)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_and_verifies_tokens() {
        let (token, hash) = generate_api_token();
        assert!(token.starts_with("orbit_sk_"));
        assert_eq!(hash, hash_api_token(&token));
        assert_ne!(hash, hash_api_token("other"));
    }

    #[test]
    fn parses_email_verified_claim() {
        use serde_json::json;
        assert_eq!(parse_email_verified(None), None);
        assert_eq!(parse_email_verified(Some(&json!(true))), Some(true));
        assert_eq!(parse_email_verified(Some(&json!(false))), Some(false));
        assert_eq!(parse_email_verified(Some(&json!("False"))), Some(false));
        assert_eq!(parse_email_verified(Some(&json!("true"))), Some(true));
        assert_eq!(parse_email_verified(Some(&json!("maybe"))), None);
        assert_eq!(parse_email_verified(Some(&json!(null))), None);
    }

    #[test]
    fn parses_bearer_header() {
        assert_eq!(bearer_token(Some("Bearer abc")), Some("abc"));
        assert_eq!(bearer_token(Some("bearer abc")), Some("abc"));
        assert_eq!(bearer_token(Some("Basic abc")), None);
        assert_eq!(bearer_token(None), None);
    }

    #[test]
    fn parses_team_group_mappings() {
        let mappings = parse_team_groups("platform=platform:member, leads=platform:lead").unwrap();
        assert_eq!(mappings.len(), 2);
        assert_eq!(mappings[1].0, "leads");
        assert_eq!(mappings[1].1, "platform");
        assert_eq!(mappings[1].2, "lead");
        assert!(parse_team_groups("platform=platform:owner").is_err());
        assert!(parse_team_groups("platform:member").is_err());
        assert!(parse_team_groups("").unwrap().is_empty());
    }

    #[test]
    fn parses_group_role_mappings() {
        let mappings = parse_group_roles("orbit-admins=admin, orbit-operators=operator").unwrap();
        assert_eq!(
            mappings,
            vec![
                ("orbit-admins".to_string(), UserRole::Admin),
                ("orbit-operators".to_string(), UserRole::Operator),
            ]
        );
        let json = parse_group_roles(r#"{"orbit-owners":"owner"}"#).unwrap();
        assert_eq!(json, vec![("orbit-owners".to_string(), UserRole::Owner)]);
        assert!(parse_group_roles("missing-role").is_err());
        assert!(parse_group_roles("group=superuser").is_err());
        assert!(parse_group_roles("").unwrap().is_empty());
    }

    #[test]
    fn picks_the_highest_mapped_role() {
        let mapping = vec![
            ("viewers".to_string(), UserRole::Viewer),
            ("operators".to_string(), UserRole::Operator),
            ("admins".to_string(), UserRole::Admin),
        ];
        assert_eq!(
            highest_mapped_role(&["viewers".into(), "admins".into()], &mapping),
            Some(UserRole::Admin)
        );
        assert_eq!(highest_mapped_role(&["unknown".into()], &mapping), None);
        assert_eq!(highest_mapped_role(&[], &mapping), None);
    }

    #[test]
    fn extracts_claim_values() {
        assert_eq!(
            claim_values(Some(&serde_json::json!("orbit-admins"))),
            vec!["orbit-admins".to_string()]
        );
        assert_eq!(
            claim_values(Some(&serde_json::json!(["a", "b"]))),
            vec!["a".to_string(), "b".to_string()]
        );
        assert!(claim_values(Some(&serde_json::json!({"a": 1}))).is_empty());
        assert!(claim_values(None).is_empty());
    }

    /// Minimal JWKS server: counts requests, answers with an empty key set.
    async fn jwks_server(
        status: &'static str,
    ) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/jwks", listener.local_addr().unwrap());
        let hits = std::sync::Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                counter.fetch_add(1, Ordering::SeqCst);
                let mut buffer = [0u8; 2048];
                let _ = socket.read(&mut buffer).await;
                let body = r#"{"keys":[]}"#;
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            }
        });
        (url, hits)
    }

    fn oidc_config(jwks_url: &str) -> OidcConfig {
        OidcConfig {
            issuer: "https://idp.test".into(),
            audience: "orbit".into(),
            jwks_url: jwks_url.into(),
            client_id: "orbit".into(),
            client_secret: None,
            redirect_uri: "http://localhost/cb".into(),
            scopes: vec![],
            auto_provision: false,
            default_role: UserRole::Viewer,
            role_claim: "groups".into(),
            email_claim: "email".into(),
            group_roles: vec![],
            team_groups: vec![],
            auto_create_teams: false,
            require_mapped_group: false,
            sync_roles: false,
            require_verified_email: false,
            clock_skew_seconds: 5,
        }
    }

    fn token_with_kid(kid: &str) -> String {
        use base64::Engine;
        let b64 = |text: &str| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(text);
        format!(
            "{}.{}.{}",
            b64(&format!(r#"{{"alg":"RS256","typ":"JWT","kid":"{kid}"}}"#)),
            b64("{}"),
            b64("sig")
        )
    }

    #[tokio::test]
    async fn unknown_kid_floods_cause_one_jwks_fetch_per_interval() {
        let (url, hits) = jwks_server("200 OK").await;
        let validator = JwtValidator::with_jwks_timing(
            &oidc_config(&url),
            Duration::from_secs(600),
            Duration::from_secs(60),
        )
        .unwrap();
        for index in 0..100 {
            let result = validator
                .validate(&token_with_kid(&format!("unknown-{index}")))
                .await;
            assert!(matches!(result, Err(SecurityError::Unauthorized)));
        }
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn failing_jwks_endpoint_is_not_hammered() {
        let (url, hits) = jwks_server("500 Internal Server Error").await;
        let validator = JwtValidator::with_jwks_timing(
            &oidc_config(&url),
            Duration::from_secs(600),
            Duration::from_secs(60),
        )
        .unwrap();
        for _ in 0..50 {
            let result = validator.validate(&token_with_kid("k")).await;
            assert!(matches!(result, Err(SecurityError::Unavailable(_))));
        }
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
