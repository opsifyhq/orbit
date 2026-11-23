//! Hardening flags for worker containers started with the Docker CLI.
//!
//! Kubernetes worker pods drop all capabilities, forbid privilege escalation and run as
//! a non-root user. The two Docker paths (`orbit-launcher` for Docker targets and the
//! worker's per-execution `docker` driver) start containers that run agent code too, so
//! they get the same profile from this module.
//!
//! Installer settings (both components read the same variables):
//!
//! | Variable | Default | Meaning |
//! |---|---|---|
//! | `ORBIT_DOCKER_USER` | `10001:10001` | `--user`; empty keeps the image's user |
//! | `ORBIT_DOCKER_PIDS_LIMIT` | `4096` | `--pids-limit`; `0` disables it |
//! | `ORBIT_DOCKER_HOST_GATEWAY` | `auto` | `--add-host host.docker.internal:host-gateway`: `auto` only when the API URL uses that name, `true` always, `false` never |

pub const DOCKER_USER_ENV: &str = "ORBIT_DOCKER_USER";
pub const DOCKER_PIDS_LIMIT_ENV: &str = "ORBIT_DOCKER_PIDS_LIMIT";
pub const DOCKER_HOST_GATEWAY_ENV: &str = "ORBIT_DOCKER_HOST_GATEWAY";

/// The `orbit` user every ORBIT worker image creates.
pub const DEFAULT_DOCKER_USER: &str = "10001:10001";
/// Generous for Node-based harnesses (threads count as PIDs) while stopping fork bombs.
pub const DEFAULT_DOCKER_PIDS_LIMIT: u32 = 4096;

const HOST_GATEWAY_NAME: &str = "host.docker.internal";

/// When a container gets `host.docker.internal` mapped to the Docker host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostGateway {
    /// Only when the API URL handed to the container points at `host.docker.internal`.
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerSandbox {
    pub user: Option<String>,
    pub pids_limit: Option<u32>,
    pub host_gateway: HostGateway,
}

impl Default for DockerSandbox {
    fn default() -> Self {
        Self {
            user: Some(DEFAULT_DOCKER_USER.into()),
            pids_limit: Some(DEFAULT_DOCKER_PIDS_LIMIT),
            host_gateway: HostGateway::Auto,
        }
    }
}

/// `uid`, `uid:gid`, `name` or `name:group` as Docker accepts for `--user`.
fn valid_user(value: &str) -> bool {
    let part = |segment: &str| {
        !segment.is_empty()
            && segment.len() <= 32
            && segment
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    };
    match value.split_once(':') {
        Some((user, group)) => part(user) && part(group),
        None => part(value),
    }
}

impl DockerSandbox {
    pub fn from_env() -> Self {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// Build from a variable lookup. Invalid values fall back to the defaults, so a
    /// typo never silently removes a restriction.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let defaults = Self::default();
        let user = match lookup(DOCKER_USER_ENV).map(|value| value.trim().to_string()) {
            None => defaults.user,
            Some(value) if value.is_empty() => None,
            Some(value) if valid_user(&value) => Some(value),
            Some(_) => defaults.user,
        };
        let pids_limit = match lookup(DOCKER_PIDS_LIMIT_ENV)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        {
            None => defaults.pids_limit,
            Some(value) => match value.parse::<u32>() {
                Ok(0) => None,
                Ok(limit) => Some(limit),
                Err(_) => defaults.pids_limit,
            },
        };
        let host_gateway = match lookup(DOCKER_HOST_GATEWAY_ENV)
            .map(|value| value.trim().to_ascii_lowercase())
            .as_deref()
        {
            Some("true" | "1" | "yes" | "always") => HostGateway::Always,
            Some("false" | "0" | "no" | "never") => HostGateway::Never,
            _ => HostGateway::Auto,
        };
        Self {
            user,
            pids_limit,
            host_gateway,
        }
    }

    /// `docker run` flags. `api_url` is the URL the container uses to reach orbit-api.
    pub fn args(&self, api_url: &str) -> Vec<String> {
        let mut args: Vec<String> = vec![
            "--cap-drop".into(),
            "ALL".into(),
            "--security-opt".into(),
            "no-new-privileges".into(),
        ];
        if let Some(limit) = self.pids_limit {
            args.push("--pids-limit".into());
            args.push(limit.to_string());
        }
        if let Some(user) = &self.user {
            args.push("--user".into());
            args.push(user.clone());
        }
        let gateway = match self.host_gateway {
            HostGateway::Always => true,
            HostGateway::Never => false,
            HostGateway::Auto => url_host(api_url) == Some(HOST_GATEWAY_NAME),
        };
        if gateway {
            args.push("--add-host".into());
            args.push(format!("{HOST_GATEWAY_NAME}:host-gateway"));
        }
        args
    }
}

/// Host part of `scheme://[userinfo@]host[:port][/path]`, lowercased comparison is left
/// to the caller (Docker names are lowercase).
fn url_host(url: &str) -> Option<&str> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host_port = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    if host_port.starts_with('[') {
        return host_port.split_once(']').map(|(host, _)| &host[1..]);
    }
    Some(host_port.split(':').next().unwrap_or(host_port))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn sandbox(vars: &[(&str, &str)]) -> DockerSandbox {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        DockerSandbox::from_lookup(|key| vars.get(key).cloned())
    }

    fn pair(args: &[String], flag: &str) -> Option<String> {
        args.windows(2)
            .find(|window| window[0] == flag)
            .map(|window| window[1].clone())
    }

    #[test]
    fn defaults_drop_capabilities_and_run_as_the_orbit_user() {
        let args = sandbox(&[]).args("http://orbit-api:8080");
        assert_eq!(pair(&args, "--cap-drop").as_deref(), Some("ALL"));
        assert_eq!(
            pair(&args, "--security-opt").as_deref(),
            Some("no-new-privileges")
        );
        assert_eq!(pair(&args, "--pids-limit").as_deref(), Some("4096"));
        assert_eq!(pair(&args, "--user").as_deref(), Some("10001:10001"));
        assert!(!args.iter().any(|arg| arg == "--add-host"));
    }

    #[test]
    fn host_gateway_is_added_only_when_the_api_url_needs_it() {
        let auto = sandbox(&[]);
        for url in [
            "http://host.docker.internal:8080",
            "http://user:pw@host.docker.internal:8080/api",
            "https://host.docker.internal",
        ] {
            assert_eq!(
                pair(&auto.args(url), "--add-host").as_deref(),
                Some("host.docker.internal:host-gateway"),
                "{url}"
            );
        }
        for url in [
            "http://orbit-api:8080",
            "http://host.docker.internal.evil:8080",
            "http://example.com/host.docker.internal",
            "not a url",
        ] {
            assert!(!auto.args(url).iter().any(|a| a == "--add-host"), "{url}");
        }
        let always = sandbox(&[(DOCKER_HOST_GATEWAY_ENV, "true")]);
        assert!(always
            .args("http://orbit-api:8080")
            .iter()
            .any(|a| a == "--add-host"));
        let never = sandbox(&[(DOCKER_HOST_GATEWAY_ENV, "false")]);
        assert!(!never
            .args("http://host.docker.internal:8080")
            .iter()
            .any(|a| a == "--add-host"));
    }

    #[test]
    fn installer_overrides_and_invalid_values_keep_the_restriction() {
        let custom = sandbox(&[
            (DOCKER_USER_ENV, "1000:1000"),
            (DOCKER_PIDS_LIMIT_ENV, "512"),
        ]);
        let args = custom.args("http://api");
        assert_eq!(pair(&args, "--user").as_deref(), Some("1000:1000"));
        assert_eq!(pair(&args, "--pids-limit").as_deref(), Some("512"));

        let off = sandbox(&[(DOCKER_USER_ENV, ""), (DOCKER_PIDS_LIMIT_ENV, "0")]);
        let args = off.args("http://api");
        assert!(!args.iter().any(|a| a == "--user" || a == "--pids-limit"));
        // Capabilities and no-new-privileges are not configurable.
        assert_eq!(pair(&args, "--cap-drop").as_deref(), Some("ALL"));

        let invalid = sandbox(&[
            (DOCKER_USER_ENV, "root --privileged"),
            (DOCKER_PIDS_LIMIT_ENV, "lots"),
        ]);
        let args = invalid.args("http://api");
        assert_eq!(pair(&args, "--user").as_deref(), Some("10001:10001"));
        assert_eq!(pair(&args, "--pids-limit").as_deref(), Some("4096"));
    }
}
