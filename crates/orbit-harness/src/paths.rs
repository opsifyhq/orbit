//! Confinement of agent-author-supplied directory options (`workspaceSubdir`,
//! `sessionDir`) to the execution workspace.
//!
//! On shared workers several executions live side by side under the workspace root, so a
//! config value must never be able to point an adapter at another execution's directory
//! or anywhere else on the host.

use std::path::{Component, Path, PathBuf};

use orbit_contracts::{HarnessError, HarnessErrorKind};

/// Static validation: the value must be a plain relative path with only normal
/// components (no `/` prefix, drive/UNC prefix, `.` tricks or `..`).
pub fn validate_relative_subpath(value: &str) -> Result<PathBuf, String> {
    let trimmed = value.trim();
    if trimmed.contains('\0') {
        return Err("must not contain NUL bytes".into());
    }
    let path = Path::new(trimmed);
    if trimmed.starts_with('/') || trimmed.starts_with('\\') || path.is_absolute() {
        return Err("must be a relative path inside the execution workspace".into());
    }
    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                if part.to_string_lossy().contains('\\') {
                    return Err("must not contain backslashes".into());
                }
                clean.push(part);
            }
            Component::CurDir => {}
            Component::ParentDir => return Err("must not contain '..' components".into()),
            Component::RootDir | Component::Prefix(_) => {
                return Err("must be a relative path inside the execution workspace".into())
            }
        }
    }
    Ok(clean)
}

fn config_error(message: impl Into<String>) -> HarnessError {
    HarnessError::new(HarnessErrorKind::InvalidConfig, message)
}

/// Resolve `rel` under `root` and create it, asserting at every step that the real
/// (symlink-resolved) path stays inside the canonical `root`. Returns the canonical
/// directory. Empty `rel` resolves to the root itself.
pub async fn resolve_confined_dir(root: &Path, rel: &str) -> Result<PathBuf, HarnessError> {
    let clean = validate_relative_subpath(rel).map_err(config_error)?;
    tokio::fs::create_dir_all(root).await.map_err(|e| {
        HarnessError::new(
            HarnessErrorKind::Internal,
            format!("could not create workspace {}: {e}", root.display()),
        )
    })?;
    let canonical_root = tokio::fs::canonicalize(root).await.map_err(|e| {
        HarnessError::new(
            HarnessErrorKind::Internal,
            format!("could not resolve workspace {}: {e}", root.display()),
        )
    })?;
    let mut current = canonical_root.clone();
    for component in clean.components() {
        let next = current.join(component);
        match tokio::fs::symlink_metadata(&next).await {
            Ok(_) => {
                // Existing entry (possibly a symlink): follow it and re-check.
                let resolved = tokio::fs::canonicalize(&next).await.map_err(|e| {
                    config_error(format!("could not resolve {}: {e}", next.display()))
                })?;
                if !resolved.starts_with(&canonical_root) {
                    return Err(config_error(
                        "path escapes the execution workspace (symlink)",
                    ));
                }
                current = resolved;
            }
            Err(_) => {
                tokio::fs::create_dir(&next).await.map_err(|e| {
                    HarnessError::new(
                        HarnessErrorKind::Internal,
                        format!("could not create {}: {e}", next.display()),
                    )
                })?;
                current = next;
            }
        }
    }
    if !current.starts_with(&canonical_root) {
        return Err(config_error("path escapes the execution workspace"));
    }
    Ok(current)
}

fn refuse_symlink(path: &Path) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        format!(
            "refusing to follow symlink {} inside the execution workspace",
            path.display()
        ),
    )
}

/// Create the parent directories of `rel` under `workspace`, refusing to traverse a
/// symlink, and return the full path of `rel`. The workspace root itself is trusted
/// (the worker creates it).
fn create_parents_no_follow(workspace: &Path, rel: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(workspace)?;
    let mut current = workspace.to_path_buf();
    let mut components = rel.components().peekable();
    while let Some(component) = components.next() {
        let Component::Normal(part) = component else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "workspace file path must be relative and normal",
            ));
        };
        current.push(part);
        if components.peek().is_none() {
            break;
        }
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => return Err(refuse_symlink(&current)),
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    format!("{} exists and is not a directory", current.display()),
                ))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => std::fs::create_dir(&current)?,
            Err(e) => return Err(e),
        }
    }
    Ok(current)
}

/// Write a worker-owned file at `rel` inside `workspace` without ever following a
/// symlink that the (untrusted) cloned repository may have shipped.
///
/// A symlinked parent directory is refused with an error. A symlink at the file path
/// itself is replaced: the link is unlinked, never written through.
pub fn write_workspace_file(
    workspace: &Path,
    rel: impl AsRef<Path>,
    contents: impl AsRef<[u8]>,
) -> std::io::Result<()> {
    write_file(workspace, rel.as_ref(), contents.as_ref(), false)
}

/// [`write_workspace_file`] for a file the harness must be able to run (skill scripts).
pub fn write_workspace_executable(
    workspace: &Path,
    rel: impl AsRef<Path>,
    contents: impl AsRef<[u8]>,
) -> std::io::Result<()> {
    write_file(workspace, rel.as_ref(), contents.as_ref(), true)
}

fn write_file(
    workspace: &Path,
    rel: &Path,
    contents: &[u8],
    executable: bool,
) -> std::io::Result<()> {
    use std::io::Write;
    let path = create_parents_no_follow(workspace, rel)?;
    if let Ok(meta) = std::fs::symlink_metadata(&path) {
        if meta.file_type().is_symlink() {
            std::fs::remove_file(&path)?;
        }
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Closes the check/use race: a link planted meanwhile makes open() fail.
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(&path)?;
    file.write_all(contents)?;
    #[cfg(unix)]
    if executable {
        use std::os::unix::fs::PermissionsExt;
        // On the open handle, so the mode lands on the file just written and nothing else.
        file.set_permissions(std::fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(not(unix))]
    let _ = executable;
    Ok(())
}

/// Create a directory chain under `workspace`, refusing symlinked components.
pub fn create_workspace_dir(workspace: &Path, rel: impl AsRef<Path>) -> std::io::Result<()> {
    create_parents_no_follow(workspace, &rel.as_ref().join("_")).map(|_| ())
}

/// Read a workspace file that the repository controls (for example an existing
/// `AGENTS.md`). Returns `None` when it is missing, unreadable, not a regular file, or a
/// symlink whose target is outside the workspace; a symlink that stays inside the
/// workspace (e.g. `AGENTS.md -> CLAUDE.md`) is followed.
pub fn read_workspace_file(workspace: &Path, rel: impl AsRef<Path>) -> Option<String> {
    let path = workspace.join(rel);
    let meta = std::fs::symlink_metadata(&path).ok()?;
    if meta.file_type().is_symlink() {
        let root = std::fs::canonicalize(workspace).ok()?;
        let target = std::fs::canonicalize(&path).ok()?;
        if !target.starts_with(root) {
            return None;
        }
    }
    if !std::fs::metadata(&path).ok()?.is_file() {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    mod symlinks {
        use super::super::*;
        use std::os::unix::fs::symlink;

        #[test]
        fn write_replaces_a_file_symlink_instead_of_writing_through_it() {
            let outside = tempfile::tempdir().unwrap();
            let victim = outside.path().join("victim");
            std::fs::write(&victim, "keep").unwrap();
            let ws = tempfile::tempdir().unwrap();
            symlink(&victim, ws.path().join("AGENTS.md")).unwrap();

            write_workspace_file(ws.path(), "AGENTS.md", "new").unwrap();

            assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep");
            let meta = std::fs::symlink_metadata(ws.path().join("AGENTS.md")).unwrap();
            assert!(!meta.file_type().is_symlink());
            assert_eq!(
                std::fs::read_to_string(ws.path().join("AGENTS.md")).unwrap(),
                "new"
            );
        }

        #[test]
        fn write_refuses_a_symlinked_parent_directory() {
            let outside = tempfile::tempdir().unwrap();
            let ws = tempfile::tempdir().unwrap();
            symlink(outside.path(), ws.path().join(".opencode")).unwrap();

            let error = write_workspace_file(ws.path(), ".opencode/plugins/orbit-policy.ts", "x")
                .unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
            assert!(create_workspace_dir(ws.path(), ".opencode/skills").is_err());
            assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
        }

        #[test]
        fn write_creates_missing_parents_and_overwrites_regular_files() {
            let ws = tempfile::tempdir().unwrap();
            write_workspace_file(ws.path(), ".pi/extensions/a.ts", "one").unwrap();
            write_workspace_file(ws.path(), ".pi/extensions/a.ts", "two").unwrap();
            assert_eq!(
                std::fs::read_to_string(ws.path().join(".pi/extensions/a.ts")).unwrap(),
                "two"
            );
        }

        #[test]
        fn read_ignores_links_that_leave_the_workspace_but_follows_inner_ones() {
            let outside = tempfile::tempdir().unwrap();
            std::fs::write(outside.path().join("secret"), "top secret").unwrap();
            let ws = tempfile::tempdir().unwrap();
            symlink(outside.path().join("secret"), ws.path().join("AGENTS.md")).unwrap();
            assert_eq!(read_workspace_file(ws.path(), "AGENTS.md"), None);

            std::fs::write(ws.path().join("CLAUDE.md"), "repo guidance").unwrap();
            std::fs::remove_file(ws.path().join("AGENTS.md")).unwrap();
            symlink("CLAUDE.md", ws.path().join("AGENTS.md")).unwrap();
            assert_eq!(
                read_workspace_file(ws.path(), "AGENTS.md").as_deref(),
                Some("repo guidance")
            );
        }
    }

    use super::*;

    #[test]
    fn rejects_absolute_and_parent_paths() {
        for bad in [
            "/etc",
            "/",
            "\\windows",
            "../other-execution",
            "a/../../b",
            "a/..",
            "..",
            "ok/../..",
            "a\0b",
        ] {
            assert!(validate_relative_subpath(bad).is_err(), "{bad:?} accepted");
        }
    }

    #[test]
    fn accepts_plain_relative_paths() {
        assert_eq!(
            validate_relative_subpath("repo/src").unwrap(),
            PathBuf::from("repo/src")
        );
        assert_eq!(
            validate_relative_subpath("./repo").unwrap(),
            PathBuf::from("repo")
        );
        assert_eq!(validate_relative_subpath("").unwrap(), PathBuf::new());
    }

    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("orbit-paths-{name}-{}", uuid::Uuid::now_v7()))
    }

    #[tokio::test]
    async fn resolves_inside_root() {
        let root = scratch("ok");
        let dir = resolve_confined_dir(&root, "a/b").await.unwrap();
        assert!(dir.starts_with(tokio::fs::canonicalize(&root).await.unwrap()));
        assert!(dir.is_dir());
        let _ = tokio::fs::remove_dir_all(&root).await;
    }

    #[tokio::test]
    async fn runtime_rejects_traversal_and_absolute() {
        let root = scratch("bad");
        assert!(resolve_confined_dir(&root, "../x").await.is_err());
        assert!(resolve_confined_dir(&root, "/tmp").await.is_err());
        let _ = tokio::fs::remove_dir_all(&root).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runtime_rejects_symlink_escape() {
        let root = scratch("link");
        let outside = scratch("outside");
        tokio::fs::create_dir_all(&root).await.unwrap();
        tokio::fs::create_dir_all(&outside).await.unwrap();
        std::os::unix::fs::symlink(&outside, root.join("escape")).unwrap();
        let err = resolve_confined_dir(&root, "escape/inner")
            .await
            .unwrap_err();
        assert!(err.message.contains("escapes"));
        assert!(!outside.join("inner").exists());
        let _ = tokio::fs::remove_dir_all(&root).await;
        let _ = tokio::fs::remove_dir_all(&outside).await;
    }
}
