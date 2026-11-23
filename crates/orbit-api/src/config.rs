use std::time::Duration;

use orbit_security::AuthConfig;

pub const DEFAULT_ANON_RATE_LIMIT_PER_MINUTE: u32 = 120;

#[derive(Clone, Debug)]
pub struct ApiConfig {
    pub environment: String,
    pub http_addr: String,
    pub database_url: String,
    pub db_max_connections: u32,
    pub auto_migrate: bool,
    pub cors_origins: Vec<String>,
    pub seed_demo: bool,
    /// Seed a launchable mock worker profile when a launcher is configured.
    pub seed_worker_profile: bool,
    /// Tag used for generated worker-profile image defaults.
    pub image_tag: String,
    pub default_org_slug: String,
    pub default_org_name: String,
    pub public_base_url: String,
    pub body_limit_bytes: usize,
    /// Requests per 60-second window allowed per source IP on public/unauthenticated routes.
    pub anon_rate_limit_per_minute: u32,
    /// Peers whose `X-Forwarded-For` the anonymous rate limit believes
    /// (`ORBIT_TRUSTED_PROXIES`, comma-separated CIDRs or addresses).
    pub trusted_proxies: Vec<crate::rate_limit::TrustedProxy>,
    pub auth: AuthConfig,
    pub master_key: String,
    pub launcher_url: Option<String>,
    pub launcher_token: Option<String>,
    pub worker_api_url: String,
    /// Secret mounted into launcher-created workers for private CAs.
    pub ca_secret_name: Option<String>,
    pub ca_secret_key: String,
    /// Days after which terminal executions, team runs and worker launches are archived.
    /// `0` disables the retention loop.
    pub archive_after_days: Option<u32>,
}

impl ApiConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let environment = env_or("ORBIT_ENV", "development");
        let auth = AuthConfig::from_env()?;
        let master_key = resolve_master_key(&environment)?;
        let cors_origins = env_or("ORBIT_CORS_ORIGINS", "http://localhost:3000")
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();

        Ok(Self {
            environment,
            http_addr: env_or("ORBIT_HTTP_ADDR", "0.0.0.0:8080"),
            database_url: resolve_database_url(),
            db_max_connections: env_or("ORBIT_DB_MAX_CONNECTIONS", "10")
                .parse()
                .unwrap_or(10),
            auto_migrate: env_bool("ORBIT_DB_AUTO_MIGRATE", true),
            cors_origins,
            seed_demo: env_bool("ORBIT_SEED_DEMO", true),
            seed_worker_profile: env_bool("ORBIT_SEED_WORKER_PROFILE", true),
            image_tag: env_or("ORBIT_IMAGE_TAG", "latest"),
            default_org_slug: env_or("ORBIT_DEFAULT_ORG_SLUG", "orbit"),
            default_org_name: env_or("ORBIT_DEFAULT_ORG_NAME", "ORBIT"),
            public_base_url: env_or("ORBIT_PUBLIC_BASE_URL", "http://localhost:8080"),
            body_limit_bytes: env_or("ORBIT_BODY_LIMIT_BYTES", "2097152")
                .parse()
                .unwrap_or(2 * 1024 * 1024),
            anon_rate_limit_per_minute: parse_positive_u32(
                &env_or(
                    "ORBIT_ANON_RATE_LIMIT_PER_MINUTE",
                    &DEFAULT_ANON_RATE_LIMIT_PER_MINUTE.to_string(),
                ),
                DEFAULT_ANON_RATE_LIMIT_PER_MINUTE,
            ),
            trusted_proxies: {
                let (proxies, invalid) =
                    crate::rate_limit::parse_trusted_proxies(&env_or("ORBIT_TRUSTED_PROXIES", ""));
                if !invalid.is_empty() {
                    tracing::warn!(?invalid, "ignoring invalid ORBIT_TRUSTED_PROXIES entries");
                }
                proxies
            },
            auth,
            master_key,
            launcher_url: std::env::var("ORBIT_LAUNCHER_URL")
                .ok()
                .filter(|value| !value.trim().is_empty()),
            launcher_token: std::env::var("ORBIT_LAUNCHER_TOKEN")
                .ok()
                .filter(|value| !value.trim().is_empty()),
            worker_api_url: env_or("ORBIT_WORKER_API_URL", "http://orbit-api:8080"),
            ca_secret_name: std::env::var("ORBIT_CA_SECRET_NAME")
                .ok()
                .filter(|value| !value.trim().is_empty()),
            ca_secret_key: env_or("ORBIT_CA_SECRET_KEY", "ca-bundle.crt"),
            archive_after_days: env_or("ORBIT_ARCHIVE_AFTER_DAYS", "30")
                .parse::<u32>()
                .ok()
                .filter(|days| *days > 0),
        })
    }

    pub fn is_production(&self) -> bool {
        self.environment == "production"
    }

    pub fn request_timeout(&self) -> Duration {
        Duration::from_secs(30)
    }
}

/// The documented development key (base64 of this string) and the SHA-256 derived
/// fallback. A production process must never encrypt real secrets with either.
const DEVELOPMENT_MASTER_KEY: &[u8] = b"orbit-dev-master-key-32-bytes!!!";

fn is_development_master_key(key: &str) -> bool {
    use base64::Engine;
    use sha2::{Digest, Sha256};
    let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(key.trim()) else {
        return false;
    };
    if decoded == DEVELOPMENT_MASTER_KEY {
        return true;
    }
    let mut hasher = Sha256::new();
    hasher.update(b"orbit-development-master-key-do-not-use-in-production");
    decoded == hasher.finalize().as_slice()
}

fn resolve_master_key(environment: &str) -> anyhow::Result<String> {
    if let Ok(key) = std::env::var("ORBIT_MASTER_KEY") {
        let key = key.trim();
        if !key.is_empty() {
            if environment == "production" && is_development_master_key(key) {
                anyhow::bail!(
                    "ORBIT_MASTER_KEY is a published development key and cannot be used in \
                     production; generate a new one with `openssl rand -base64 32`"
                );
            }
            return Ok(key.to_string());
        }
    }
    if environment == "production" {
        anyhow::bail!(
            "ORBIT_MASTER_KEY is required in production (base64-encoded 32-byte key); \
             generate one with `openssl rand -base64 32`"
        );
    }
    // Deterministic, clearly non-production development key so local secrets survive
    // restarts. Never used when ORBIT_ENV=production.
    use base64::Engine;
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"orbit-development-master-key-do-not-use-in-production");
    tracing::warn!(
        "ORBIT_MASTER_KEY is not set; using an insecure development key. Set ORBIT_MASTER_KEY."
    );
    Ok(base64::engine::general_purpose::STANDARD.encode(hasher.finalize()))
}

/// Resolve the PostgreSQL connection string.
///
/// `ORBIT_DATABASE_URL` wins when set (Helm/production standard). Otherwise we compose
/// one from component variables, which lets Kubernetes operators map the fields of an
/// operator-managed Secret (for example a CNPG application Secret with `host`, `port`,
/// `dbname`, `user` and `password`) without concatenating a URL themselves.
fn resolve_database_url() -> String {
    if let Ok(url) = std::env::var("ORBIT_DATABASE_URL") {
        let url = url.trim();
        if !url.is_empty() {
            return url.to_string();
        }
    }
    let host = env_opt("ORBIT_DATABASE_HOST");
    let database = env_opt("ORBIT_DATABASE_NAME");
    let user = env_opt("ORBIT_DATABASE_USER");
    let password = env_opt("ORBIT_DATABASE_PASSWORD");
    let port = env_opt("ORBIT_DATABASE_PORT")
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(5432);
    let ssl_mode = env_opt("ORBIT_DATABASE_SSLMODE").unwrap_or_else(|| "require".into());

    compose_database_url(
        host.as_deref(),
        port,
        database.as_deref(),
        user.as_deref(),
        password.as_deref(),
        &ssl_mode,
    )
    .unwrap_or_else(|| "postgres://orbit:orbit@localhost:5432/orbit".into())
}

fn compose_database_url(
    host: Option<&str>,
    port: u16,
    database: Option<&str>,
    user: Option<&str>,
    password: Option<&str>,
    ssl_mode: &str,
) -> Option<String> {
    let host = host?.trim();
    let database = database?.trim();
    let user = user?.trim();
    if host.is_empty() || database.is_empty() || user.is_empty() {
        return None;
    }
    let credentials = match password.map(str::trim).filter(|value| !value.is_empty()) {
        Some(password) => format!("{}:{}", percent_encode(user), percent_encode(password)),
        None => percent_encode(user),
    };
    Some(format!(
        "postgres://{credentials}@{host}:{port}/{}?sslmode={}",
        percent_encode(database),
        percent_encode(ssl_mode)
    ))
}

/// Percent-encode a URL user-info/path component (RFC 3986 unreserved set kept as-is).
fn percent_encode(value: &str) -> String {
    use std::fmt::Write;
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char);
            }
            _ => {
                let _ = write!(encoded, "%{byte:02X}");
            }
        }
    }
    encoded
}

fn env_opt(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// Invalid or non-positive values fall back to the default instead of disabling the
/// anonymous rate limit by accident.
fn parse_positive_u32(value: &str, default: u32) -> u32 {
    value
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn env_bool(key: &str, default: bool) -> bool {
    std::env::var(key)
        .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes"))
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composes_url_from_components() {
        let url = compose_database_url(
            Some("pg.example.internal"),
            5432,
            Some("orbit"),
            Some("orbit"),
            Some("secret"),
            "require",
        )
        .expect("composed");
        assert_eq!(
            url,
            "postgres://orbit:secret@pg.example.internal:5432/orbit?sslmode=require"
        );
    }

    #[test]
    fn percent_encodes_credentials_and_database() {
        let url = compose_database_url(
            Some("db"),
            5433,
            Some("orbit/db"),
            Some("orbit@ops"),
            Some("p@ss:w/rd?#"),
            "verify-full",
        )
        .expect("composed");
        assert_eq!(
            url,
            "postgres://orbit%40ops:p%40ss%3Aw%2Frd%3F%23@db:5433/orbit%2Fdb?sslmode=verify-full"
        );
    }

    #[test]
    fn omits_password_when_absent() {
        let url = compose_database_url(
            Some("db"),
            5432,
            Some("orbit"),
            Some("orbit"),
            None,
            "disable",
        )
        .expect("composed");
        assert_eq!(url, "postgres://orbit@db:5432/orbit?sslmode=disable");
    }

    #[test]
    fn anonymous_rate_limit_uses_positive_values_only() {
        assert_eq!(
            parse_positive_u32("3", DEFAULT_ANON_RATE_LIMIT_PER_MINUTE),
            3
        );
        assert_eq!(
            parse_positive_u32(" 25 ", DEFAULT_ANON_RATE_LIMIT_PER_MINUTE),
            25
        );
        for invalid in ["0", "-1", "abc", ""] {
            assert_eq!(
                parse_positive_u32(invalid, DEFAULT_ANON_RATE_LIMIT_PER_MINUTE),
                DEFAULT_ANON_RATE_LIMIT_PER_MINUTE,
                "value {invalid}"
            );
        }
    }

    #[test]
    fn development_master_keys_are_rejected() {
        use base64::Engine;
        let published = base64::engine::general_purpose::STANDARD.encode(DEVELOPMENT_MASTER_KEY);
        assert!(is_development_master_key(&published));
        let mut hasher = sha2::Sha256::new();
        use sha2::Digest;
        hasher.update(b"orbit-development-master-key-do-not-use-in-production");
        let derived = base64::engine::general_purpose::STANDARD.encode(hasher.finalize());
        assert!(is_development_master_key(&derived));
        assert!(!is_development_master_key(
            &base64::engine::general_purpose::STANDARD.encode([42u8; 32])
        ));
        assert!(!is_development_master_key("not-base64"));
    }

    #[test]
    fn requires_host_database_and_user() {
        assert!(
            compose_database_url(Some("db"), 5432, None, Some("orbit"), None, "require").is_none()
        );
        assert!(
            compose_database_url(None, 5432, Some("orbit"), Some("orbit"), None, "require")
                .is_none()
        );
    }
}
