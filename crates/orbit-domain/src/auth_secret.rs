//! Which Kubernetes Secrets a worker profile may mount as harness auth.
//!
//! The launcher mounts the Secret named by `WorkerProfile.authSecretName` into the agent
//! pod. Without a rule, anyone who can edit a profile could name any Secret in the
//! namespace (including ORBIT's own). The installer controls the rule through
//! `ORBIT_WORKER_AUTH_SECRETS`; both `orbit-api` (on write) and `orbit-launcher` (on launch)
//! evaluate it with this module, so neither depends on the other.

/// Env var holding the comma-separated allowlist, set by the Helm chart on the API and
/// the launcher.
pub const AUTH_SECRETS_ENV: &str = "ORBIT_WORKER_AUTH_SECRETS";

/// Names the shipped docs and web editor use today.
pub const DEFAULT_AUTH_SECRETS: &str = "orbit-opencode-auth*,orbit-pi-auth*";

/// Installer-controlled allowlist: exact names, or `prefix*` patterns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthSecretPolicy {
    patterns: Vec<String>,
}

/// A Kubernetes Secret name must be a DNS-1123 subdomain; names mounted here are held to
/// the stricter label form: lowercase alphanumerics and `-`, starting and ending with an
/// alphanumeric, at most 63 characters.
pub fn is_dns_label(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 63
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
        && bytes[0] != b'-'
        && bytes[bytes.len() - 1] != b'-'
}

impl AuthSecretPolicy {
    /// Parse a comma-separated list. Blank entries are ignored; an empty list allows
    /// nothing (auth Secrets disabled).
    pub fn parse(list: &str) -> Self {
        Self {
            patterns: list
                .split(',')
                .map(str::trim)
                .filter(|pattern| !pattern.is_empty())
                .map(str::to_string)
                .collect(),
        }
    }

    pub fn default_policy() -> Self {
        Self::parse(DEFAULT_AUTH_SECRETS)
    }

    /// Policy from `ORBIT_WORKER_AUTH_SECRETS`; unset means the default. A variable that
    /// is set but empty means "allow none".
    pub fn from_env() -> Self {
        match std::env::var(AUTH_SECRETS_ENV) {
            Ok(list) => Self::parse(&list),
            Err(_) => Self::default_policy(),
        }
    }

    pub fn patterns(&self) -> &[String] {
        &self.patterns
    }

    /// `Ok` when `name` is a valid DNS label and matches the allowlist.
    pub fn check(&self, name: &str) -> Result<(), String> {
        if !is_dns_label(name) {
            return Err(
                "must be a valid Kubernetes name: lowercase letters, digits and '-', at most 63 characters"
                    .into(),
            );
        }
        let allowed = self
            .patterns
            .iter()
            .any(|pattern| match pattern.strip_suffix('*') {
                Some(prefix) => name.starts_with(prefix),
                None => name == pattern,
            });
        if allowed {
            Ok(())
        } else {
            Err(format!(
                "'{name}' is not an allowed harness auth Secret; the installer allows: {}",
                if self.patterns.is_empty() {
                    "none".to_string()
                } else {
                    self.patterns.join(", ")
                }
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_accepts_the_documented_names() {
        let policy = AuthSecretPolicy::default_policy();
        assert!(policy.check("orbit-opencode-auth").is_ok());
        assert!(policy.check("orbit-pi-auth").is_ok());
        assert!(policy.check("orbit-pi-auth-team-a").is_ok());
    }

    #[test]
    fn platform_and_unlisted_secrets_are_refused() {
        let policy = AuthSecretPolicy::default_policy();
        for name in [
            "orbit",
            "orbit-orbit",
            "orbit-secrets",
            "postgres",
            "my-auth",
        ] {
            assert!(policy.check(name).is_err(), "{name}");
        }
    }

    #[test]
    fn invalid_names_are_refused_even_when_a_pattern_matches() {
        let policy = AuthSecretPolicy::parse("orbit-*");
        for name in ["orbit-A", "orbit-", "orbit_x", "orbit-a.b", "../etc", ""] {
            assert!(policy.check(name).is_err(), "{name}");
        }
        assert!(policy.check("orbit-x").is_ok());
        assert!(policy.check(&format!("orbit-{}", "a".repeat(60))).is_err());
    }

    #[test]
    fn exact_entries_do_not_act_as_prefixes_and_empty_allows_none() {
        let policy = AuthSecretPolicy::parse(" team-auth , ");
        assert!(policy.check("team-auth").is_ok());
        assert!(policy.check("team-auth-2").is_err());
        assert!(AuthSecretPolicy::parse("").check("team-auth").is_err());
    }
}
