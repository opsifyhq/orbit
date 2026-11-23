//! Offline license verification and entitlements.
//!
//! Licenses are Ed25519-signed JWS compact tokens. Verification is fully local:
//! public keys are compiled in from `license-public-keys.json` and the private signing key
//! never ships. Debug builds additionally accept a local dev key and
//! `ORBIT_LICENSE_PUBLIC_KEYS` for development; release builds ignore runtime keys.
//! See `docs/decisions/0010-offline-licensing.md`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use base64::Engine;
use chrono::{DateTime, Utc};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

pub const FEATURE_CUSTOM_ROLES: &str = "custom_roles";
pub const FEATURE_TEAMS: &str = "teams";
pub const FEATURE_RESOURCE_PERMISSIONS: &str = "resource_permissions";
pub const FEATURE_SHARED_CONNECTIONS: &str = "shared_connections";
pub const FEATURE_QUOTAS: &str = "quotas";
pub const FEATURE_MARKETPLACE: &str = "marketplace";

/// Feature flags that are signed and verified, but have no shipped product feature yet.
/// They are kept in the verifier for forward compatibility and must not be advertised as
/// available. Gating that exists today uses [`FEATURE_TEAMS`] and
/// [`FEATURE_CUSTOM_ROLES`].
pub const RESERVED_FEATURES: &[&str] = &[
    FEATURE_RESOURCE_PERMISSIONS,
    FEATURE_SHARED_CONNECTIONS,
    FEATURE_QUOTAS,
    FEATURE_MARKETPLACE,
];

/// Public verification keys shipped in the official build. Replace the `{}` with a JSON
/// object of `kid -> base64url raw public key` before issuing licenses. Public keys are
/// not secret; never add a private key here.
const BUILD_KEYS_JSON: &str = include_str!("../license-public-keys.json");

/// Development-only key. Release builds must contain the vendor key above.
#[cfg(debug_assertions)]
const DEV_KEYS: &[(&str, &str)] = &[(
    "orbit-dev-2026-01",
    "TcdJnlUQOeZVx2zcz1cGTBKdvC1MI9jWKKd7s0AqX8A",
)];

const DEFAULT_LICENSE_FILE: &str = "/etc/orbit/license.jwt";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct LicenseLimits {
    pub seats: Option<u64>,
    pub agents: Option<u64>,
    pub workers: Option<u64>,
    /// Signed claims spell it `executions_per_day` (ADR 0010); the API returns camelCase.
    #[serde(alias = "executions_per_day")]
    pub executions_per_day: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
struct LicenseClaims {
    v: u8,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    customer: Option<String>,
    #[serde(default)]
    plan: Option<String>,
    #[serde(default)]
    org_slug: Option<String>,
    #[serde(default)]
    features: Vec<String>,
    #[serde(default)]
    limits: LicenseLimits,
    issued_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    #[serde(default = "default_grace_days")]
    grace_days: u32,
    #[serde(default)]
    trial: bool,
}

fn default_grace_days() -> u32 {
    30
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LicenseStatus {
    /// `community`, `enterprise`, … (the plan while the license is usable).
    pub tier: String,
    #[serde(default)]
    pub customer: Option<String>,
    #[serde(default)]
    pub plan: Option<String>,
    #[serde(default)]
    pub license_id: Option<String>,
    #[serde(default)]
    pub org_slug: Option<String>,
    /// Raw signed flags. Reserved flags remain here because the verifier accepts them and
    /// existing gates still read them; use [`LicenseStatus::available_features`] for display.
    #[serde(default)]
    pub features: Vec<String>,
    /// Signed flags backed by shipped functionality. This is what the API/UI advertises.
    #[serde(default)]
    pub available_features: Vec<String>,
    #[serde(default)]
    pub limits: LicenseLimits,
    #[serde(default)]
    pub issued_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
    /// Whole days until expiry; negative once the expiry date has passed.
    #[serde(default)]
    pub days_remaining: Option<i64>,
    /// True while inside the post-expiry grace window.
    #[serde(default)]
    pub grace: bool,
    /// End of the grace window. Set for signed licenses so the UI can show a positive
    /// "grace ends in X days" countdown instead of negative expiry days.
    #[serde(default)]
    pub grace_ends_at: Option<DateTime<Utc>>,
    /// False for community, invalid, or fully expired licenses.
    #[serde(default)]
    pub valid: bool,
    #[serde(default)]
    pub trial: bool,
    #[serde(default)]
    pub error: Option<String>,
}

impl LicenseStatus {
    pub fn community() -> Self {
        Self {
            tier: "community".into(),
            customer: None,
            plan: None,
            license_id: None,
            org_slug: None,
            features: Vec::new(),
            available_features: Vec::new(),
            limits: LicenseLimits::default(),
            issued_at: None,
            expires_at: None,
            days_remaining: None,
            grace: false,
            grace_ends_at: None,
            valid: false,
            trial: false,
            error: None,
        }
    }

    pub fn has_feature(&self, feature: &str) -> bool {
        self.valid && self.features.iter().any(|value| value == feature)
    }

    /// Feature flags safe to present as available. Reserved flags stay in `features` so
    /// verifier and gate behaviour do not change, but are never advertised to users.
    pub fn available_features(&self) -> Vec<String> {
        if !self.valid {
            return Vec::new();
        }
        self.features
            .iter()
            .filter(|feature| !RESERVED_FEATURES.iter().any(|reserved| reserved == feature))
            .cloned()
            .collect()
    }

    /// Whole days left in the grace window, clamped at zero. `None` outside grace.
    pub fn grace_days_remaining(&self) -> Option<i64> {
        if !self.grace {
            return None;
        }
        self.grace_ends_at
            .map(|end| (end - Utc::now()).num_days().max(0))
    }

    pub fn limit(&self, limit: &str) -> Option<u64> {
        if !self.valid {
            return None;
        }
        match limit {
            "seats" => self.limits.seats,
            "agents" => self.limits.agents,
            "workers" => self.limits.workers,
            "executions_per_day" => self.limits.executions_per_day,
            _ => None,
        }
    }
}

#[derive(Clone)]
pub struct LicenseVerifier {
    keys: BTreeMap<String, VerifyingKey>,
}

impl Default for LicenseVerifier {
    fn default() -> Self {
        Self::new()
    }
}

impl LicenseVerifier {
    pub fn new() -> Self {
        let mut keys = BTreeMap::new();
        match serde_json::from_str::<BTreeMap<String, String>>(BUILD_KEYS_JSON) {
            Ok(embedded) => {
                for (kid, encoded) in embedded {
                    match decode_key(&encoded) {
                        Some(key) => {
                            keys.insert(kid, key);
                        }
                        None => tracing::warn!(%kid, "ignoring invalid embedded license key"),
                    }
                }
            }
            Err(error) => {
                tracing::error!(%error, "license-public-keys.json is not a JSON object");
            }
        }

        // Development convenience only. Release builds deliberately ignore runtime keys:
        // otherwise anyone could self-sign a license.
        #[cfg(debug_assertions)]
        {
            for (kid, encoded) in DEV_KEYS {
                if let Some(key) = decode_key(encoded) {
                    keys.insert((*kid).to_string(), key);
                }
            }
            if let Ok(value) = std::env::var("ORBIT_LICENSE_PUBLIC_KEYS") {
                if let Ok(custom) = serde_json::from_str::<BTreeMap<String, String>>(&value) {
                    for (kid, encoded) in custom {
                        if let Some(key) = decode_key(&encoded) {
                            keys.insert(kid, key);
                        } else {
                            tracing::warn!(%kid, "ignoring invalid ORBIT_LICENSE_PUBLIC_KEYS entry");
                        }
                    }
                } else {
                    tracing::warn!(
                        "ORBIT_LICENSE_PUBLIC_KEYS is not a JSON object of kid -> base64 key"
                    );
                }
            }
        }
        Self { keys }
    }

    pub fn verify(&self, token: &str) -> LicenseStatus {
        match self.verify_inner(token) {
            Ok(status) => status,
            Err(error) => {
                let mut status = LicenseStatus::community();
                status.error = Some(error);
                status
            }
        }
    }

    fn verify_inner(&self, token: &str) -> Result<LicenseStatus, String> {
        let parts: Vec<&str> = token.trim().split('.').collect();
        if parts.len() != 3 {
            return Err("license is not a compact JWS token".into());
        }
        let header: serde_json::Value =
            decode_json(parts[0]).map_err(|_| "license header is not valid JSON".to_string())?;
        let kid = header
            .get("kid")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "license header is missing kid".to_string())?;
        let alg = header
            .get("alg")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        if alg != "EdDSA" {
            return Err("license alg must be EdDSA".into());
        }
        let key = self
            .keys
            .get(kid)
            .ok_or_else(|| format!("unknown license key '{kid}'"))?;
        let signature = decode_bytes(parts[2])?;
        let signature = Signature::from_slice(&signature)
            .map_err(|_| "license signature is malformed".to_string())?;
        let signed = format!("{}.{}", parts[0], parts[1]);
        key.verify(signed.as_bytes(), &signature)
            .map_err(|_| "license signature is invalid".to_string())?;

        let claims: LicenseClaims =
            decode_json(parts[1]).map_err(|_| "license payload is not valid JSON".to_string())?;
        if claims.v != 1 {
            return Err(format!("unsupported license version {}", claims.v));
        }
        let now = Utc::now();
        let grace_end = claims.expires_at + chrono::Duration::days(claims.grace_days as i64);
        let expired = now > claims.expires_at;
        let in_grace = expired && now <= grace_end;
        let valid = now <= grace_end;
        let days_remaining = (claims.expires_at - now).num_days();
        let error = if !valid {
            Some("license expired".to_string())
        } else if in_grace {
            Some("license is in its grace period".to_string())
        } else {
            None
        };
        let features = claims.features;
        let available_features = if valid {
            features
                .iter()
                .filter(|feature| !RESERVED_FEATURES.iter().any(|reserved| reserved == feature))
                .cloned()
                .collect()
        } else {
            Vec::new()
        };

        Ok(LicenseStatus {
            tier: if valid {
                claims.plan.clone().unwrap_or_else(|| "enterprise".into())
            } else {
                "community".into()
            },
            customer: claims.customer,
            plan: claims.plan,
            license_id: claims.id,
            org_slug: claims.org_slug,
            features,
            available_features,
            limits: claims.limits,
            issued_at: Some(claims.issued_at),
            expires_at: Some(claims.expires_at),
            days_remaining: Some(days_remaining),
            grace: in_grace,
            grace_ends_at: Some(grace_end),
            valid,
            trial: claims.trial,
            error,
        })
    }
}

fn decode_json<T: serde::de::DeserializeOwned>(value: &str) -> Result<T, serde_json::Error> {
    let bytes =
        decode_bytes(value).map_err(|error| serde_json::Error::io(std::io::Error::other(error)))?;
    serde_json::from_slice(&bytes)
}

fn decode_bytes(value: &str) -> Result<Vec<u8>, String> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(value.trim_end_matches('='))
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(value))
        .map_err(|_| "license contains invalid base64".to_string())
}

fn decode_key(value: &str) -> Option<VerifyingKey> {
    let bytes = decode_bytes(value).ok()?;
    let bytes: [u8; 32] = bytes.try_into().ok()?;
    VerifyingKey::from_bytes(&bytes).ok()
}

/// Where the license comes from: inline env, mounted file, or the default path.
#[derive(Debug, Clone)]
pub struct LicenseSource {
    pub token: Option<String>,
    pub file: Option<PathBuf>,
}

impl LicenseSource {
    pub fn from_env() -> Self {
        let token = std::env::var("ORBIT_LICENSE")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let file = std::env::var("ORBIT_LICENSE_FILE")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                Path::new(DEFAULT_LICENSE_FILE)
                    .exists()
                    .then(|| PathBuf::from(DEFAULT_LICENSE_FILE))
            });
        Self { token, file }
    }

    pub fn load(&self) -> LicenseStatus {
        let verifier = LicenseVerifier::new();
        match &self.token {
            Some(token) => verifier.verify(token),
            None => match &self.file {
                Some(path) => match std::fs::read_to_string(path) {
                    Ok(token) => verifier.verify(&token),
                    Err(error) => {
                        tracing::warn!(%error, path = %path.display(), "could not read license file");
                        LicenseStatus::community()
                    }
                },
                None => LicenseStatus::community(),
            },
        }
    }
}

/// Shared, hot-reloadable license status for the API.
#[derive(Clone)]
pub struct LicenseHandle {
    status: std::sync::Arc<RwLock<LicenseStatus>>,
    source: LicenseSource,
}

impl LicenseHandle {
    pub fn load(source: LicenseSource) -> Self {
        let status = source.load();
        tracing::info!(
            tier = %status.tier,
            valid = status.valid,
            customer = ?status.customer,
            days_remaining = ?status.days_remaining,
            "license loaded"
        );
        Self {
            status: std::sync::Arc::new(RwLock::new(status)),
            source,
        }
    }

    /// Build a handle from an already verified status. Used by tests and by callers that
    /// obtain a status from another trusted source; no network or file access is performed.
    pub fn from_status(status: LicenseStatus) -> Self {
        Self {
            status: std::sync::Arc::new(RwLock::new(status)),
            source: LicenseSource {
                token: None,
                file: None,
            },
        }
    }

    pub fn status(&self) -> LicenseStatus {
        self.status
            .read()
            .map(|status| status.clone())
            .unwrap_or_else(|_| LicenseStatus::community())
    }

    pub fn source(&self) -> &LicenseSource {
        &self.source
    }

    pub fn reload(&self) {
        let next = self.source.load();
        if let Ok(mut status) = self.status.write() {
            *status = next;
        }
    }

    pub fn require_feature(&self, feature: &str) -> Result<(), String> {
        let status = self.status();
        if status.has_feature(feature) {
            Ok(())
        } else {
            Err(format!(
                "feature '{feature}' requires an active enterprise license"
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use rand::rngs::OsRng;

    fn token_for(signing: &SigningKey, kid: &str, claims: serde_json::Value) -> String {
        let header = serde_json::json!({ "alg": "EdDSA", "kid": kid });
        let encode = |value: &serde_json::Value| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(value).unwrap())
        };
        let signing_input = format!("{}.{}", encode(&header), encode(&claims));
        let signature = signing.sign(signing_input.as_bytes());
        format!(
            "{signing_input}.{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature.to_bytes())
        )
    }

    fn verifier_for(signing: &SigningKey, kid: &str) -> LicenseVerifier {
        let mut keys = BTreeMap::new();
        keys.insert(
            kid.to_string(),
            VerifyingKey::from_bytes(&signing.verifying_key().to_bytes()).unwrap(),
        );
        LicenseVerifier { keys }
    }

    fn claims(expires_in_days: i64, grace_days: u32) -> serde_json::Value {
        let now = Utc::now();
        serde_json::json!({
            "v": 1,
            "id": "lic_test",
            "customer": "Test",
            "plan": "enterprise",
            "features": ["custom_roles", "quotas"],
            "limits": { "seats": 10, "agents": 100 },
            "issued_at": now - chrono::Duration::days(1),
            "expires_at": now + chrono::Duration::days(expires_in_days),
            "grace_days": grace_days
        })
    }

    #[test]
    fn build_public_key_is_loaded() {
        let verifier = LicenseVerifier::new();
        assert!(verifier.keys.contains_key("orbit-2026-01"));
    }

    #[test]
    fn verifies_a_valid_license() {
        let signing = SigningKey::generate(&mut OsRng);
        let token = token_for(&signing, "test-key", claims(30, 30));
        let status = verifier_for(&signing, "test-key").verify(&token);
        assert!(status.valid);
        assert!(!status.grace);
        assert_eq!(status.tier, "enterprise");
        assert!(status.has_feature(FEATURE_CUSTOM_ROLES));
        assert_eq!(status.limit("seats"), Some(10));
    }

    #[test]
    fn reads_the_documented_snake_case_limits() {
        let signing = SigningKey::from_bytes(&[7u8; 32]);
        let mut claims = claims(30, 30);
        claims["limits"] = serde_json::json!({ "workers": 25, "executions_per_day": 10000 });
        let status = verifier_for(&signing, "test").verify(&token_for(&signing, "test", claims));
        assert!(status.valid);
        assert_eq!(status.limit("executions_per_day"), Some(10000));
        assert_eq!(status.limit("workers"), Some(25));
    }

    #[test]
    fn grace_window_is_usable_but_flagged() {
        let signing = SigningKey::generate(&mut OsRng);
        let token = token_for(&signing, "test-key", claims(-1, 30));
        let status = verifier_for(&signing, "test-key").verify(&token);
        assert!(status.valid);
        assert!(status.grace);
        assert!(status.has_feature(FEATURE_CUSTOM_ROLES));
    }

    #[test]
    fn expired_license_falls_back_to_community() {
        let signing = SigningKey::generate(&mut OsRng);
        let token = token_for(&signing, "test-key", claims(-40, 30));
        let status = verifier_for(&signing, "test-key").verify(&token);
        assert!(!status.valid);
        assert_eq!(status.tier, "community");
        assert!(!status.has_feature(FEATURE_CUSTOM_ROLES));
    }

    #[test]
    fn reserved_flags_are_verified_but_not_advertised() {
        let signing = SigningKey::from_bytes(&[11u8; 32]);
        let mut claims = claims(30, 30);
        claims["features"] = serde_json::json!([
            "custom_roles",
            "teams",
            "resource_permissions",
            "shared_connections",
            "quotas",
            "marketplace"
        ]);
        let status = verifier_for(&signing, "test").verify(&token_for(&signing, "test", claims));
        assert!(status.valid);
        assert_eq!(status.features.len(), 6);
        assert_eq!(status.available_features, ["custom_roles", "teams"]);
        // Gates keep reading signed flags; advertising does not.
        assert!(status.has_feature(FEATURE_QUOTAS));
    }

    #[test]
    fn grace_countdown_is_positive_while_expiry_days_are_negative() {
        let signing = SigningKey::from_bytes(&[12u8; 32]);
        let status =
            verifier_for(&signing, "test").verify(&token_for(&signing, "test", claims(-1, 30)));
        assert!(status.valid);
        assert!(status.grace);
        assert!(status.days_remaining.expect("days") <= 0);
        assert!(status.grace_ends_at.is_some());
        let days = status.grace_days_remaining().expect("grace days");
        assert!(
            (1..=30).contains(&days),
            "unexpected grace countdown: {days}"
        );
    }

    #[test]
    fn expired_license_has_no_grace_or_advertised_features() {
        let signing = SigningKey::from_bytes(&[13u8; 32]);
        let status =
            verifier_for(&signing, "test").verify(&token_for(&signing, "test", claims(-40, 30)));
        assert!(!status.valid);
        assert!(!status.grace);
        assert_eq!(status.grace_days_remaining(), None);
        assert!(status.days_remaining.expect("days") < 0);
        assert!(status.available_features.is_empty());
        assert_eq!(status.tier, "community");
    }

    #[cfg(debug_assertions)]
    #[test]
    fn inline_license_loads_without_network_or_a_mounted_file() {
        let signing = SigningKey::from_bytes(&[14u8; 32]);
        let token = token_for(&signing, "offline-test", claims(30, 30));
        // Debug builds accept a runtime key for tests; verification still uses only local
        // keys and no HTTP client or file access.
        std::env::set_var(
            "ORBIT_LICENSE_PUBLIC_KEYS",
            serde_json::json!({
                "offline-test": base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .encode(signing.verifying_key().to_bytes())
            })
            .to_string(),
        );
        let source = LicenseSource {
            token: Some(token),
            file: None,
        };
        let status = source.load();
        std::env::remove_var("ORBIT_LICENSE_PUBLIC_KEYS");
        assert!(status.valid, "{status:?}");
    }

    #[cfg(not(debug_assertions))]
    #[test]
    fn release_build_ignores_runtime_public_keys() {
        let signing = SigningKey::generate(&mut OsRng);
        let token = token_for(&signing, "evil-key", claims(30, 30));
        std::env::set_var(
            "ORBIT_LICENSE_PUBLIC_KEYS",
            serde_json::json!({
                "evil-key": base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .encode(signing.verifying_key().to_bytes())
            })
            .to_string(),
        );
        let status = LicenseVerifier::new().verify(&token);
        std::env::remove_var("ORBIT_LICENSE_PUBLIC_KEYS");
        assert!(!status.valid, "release builds must not trust runtime keys");
    }

    #[test]
    fn rejects_tampering_and_unknown_keys() {
        let signing = SigningKey::generate(&mut OsRng);
        let token = token_for(&signing, "test-key", claims(30, 30));
        let mut tampered = token.clone();
        tampered.push('x');
        assert!(!verifier_for(&signing, "test-key").verify(&tampered).valid);
        assert!(!verifier_for(&signing, "other-key").verify(&token).valid);

        let wrong = SigningKey::generate(&mut OsRng);
        let status = verifier_for(&wrong, "test-key").verify(&token);
        assert!(!status.valid);
        assert!(status.error.unwrap().contains("signature"));
    }
}
