//! Git operations for coding agents.
//!
//! Credentials are injected only into the short-lived git child process via
//! `GIT_ASKPASS`; they are not written to the repository remote URL or workspace files.

use std::io::Write;
use std::path::Path;
use std::process::{Output, Stdio};
use std::time::Duration;

use serde_json::Value;
use tempfile::TempPath;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::client::{WorkerClient, WorkerConnection};

const ORBIT_GENERATED_PATHS: &[&str] = &[
    "AGENTS.md",
    "opencode.json",
    ".opencode/agents",
    ".opencode/plugins/orbit-policy.ts",
    ".opencode/skills",
    ".pi/mcp.json",
    ".pi/extensions/orbit-policy.ts",
    ".pi/skills",
    ".pi-sessions",
];
const GIT_CLONE_TIMEOUT: Duration = Duration::from_secs(300);
const GIT_PUSH_TIMEOUT: Duration = Duration::from_secs(300);
/// Local-only git operations (config, add, reset, commit): these never touch the network.
const GIT_LOCAL_TIMEOUT: Duration = Duration::from_secs(120);

/// Why a git operation was stopped before it finished. Carried inside the `anyhow` error
/// so callers can tell a cancel from a timeout without matching on message text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitAbort {
    Cancelled,
    TimedOut { op: &'static str, seconds: u64 },
}

impl std::fmt::Display for GitAbort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => write!(f, "repository checkout cancelled"),
            Self::TimedOut { op, seconds } => {
                write!(f, "git {op} timed out after {seconds} seconds")
            }
        }
    }
}

impl std::error::Error for GitAbort {}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoSpec {
    pub connection_id: Option<uuid::Uuid>,
    pub repository: String,
    #[serde(default)]
    pub path: String,
    // `primary` in the payload only orders the manifest; where a repository lives is
    // decided by `repo_directory` alone.
    #[serde(default)]
    pub branch: Option<String>,
}

/// Where a repository of the workspace lives. The single rule for checkout and for the
/// broker's repo tools, which used to disagree about a primary repository with an empty
/// path in a multi-repository workspace (#307): the workspace root holds the repository
/// only when it is the only one; otherwise each repository without a path gets a
/// directory named after it.
pub fn repo_directory(workspace: &Path, spec: &RepoSpec, single: bool) -> std::path::PathBuf {
    if !spec.path.is_empty() {
        workspace.join(&spec.path)
    } else if single {
        workspace.to_path_buf()
    } else {
        workspace.join(spec.repository.split('/').next_back().unwrap_or("repo"))
    }
}

pub fn repo_specs(input: &Value) -> Vec<RepoSpec> {
    input
        .get("payload")
        .and_then(|payload| payload.get("repositories"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| serde_json::from_value::<RepoSpec>(item.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

fn validate_repo_path(path: &str) -> anyhow::Result<()> {
    if path.is_empty() {
        return Ok(());
    }
    if path.starts_with('/') || path.starts_with('\\') || path.contains(":") {
        anyhow::bail!("repository path must be a relative workspace path");
    }
    if path
        .split('/')
        .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        anyhow::bail!("repository path cannot escape the workspace");
    }
    Ok(())
}

fn git_available() -> bool {
    which("git")
}

fn which(binary: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|path| path.join(binary).is_file()))
        .unwrap_or(false)
}

/// Environment variables a worker-side git child may inherit. Everything else
/// (notably `ORBIT_WORKER_TOKEN`, `ORBIT_API_URL`, provider keys) is dropped.
///
/// `GIT_SSL_CAINFO` / `GIT_SSL_CAPATH` carry the custom CA bundle that the Helm chart and
/// the launcher mount (`caCertificates`): git built against libcurl-gnutls, as in the
/// Debian-based worker images, ignores `SSL_CERT_FILE` (#300). `GIT_SSL_NO_VERIFY` is
/// deliberately not inherited.
const GIT_ENV_ALLOWLIST: &[&str] = &[
    "PATH",
    "HOME",
    "LANG",
    "TMPDIR",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "CURL_CA_BUNDLE",
    "GIT_SSL_CAINFO",
    "GIT_SSL_CAPATH",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
];

/// Overrides that beat repository-local config for keys that would otherwise execute
/// workspace-controlled programs.
const GIT_HARDENING_CONFIG: &[&str] = &[
    "core.hooksPath=/dev/null",
    "core.fsmonitor=false",
    "core.sshCommand=",
    "diff.external=",
    "protocol.file.allow=never",
    "protocol.ext.allow=never",
    "commit.gpgsign=false",
];

fn is_git_inherited_name(name: &str) -> bool {
    GIT_ENV_ALLOWLIST.contains(&name) || name.starts_with("LC_")
}

/// The single constructor for every worker-side git invocation.
///
/// The agent can write anywhere in the workspace, including `.git/`, so git must not
/// run with the worker's environment or honour workspace-controlled execution hooks:
/// the environment is cleared and rebuilt from an allowlist, system and global config
/// are disabled, and hooks, fsmonitor, ssh command and external diff are overridden.
/// Repository-local `filter.*` and similar drivers cannot be overridden by name ahead of
/// time, so callers must run [`ensure_safe_repo_config`] before using a repository.
fn git_command(workspace: &Path) -> Command {
    let mut command = Command::new("git");
    command.env_clear();
    for (name, value) in std::env::vars_os() {
        if name.to_str().is_some_and(is_git_inherited_name) {
            command.env(name, value);
        }
    }
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_ATTR_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .current_dir(workspace);
    for setting in GIT_HARDENING_CONFIG {
        command.arg("-c").arg(setting);
    }
    command
}

/// True when a config key can make git execute a program or load more config from a
/// path the agent controls.
fn is_dangerous_config_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    let section = key.split('.').next().unwrap_or("");
    if matches!(
        section,
        "filter" | "include" | "includeif" | "credential" | "gpg"
    ) {
        return true;
    }
    // `url.*.insteadOf` / `pushInsteadOf` rewrite even an explicit push URL, and `http.*`
    // (proxy, sslVerify, sslCAInfo, extraHeader) can redirect or intercept the request
    // that carries the connection token (#301). Clones never write either section.
    if matches!(section, "url" | "http") {
        return true;
    }
    if (key.starts_with("diff.") && (key.ends_with(".textconv") || key.ends_with(".command")))
        || (key.starts_with("merge.") && key.ends_with(".driver"))
    {
        return true;
    }
    matches!(
        key.as_str(),
        "core.pager"
            | "core.editor"
            | "core.askpass"
            | "core.gitproxy"
            | "core.attributesfile"
            | "sequence.editor"
    )
}

/// Refuse to run git when the repository config defines drivers that execute programs.
///
/// Refusing (rather than blanking each driver with `-c filter.<name>.clean=`) is more
/// robust: it covers every driver kind (filter, textconv, merge, credential helper,
/// includes) and cannot miss a name. System and global config are disabled, so any hit
/// comes from the workspace itself; legitimate clones define none of these keys.
async fn ensure_safe_repo_config(
    workspace: &Path,
    cancel: &CancellationToken,
) -> anyhow::Result<()> {
    let mut command = git_command(workspace);
    command.args(["config", "--list", "--name-only", "--includes", "-z"]);
    let output = run_git(command, cancel, GIT_LOCAL_TIMEOUT, "config").await?;
    if !output.status.success() {
        anyhow::bail!(
            "could not read repository config: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let listing = String::from_utf8_lossy(&output.stdout);
    if let Some(key) = listing
        .split('\0')
        .map(|entry| entry.trim_end_matches('\n'))
        .find(|key| is_dangerous_config_key(key))
    {
        anyhow::bail!(
            "refusing to run git: repository config defines '{key}', which can execute programs"
        );
    }
    Ok(())
}

fn commit_command(workspace: &Path, message: &str) -> Command {
    let mut command = git_command(workspace);
    command.args([
        "-c",
        "user.name=ORBIT Agent",
        "-c",
        "user.email=orbit@localhost",
        "commit",
        "--no-verify",
        "-m",
        message,
    ]);
    command
}

/// Push to an explicit URL. The workspace's `remote.*` config is agent-writable, so the
/// push target must never be taken from it (#301).
fn push_command(workspace: &Path, url: &str, branch: &str) -> Command {
    let mut command = git_command(workspace);
    command.args([
        "push",
        "--no-verify",
        url,
        &format!("HEAD:refs/heads/{branch}"),
    ]);
    command
}

/// The HTTPS URL of `repo` on the connection's host, the same one the clone uses.
fn repository_url(connection: &WorkerConnection, repo: &str) -> String {
    let clone_base = connection
        .connection
        .config
        .get("cloneBaseUrl")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            if connection.connector.slug == "gitlab" {
                "https://gitlab.com"
            } else {
                "https://github.com"
            }
        })
        .trim_end_matches('/');
    format!("{clone_base}/{repo}.git")
}

fn write_askpass() -> anyhow::Result<TempPath> {
    let mut file = tempfile::Builder::new()
        .prefix(".orbit-git-askpass-")
        .suffix(".sh")
        .tempfile()?;
    file.write_all(b"#!/bin/sh\ncase \"$1\" in\n  Username*) printf '%s' \"${ORBIT_GIT_USERNAME:-x-access-token}\" ;;\n  Password*) printf '%s' \"$ORBIT_GIT_TOKEN\" ;;\nesac\n")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = file.as_file().metadata()?.permissions();
        permissions.set_mode(0o700);
        file.as_file().set_permissions(permissions)?;
    }
    // Close the write handle before git execs the script. Linux returns ETXTBSY
    // ("text file busy") when execve() targets a file that is still open for writing.
    // TempPath keeps the path alive (and deletes it on drop) without an open handle.
    file.flush()?;
    Ok(file.into_temp_path())
}

async fn stage_workspace_changes(
    workspace: &Path,
    cancel: &CancellationToken,
) -> anyhow::Result<()> {
    ensure_safe_repo_config(workspace, cancel).await?;
    let mut command = git_command(workspace);
    command.args(["rev-parse", "--verify", "HEAD"]);
    let head = run_git(command, cancel, GIT_LOCAL_TIMEOUT, "rev-parse").await?;
    let unstage = if head.status.success() {
        let mut command = git_command(workspace);
        command
            .args(["reset", "--quiet", "HEAD", "--"])
            .args(ORBIT_GENERATED_PATHS);
        run_git(command, cancel, GIT_LOCAL_TIMEOUT, "reset").await?
    } else {
        let mut command = git_command(workspace);
        command
            .args(["rm", "--cached", "--ignore-unmatch", "-r", "--"])
            .args(ORBIT_GENERATED_PATHS);
        run_git(command, cancel, GIT_LOCAL_TIMEOUT, "rm").await?
    };
    if !unstage.status.success() {
        anyhow::bail!(
            "could not unstage ORBIT-generated files: {}",
            String::from_utf8_lossy(&unstage.stderr).trim()
        );
    }

    let mut command = git_command(workspace);
    command.args(["add", "-A", "--", "."]);
    for path in ORBIT_GENERATED_PATHS {
        command.arg(format!(":(exclude){path}"));
    }
    let add = run_git(command, cancel, GIT_LOCAL_TIMEOUT, "add").await?;
    if !add.status.success() {
        anyhow::bail!(
            "git add failed: {}",
            String::from_utf8_lossy(&add.stderr).trim()
        );
    }
    Ok(())
}

fn apply_git_auth(command: &mut Command, askpass: &Path, username: &str) {
    command
        .env("GIT_ASKPASS", askpass)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("ORBIT_GIT_USERNAME", username);
}

fn git_username(connection: &WorkerConnection) -> String {
    connection
        .connection
        .config
        .get("gitUsername")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| {
            if connection.connector.slug == "gitlab" {
                "oauth2".into()
            } else {
                "x-access-token".into()
            }
        })
}

fn input_string(input: &Value, key: &str) -> Option<String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            input
                .get("payload")
                .and_then(|payload| payload.get(key))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
}

fn repo_name(connection: &WorkerConnection, input: &Value) -> Option<String> {
    input_string(input, "repository").or_else(|| {
        connection
            .connection
            .config
            .get("defaultRepository")
            .or_else(|| connection.connection.config.get("defaultProject"))
            .and_then(Value::as_str)
            .map(str::to_string)
    })
}

fn validate_repo(connection: &WorkerConnection, repo: &str) -> anyhow::Result<()> {
    let ok = repo
        .split_once('/')
        .map(|(owner, name)| {
            !owner.is_empty()
                && !name.is_empty()
                && owner
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        })
        .unwrap_or(false);
    if !ok {
        anyhow::bail!("repository must be owner/repo");
    }
    if let Some(allowed) = connection
        .connection
        .config
        .get("allowedRepositories")
        .or_else(|| connection.connection.config.get("allowedProjects"))
        .and_then(Value::as_array)
    {
        if !allowed.is_empty()
            && !allowed
                .iter()
                .filter_map(Value::as_str)
                .any(|value| value.eq_ignore_ascii_case(repo))
        {
            anyhow::bail!("repository {repo} is not in the connection's allowedRepositories");
        }
    }
    Ok(())
}

/// Clone the GitHub repository into the workspace before the harness starts.
///
/// Single-repository compatibility path. Multi-repository teams should use
/// `checkout_workspace`, which reads the execution's repository list.
pub async fn checkout(
    client: &WorkerClient,
    execution_id: uuid::Uuid,
    connection_id: Option<uuid::Uuid>,
    input: &Value,
    workspace: &Path,
    cancel: &CancellationToken,
) -> anyhow::Result<Option<String>> {
    if cancel.is_cancelled() {
        return Err(GitAbort::Cancelled.into());
    }
    let Some(connection_id) = connection_id else {
        return Ok(None);
    };
    let connection = client.connection(execution_id, connection_id).await?;
    let Some(repo) = repo_name(&connection, input) else {
        return Ok(Some(
            "repository checkout skipped: no repository configured".into(),
        ));
    };
    checkout_repository(
        client,
        execution_id,
        connection_id,
        &repo,
        input_string(input, "branch").as_deref(),
        workspace,
        cancel,
    )
    .await
    .map(Some)
}

/// Check out the repository list from the execution payload sequentially.
///
/// A single repository preserves the current root layout for compatibility; multiple
/// repositories are cloned into their declared workspace subdirectories.
pub async fn checkout_workspace(
    client: &WorkerClient,
    execution_id: uuid::Uuid,
    default_connection_id: Option<uuid::Uuid>,
    input: &Value,
    workspace: &Path,
    cancel: &CancellationToken,
) -> anyhow::Result<Vec<String>> {
    if cancel.is_cancelled() {
        return Err(GitAbort::Cancelled.into());
    }
    let specs = repo_specs(input);
    if specs.is_empty() {
        if let Some(note) = checkout(
            client,
            execution_id,
            default_connection_id,
            input,
            workspace,
            cancel,
        )
        .await?
        {
            return Ok(vec![note]);
        }
        return Ok(Vec::new());
    }

    let single = specs.len() == 1;
    let mut notes = Vec::new();
    for spec in &specs {
        if cancel.is_cancelled() {
            return Err(GitAbort::Cancelled.into());
        }
        validate_repo_path(&spec.path)?;
        let target = repo_directory(workspace, spec, single);
        let connection_id = spec
            .connection_id
            .or(default_connection_id)
            .ok_or_else(|| anyhow::anyhow!("repository '{}' has no connection", spec.repository))?;
        notes.push(
            checkout_repository(
                client,
                execution_id,
                connection_id,
                &spec.repository,
                spec.branch.as_deref(),
                &target,
                cancel,
            )
            .await?,
        );
    }
    Ok(notes)
}

async fn checkout_repository(
    client: &WorkerClient,
    execution_id: uuid::Uuid,
    connection_id: uuid::Uuid,
    repo: &str,
    branch: Option<&str>,
    workspace: &Path,
    cancel: &CancellationToken,
) -> anyhow::Result<String> {
    if cancel.is_cancelled() {
        return Err(GitAbort::Cancelled.into());
    }
    if workspace.join(".git").exists() {
        return Ok(format!("repository {repo} already checked out"));
    }
    if !git_available() {
        anyhow::bail!("repo.checkout requires git in the worker image");
    }
    let connection = client.connection(execution_id, connection_id).await?;
    validate_repo(&connection, repo)?;
    let token = connection
        .credential
        .as_ref()
        .map(|secret| secret.value.clone());
    let askpass = match &token {
        Some(_) => Some(write_askpass()?),
        None => None,
    };
    let username = git_username(&connection);
    tokio::fs::create_dir_all(workspace).await?;
    let mut command = git_command(workspace);
    command.args(["clone", "--depth", "1"]);
    if let Some(branch) = branch.filter(|branch| !branch.is_empty()) {
        command.args(["--branch", branch]);
    }
    command.arg(repository_url(&connection, repo)).arg(".");
    if let Some(token) = &token {
        command.env("ORBIT_GIT_TOKEN", token);
    }
    if let Some(askpass) = &askpass {
        apply_git_auth(&mut command, askpass.as_ref(), &username);
    }
    let output = run_git(command, cancel, GIT_CLONE_TIMEOUT, "clone").await?;
    if !output.status.success() {
        anyhow::bail!(
            "git clone failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(format!("checked out {repo}"))
}

/// Run a git child process to completion, bounded by `timeout` and aborted when `cancel`
/// fires. The child is killed on every early return (`kill_on_drop`).
async fn run_git(
    mut command: Command,
    cancel: &CancellationToken,
    timeout: Duration,
    op: &'static str,
) -> anyhow::Result<Output> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let child = command.spawn()?;
    let output = child.wait_with_output();
    tokio::pin!(output);
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(GitAbort::Cancelled.into()),
        result = tokio::time::timeout(timeout, &mut output) => match result {
            Ok(result) => Ok(result?),
            Err(_) => Err(GitAbort::TimedOut { op, seconds: timeout.as_secs() }.into()),
        },
    }
}

/// Commit all workspace changes. Returns the commit subject/status.
///
/// A commit is local, so the connection token is not given to git here (#301).
pub async fn commit(
    workspace: &Path,
    message: &str,
    cancel: &CancellationToken,
) -> anyhow::Result<Value> {
    stage_workspace_changes(workspace, cancel).await?;
    let command = commit_command(workspace, message);
    let output = run_git(command, cancel, GIT_LOCAL_TIMEOUT, "commit").await?;
    if !output.status.success() {
        return failed_commit(&output.stdout, &output.stderr);
    }
    Ok(serde_json::json!({
        "status": "committed",
        "message": message,
        "output": String::from_utf8_lossy(&output.stdout).trim(),
    }))
}

/// The result of a `git commit` that exited non-zero. git reports "nothing to commit" on
/// stdout, not stderr (#307), which is a normal outcome for an agent, not an error.
fn failed_commit(stdout: &[u8], stderr: &[u8]) -> anyhow::Result<Value> {
    let stdout = String::from_utf8_lossy(stdout);
    let stderr = String::from_utf8_lossy(stderr);
    if stdout.contains("nothing to commit") || stderr.contains("nothing to commit") {
        return Ok(serde_json::json!({ "status": "no_changes" }));
    }
    let detail = if stderr.trim().is_empty() {
        stdout.trim()
    } else {
        stderr.trim()
    };
    anyhow::bail!("git commit failed: {detail}")
}

/// Push the current HEAD to a branch of the workspace's repository.
///
/// The target is built from the connection and the repository that was checked out
/// (`repository`, else the execution input or the connection default), validated against
/// the connection's allowlist, and passed to git explicitly. The workspace's own
/// `remote.*` and `url.*` config cannot redirect the push or the token (#301).
#[allow(clippy::too_many_arguments)]
pub async fn push(
    client: &WorkerClient,
    execution_id: uuid::Uuid,
    connection_id: uuid::Uuid,
    repository: Option<&str>,
    input: &Value,
    workspace: &Path,
    branch: &str,
    cancel: &CancellationToken,
) -> anyhow::Result<Value> {
    if branch.is_empty()
        || branch.len() > 200
        || branch
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, ':' | '~' | '^' | '?' | '*' | '[' | '\\'))
    {
        anyhow::bail!("invalid branch name");
    }
    let connection = client.connection(execution_id, connection_id).await?;
    let repo = repository
        .map(str::to_string)
        .or_else(|| repo_name(&connection, input))
        .ok_or_else(|| anyhow::anyhow!("no repository is configured for this workspace"))?;
    validate_repo(&connection, &repo)?;
    let username = git_username(&connection);
    let token = connection
        .credential
        .as_ref()
        .map(|secret| secret.value.clone())
        .ok_or_else(|| anyhow::anyhow!("connection has no token"))?;
    ensure_safe_repo_config(workspace, cancel).await?;
    let askpass = write_askpass()?;
    let mut command = push_command(workspace, &repository_url(&connection, &repo), branch);
    command.env("ORBIT_GIT_TOKEN", &token);
    apply_git_auth(&mut command, askpass.as_ref(), &username);
    let output = run_git(command, cancel, GIT_PUSH_TIMEOUT, "push").await?;
    if !output.status.success() {
        anyhow::bail!(
            "git push failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(serde_json::json!({ "status": "pushed", "repository": repo, "branch": branch }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as StdCommand;

    fn git(workspace: &Path, args: &[&str]) -> std::process::Output {
        let output = StdCommand::new("git")
            .current_dir(workspace)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    #[cfg(unix)]
    fn init_repo(name: &str) -> std::path::PathBuf {
        let workspace =
            std::env::temp_dir().join(format!("orbit-git-{name}-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&workspace).unwrap();
        git(&workspace, &["init", "--quiet"]);
        workspace
    }

    #[cfg(unix)]
    fn plant_script(path: &Path, marker: &Path) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(path, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn git_command_env_is_allowlisted_and_hardened() {
        let command = git_command(Path::new("."));
        let std_command = command.as_std();
        for (name, _) in std_command.get_envs() {
            let name = name.to_str().unwrap();
            assert!(
                is_git_inherited_name(name)
                    || matches!(
                        name,
                        "GIT_CONFIG_NOSYSTEM"
                            | "GIT_CONFIG_GLOBAL"
                            | "GIT_ATTR_NOSYSTEM"
                            | "GIT_TERMINAL_PROMPT"
                    ),
                "unexpected env var {name}"
            );
        }
        let args: Vec<_> = std_command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(args.contains(&"core.hooksPath=/dev/null".to_string()));
        assert!(args.contains(&"core.fsmonitor=false".to_string()));
        assert!(!is_git_inherited_name("ORBIT_WORKER_TOKEN"));
        assert!(!is_git_inherited_name("ORBIT_API_URL"));
        assert!(!is_git_inherited_name("ORBIT_GIT_TOKEN"));
        // The custom CA bundle must reach git (#300); turning verification off must not.
        assert!(is_git_inherited_name("GIT_SSL_CAINFO"));
        assert!(is_git_inherited_name("GIT_SSL_CAPATH"));
        assert!(!is_git_inherited_name("GIT_SSL_NO_VERIFY"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn git_child_does_not_inherit_orbit_secrets() {
        // SAFETY: test-only; the variables are unique to this test.
        unsafe {
            std::env::set_var("ORBIT_WORKER_TOKEN", "worker-secret");
            std::env::set_var("ORBIT_API_URL", "http://api.invalid");
            std::env::set_var("GIT_SSL_CAINFO", "/etc/ssl/certs/ca-bundle.crt");
        }
        let workspace = init_repo("env");
        let output = git_command(&workspace)
            .args(["-c", "alias.dumpenv=!env", "dumpenv"])
            .output()
            .await
            .unwrap();
        assert!(output.status.success());
        let env = String::from_utf8_lossy(&output.stdout);
        assert!(env.contains("PATH="));
        assert!(
            env.contains("GIT_SSL_CAINFO=/etc/ssl/certs/ca-bundle.crt"),
            "custom CA bundle not passed to git: {env}"
        );
        assert!(!env.contains("ORBIT_"), "leaked: {env}");
        assert!(!env.contains("worker-secret"));
        let _ = std::fs::remove_dir_all(workspace);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn planted_pre_commit_hook_does_not_run_and_commit_succeeds() {
        let workspace = init_repo("hook");
        let marker = workspace.join("hook-ran");
        plant_script(&workspace.join(".git/hooks/pre-commit"), &marker);
        plant_script(&workspace.join(".git/hooks/commit-msg"), &marker);
        std::fs::write(workspace.join("a.txt"), "hello\n").unwrap();
        stage_workspace_changes(&workspace, &CancellationToken::new())
            .await
            .unwrap();
        let output = commit_command(&workspace, "test commit")
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!marker.exists(), "hook executed");
        let log = git(&workspace, &["log", "--format=%s"]);
        assert_eq!(String::from_utf8_lossy(&log.stdout).trim(), "test commit");
        let _ = std::fs::remove_dir_all(workspace);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn planted_fsmonitor_does_not_run() {
        let workspace = init_repo("fsmonitor");
        let marker = workspace.join("fsmonitor-ran");
        let script = workspace.join("fsmonitor.sh");
        plant_script(&script, &marker);
        git(
            &workspace,
            &["config", "core.fsmonitor", &script.display().to_string()],
        );
        std::fs::write(workspace.join("a.txt"), "hello\n").unwrap();
        stage_workspace_changes(&workspace, &CancellationToken::new())
            .await
            .unwrap();
        let output = commit_command(&workspace, "fsmonitor")
            .output()
            .await
            .unwrap();
        assert!(output.status.success());
        assert!(!marker.exists(), "fsmonitor executed");
        let _ = std::fs::remove_dir_all(workspace);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn planted_clean_filter_is_refused_and_does_not_run() {
        let workspace = init_repo("filter");
        let marker = workspace.join("filter-ran");
        let script = workspace.join("filter.sh");
        plant_script(&script, &marker);
        git(
            &workspace,
            &["config", "filter.evil.clean", &script.display().to_string()],
        );
        std::fs::write(workspace.join(".gitattributes"), "* filter=evil\n").unwrap();
        std::fs::write(workspace.join("a.txt"), "hello\n").unwrap();
        let err = stage_workspace_changes(&workspace, &CancellationToken::new())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("filter.evil.clean"), "{err}");
        assert!(!marker.exists(), "filter executed");
        assert!(
            ensure_safe_repo_config(&workspace, &CancellationToken::new())
                .await
                .is_err()
        );
        let _ = std::fs::remove_dir_all(workspace);
    }

    /// A local HTTP listener that answers every request with a Basic 401 and records
    /// whether the request carried an `Authorization` header.
    #[cfg(unix)]
    async fn recording_git_host() -> (String, std::sync::Arc<std::sync::Mutex<Vec<bool>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let record = seen.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let record = record.clone();
                tokio::spawn(async move {
                    let mut buffer = vec![0u8; 8192];
                    let read = socket.read(&mut buffer).await.unwrap_or(0);
                    let request = String::from_utf8_lossy(&buffer[..read]).to_ascii_lowercase();
                    record
                        .lock()
                        .unwrap()
                        .push(request.contains("\r\nauthorization:"));
                    let _ = socket
                        .write_all(
                            b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"x\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        )
                        .await;
                });
            }
        });
        (format!("http://{addr}"), seen)
    }

    /// A fake worker API that serves one connection with a dummy token.
    #[cfg(unix)]
    async fn fake_connection_api(clone_base: &str) -> (WorkerClient, uuid::Uuid) {
        use axum::routing::get;
        let connection_id = uuid::Uuid::now_v7();
        let now = chrono::Utc::now();
        let body = serde_json::json!({
            "connection": orbit_store::Connection {
                id: connection_id,
                org_id: uuid::Uuid::now_v7(),
                owner_user_id: None,
                owner_team_id: None,
                visibility: "org".into(),
                connector_id: uuid::Uuid::now_v7(),
                name: "github".into(),
                status: "active".into(),
                credential_ref: None,
                config: serde_json::json!({
                    "cloneBaseUrl": clone_base,
                    "allowedRepositories": ["acme/allowed"],
                }),
                scopes: vec![],
                created_by: None,
                last_checked_at: None,
                created_at: now,
                updated_at: now,
            },
            "connector": orbit_store::ConnectorRecord {
                id: uuid::Uuid::now_v7(),
                org_id: None,
                slug: "github".into(),
                name: "GitHub".into(),
                version: "1".into(),
                description: String::new(),
                auth: serde_json::json!({}),
                capabilities: serde_json::json!([]),
                config_schema: serde_json::json!({}),
                webhooks: false,
                builtin: true,
                docs_url: None,
                created_at: now,
            },
            "credential": { "secretRef": "test/token", "value": "DUMMY-NOT-A-REAL-TOKEN" },
        });
        let app = axum::Router::new().route(
            "/api/v1/worker/connections/{id}",
            get(move || {
                let body = body.clone();
                async move { axum::Json(body) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (
            WorkerClient::new(format!("http://{addr}"), "worker-token").unwrap(),
            connection_id,
        )
    }

    #[cfg(unix)]
    fn committed_repo(name: &str) -> std::path::PathBuf {
        let workspace = init_repo(name);
        git(
            &workspace,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                "x",
            ],
        );
        workspace
    }

    /// #301: the workspace's `origin` is agent-writable; the push (and the token) must go
    /// to the connection's host and repository only.
    #[cfg(unix)]
    #[tokio::test]
    async fn push_ignores_a_rewritten_origin() {
        let (legit, legit_seen) = recording_git_host().await;
        let (attacker, attacker_seen) = recording_git_host().await;
        let (client, connection_id) = fake_connection_api(&legit).await;
        let workspace = committed_repo("push-origin");
        git(
            &workspace,
            &["remote", "add", "origin", &format!("{attacker}/other.git")],
        );

        let result = push(
            &client,
            uuid::Uuid::now_v7(),
            connection_id,
            Some("acme/allowed"),
            &serde_json::json!({}),
            &workspace,
            "orbit/test",
            &CancellationToken::new(),
        )
        .await;
        assert!(result.is_err(), "the fake host refuses the push");
        assert!(
            attacker_seen.lock().unwrap().is_empty(),
            "the rewritten origin must not be contacted"
        );
        assert!(
            !legit_seen.lock().unwrap().is_empty(),
            "the connection's host is the push target"
        );
        let _ = std::fs::remove_dir_all(workspace);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn push_refuses_url_rewrites_and_foreign_repositories() {
        let (legit, legit_seen) = recording_git_host().await;
        let (attacker, attacker_seen) = recording_git_host().await;
        let (client, connection_id) = fake_connection_api(&legit).await;

        // An insteadOf rule would rewrite even the explicit URL: refuse before any request.
        let workspace = committed_repo("push-insteadof");
        git(
            &workspace,
            &[
                "config",
                &format!("url.{attacker}/.insteadOf"),
                &format!("{legit}/"),
            ],
        );
        let error = push(
            &client,
            uuid::Uuid::now_v7(),
            connection_id,
            Some("acme/allowed"),
            &serde_json::json!({}),
            &workspace,
            "orbit/test",
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("refusing to run git"), "{error}");
        let _ = std::fs::remove_dir_all(&workspace);

        // A repository outside the connection's allowlist is refused too.
        let workspace = committed_repo("push-allowlist");
        let error = push(
            &client,
            uuid::Uuid::now_v7(),
            connection_id,
            Some("acme/other"),
            &serde_json::json!({}),
            &workspace,
            "orbit/test",
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("allowedRepositories"), "{error}");
        assert!(attacker_seen.lock().unwrap().is_empty());
        assert!(legit_seen.lock().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(workspace);
    }

    #[test]
    fn commit_does_not_receive_the_connection_token() {
        let command = commit_command(Path::new("."), "message");
        for (name, _) in command.as_std().get_envs() {
            assert_ne!(name, "ORBIT_GIT_TOKEN");
            assert_ne!(name, "GIT_ASKPASS");
        }
    }

    #[test]
    fn repo_directory_is_the_same_for_checkout_and_tools() {
        let workspace = Path::new("/work");
        let spec = |repository: &str, path: &str| RepoSpec {
            connection_id: None,
            repository: repository.into(),
            path: path.into(),
            branch: None,
        };
        // #307: a primary repository with no path in a multi-repository workspace lives in
        // its own directory, where checkout put it, not at the workspace root.
        assert_eq!(
            repo_directory(workspace, &spec("acme/app", ""), false),
            Path::new("/work/app")
        );
        assert_eq!(
            repo_directory(workspace, &spec("acme/app", ""), true),
            workspace
        );
        assert_eq!(
            repo_directory(workspace, &spec("acme/app", "src/app"), false),
            Path::new("/work/src/app")
        );
    }

    #[cfg(unix)]
    #[test]
    fn commit_with_nothing_staged_reports_no_changes() {
        // Run the real command on a clean repository and feed its output to the parser.
        let workspace = init_repo("nochanges");
        git(
            &workspace,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                "x",
            ],
        );
        let output = StdCommand::new("git")
            .current_dir(&workspace)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "--no-verify",
                "-m",
                "nothing",
            ])
            .output()
            .unwrap();
        assert!(!output.status.success());
        let value = failed_commit(&output.stdout, &output.stderr).unwrap();
        assert_eq!(value["status"], "no_changes", "{value}");
        let error = failed_commit(b"", b"fatal: something else").unwrap_err();
        assert!(error.to_string().contains("something else"));
        let _ = std::fs::remove_dir_all(workspace);
    }

    #[test]
    fn dangerous_config_keys_are_detected() {
        for key in [
            "filter.x.smudge",
            "diff.x.textconv",
            "merge.x.driver",
            "include.path",
            "includeIf.gitdir:/x.path",
            "credential.helper",
            "core.pager",
            "gpg.program",
            // #301: these can redirect or intercept the credentialed push.
            "url.https://evil.example/.insteadOf",
            "url.https://evil.example/.pushInsteadOf",
            "http.proxy",
            "http.sslVerify",
            "http.https://github.com/.extraHeader",
        ] {
            assert!(is_dangerous_config_key(key), "{key}");
        }
        for key in [
            "user.name",
            "remote.origin.url",
            "core.bare",
            "branch.main.remote",
        ] {
            assert!(!is_dangerous_config_key(key), "{key}");
        }
    }

    #[tokio::test]
    async fn staging_omits_orbit_generated_files_and_keeps_source_changes() {
        let workspace =
            std::env::temp_dir().join(format!("orbit-git-stage-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&workspace).unwrap();
        git(&workspace, &["init"]);
        std::fs::write(workspace.join("AGENTS.md"), "repository guidance\n").unwrap();
        std::fs::write(workspace.join("README.md"), "baseline\n").unwrap();
        git(&workspace, &["add", "-A"]);
        git(
            &workspace,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "--quiet",
                "-m",
                "baseline",
            ],
        );

        std::fs::write(workspace.join("AGENTS.md"), "runtime instructions\n").unwrap();
        std::fs::write(workspace.join("opencode.json"), "provider config\n").unwrap();
        for path in [
            ".opencode/agents/reviewer.md",
            ".opencode/plugins/orbit-policy.ts",
            ".opencode/skills/review/SKILL.md",
            ".pi/mcp.json",
            ".pi/extensions/orbit-policy.ts",
            ".pi/skills/review/SKILL.md",
            ".pi-sessions/session.json",
        ] {
            let path = workspace.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "runtime generated secret\n").unwrap();
        }
        let source = workspace.join("src/change.rs");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, "pub fn change() {}\n").unwrap();
        git(&workspace, &["add", "-A"]);

        stage_workspace_changes(&workspace, &CancellationToken::new())
            .await
            .unwrap();

        let staged = git(&workspace, &["diff", "--cached", "--name-only"]);
        let paths = String::from_utf8(staged.stdout).unwrap();
        assert_eq!(paths.lines().collect::<Vec<_>>(), ["src/change.rs"]);
        let _ = std::fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn staging_omits_orbit_generated_files_in_an_unborn_repository() {
        let workspace =
            std::env::temp_dir().join(format!("orbit-git-stage-unborn-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&workspace).unwrap();
        git(&workspace, &["init"]);

        std::fs::write(workspace.join("AGENTS.md"), "runtime instructions\n").unwrap();
        std::fs::write(workspace.join("opencode.json"), "provider config\n").unwrap();
        let source = workspace.join("src/change.rs");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, "pub fn change() {}\n").unwrap();
        git(&workspace, &["add", "-A"]);

        stage_workspace_changes(&workspace, &CancellationToken::new())
            .await
            .unwrap();

        let staged = git(&workspace, &["diff", "--cached", "--name-only"]);
        let paths = String::from_utf8(staged.stdout).unwrap();
        assert_eq!(paths.lines().collect::<Vec<_>>(), ["src/change.rs"]);
        assert!(workspace.join("opencode.json").exists());
        let _ = std::fs::remove_dir_all(workspace);
    }
}

#[cfg(test)]
mod cleanup_tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn askpass_can_be_executed_while_the_guard_is_held() {
        let askpass = write_askpass().unwrap();
        let askpass_path: &std::path::Path = askpass.as_ref();
        let output = std::process::Command::new(askpass_path)
            .arg("Password for 'https://git.example.com':")
            .env("ORBIT_GIT_TOKEN", "test-token")
            .output()
            .expect("askpass executes");
        assert!(
            output.status.success(),
            "askpass failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout), "test-token");
    }

    #[test]
    fn askpass_file_is_removed_when_guard_drops() {
        let askpass = write_askpass().unwrap();
        let path = askpass.to_path_buf();
        assert!(path.is_file());
        drop(askpass);
        assert!(!path.exists());
    }

    #[cfg(windows)]
    fn long_running_command() -> Command {
        let mut command = Command::new("ping.exe");
        command.args(["-n", "20", "127.0.0.1"]);
        command
    }

    #[cfg(not(windows))]
    fn long_running_command() -> Command {
        let mut command = Command::new("sleep");
        command.arg("30");
        command
    }

    #[tokio::test]
    async fn git_timeout_is_typed_and_prompt() {
        let started = std::time::Instant::now();
        let error = run_git(
            long_running_command(),
            &CancellationToken::new(),
            Duration::from_millis(300),
            "clone",
        )
        .await
        .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(
            error.downcast_ref::<GitAbort>(),
            Some(&GitAbort::TimedOut {
                op: "clone",
                seconds: 0
            })
        );
    }

    #[tokio::test]
    async fn git_cancellation_is_typed_and_prompt() {
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move {
            run_git(
                long_running_command(),
                &task_cancel,
                Duration::from_secs(30),
                "push",
            )
            .await
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        let started = std::time::Instant::now();
        cancel.cancel();
        let error = task.await.unwrap().unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(error.downcast_ref::<GitAbort>(), Some(&GitAbort::Cancelled));
    }

    #[tokio::test]
    async fn commit_and_stage_abort_when_already_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();
        let error = stage_workspace_changes(dir.path(), &cancel)
            .await
            .unwrap_err();
        assert_eq!(error.downcast_ref::<GitAbort>(), Some(&GitAbort::Cancelled));
    }
}
