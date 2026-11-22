use std::ffi::{OsStr, OsString};

const INHERITED_ENV_ALLOWLIST: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "TERM",
    "LANG",
    "TMPDIR",
    "TMP",
    "TEMP",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "REQUESTS_CA_BUNDLE",
    "CURL_CA_BUNDLE",
    "NODE_EXTRA_CA_CERTS",
    // Custom CA bundle for git run by the agent (bash) and by gh/glab (#300).
    "GIT_SSL_CAINFO",
    "GIT_SSL_CAPATH",
    "SYSTEMROOT",
    "WINDIR",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "COMSPEC",
    "PATHEXT",
];

pub(crate) fn child_environment(
    inherited: impl IntoIterator<Item = (OsString, OsString)>,
    explicit: impl IntoIterator<Item = (OsString, OsString)>,
) -> Vec<(OsString, OsString)> {
    let mut environment = Vec::new();
    for (name, value) in inherited {
        if is_inherited_name(&name) {
            set_environment(&mut environment, name, value);
        }
    }
    for (name, value) in explicit {
        if !is_worker_control_name(&name) {
            set_environment(&mut environment, name, value);
        }
    }
    environment
}

fn is_inherited_name(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    INHERITED_ENV_ALLOWLIST
        .iter()
        .any(|allowed| name.eq_ignore_ascii_case(allowed))
        || name
            .get(..4)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("XDG_"))
        || name
            .get(..3)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("LC_"))
}

fn is_worker_control_name(name: &OsStr) -> bool {
    name.to_str().is_some_and(|name| {
        name.eq_ignore_ascii_case("ORBIT_WORKER_TOKEN")
            || name.eq_ignore_ascii_case("ORBIT_API_URL")
    })
}

fn set_environment(environment: &mut Vec<(OsString, OsString)>, name: OsString, value: OsString) {
    environment.retain(|(existing, _)| !same_environment_name(existing, &name));
    environment.push((name, value));
}

fn same_environment_name(left: &OsStr, right: &OsStr) -> bool {
    match (left.to_str(), right.to_str()) {
        (Some(left), Some(right)) => {
            #[cfg(windows)]
            {
                left.eq_ignore_ascii_case(right)
            }
            #[cfg(not(windows))]
            {
                left == right
            }
        }
        _ => left == right,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::process::Command;

    const CHILD_PROBE: &str = "ORBIT_HARNESS_ENV_PROBE";

    #[test]
    fn environment_probe_child() {
        if std::env::var_os(CHILD_PROBE).is_none() {
            return;
        }

        for (name, expected) in [
            ("PATH", "safe-path"),
            ("HOME", "safe-home"),
            ("LANG", "C"),
            ("XDG_CACHE_HOME", "safe-xdg-cache"),
            ("LC_ALL", "C.UTF-8"),
            ("SSL_CERT_FILE", "safe-cert-file"),
            ("REQUESTS_CA_BUNDLE", "safe-requests-ca"),
            ("NODE_EXTRA_CA_CERTS", "safe-node-ca"),
            ("GIT_SSL_CAINFO", "safe-git-ca"),
            ("ORBIT_REQUEST_VALUE", "request-value"),
            ("ORBIT_BROKER_URL", "http://127.0.0.1:1234"),
            ("ORBIT_BROKER_TOKEN", "execution-scoped-token"),
        ] {
            assert_eq!(std::env::var(name).as_deref(), Ok(expected), "{name}");
        }

        for name in [
            "ORBIT_WORKER_TOKEN",
            "ORBIT_API_URL",
            "WORKER_PRIVATE_VALUE",
            "DATABASE_URL",
        ] {
            assert!(std::env::var_os(name).is_none(), "{name} leaked to child");
        }
    }

    #[tokio::test]
    async fn child_process_gets_allowlisted_and_explicit_values_only() {
        let inherited = [
            ("PATH", "safe-path"),
            ("HOME", "safe-home"),
            ("LANG", "C"),
            ("XDG_CACHE_HOME", "safe-xdg-cache"),
            ("LC_ALL", "C.UTF-8"),
            ("SSL_CERT_FILE", "safe-cert-file"),
            ("REQUESTS_CA_BUNDLE", "safe-requests-ca"),
            ("NODE_EXTRA_CA_CERTS", "safe-node-ca"),
            ("GIT_SSL_CAINFO", "safe-git-ca"),
            ("ORBIT_WORKER_TOKEN", "worker-token"),
            ("ORBIT_API_URL", "http://worker-api"),
            ("WORKER_PRIVATE_VALUE", "private-worker-value"),
            ("DATABASE_URL", "postgres://worker-db"),
        ]
        .into_iter()
        .map(|(name, value)| (OsString::from(name), OsString::from(value)));
        let explicit = [
            (CHILD_PROBE, "1"),
            ("ORBIT_REQUEST_VALUE", "request-value"),
            ("ORBIT_BROKER_URL", "http://127.0.0.1:1234"),
            ("ORBIT_BROKER_TOKEN", "execution-scoped-token"),
            ("ORBIT_WORKER_TOKEN", "must-not-override"),
            ("ORBIT_API_URL", "must-not-override"),
        ]
        .into_iter()
        .map(|(name, value)| (OsString::from(name), OsString::from(value)));

        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "process_env::tests::environment_probe_child",
                "--nocapture",
            ])
            .env_clear()
            .envs(child_environment(inherited, explicit));
        let output = command.output().await.unwrap();
        assert!(
            output.status.success(),
            "child environment probe failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
