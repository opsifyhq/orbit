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

fn invalid(message: impl Into<String>) -> HarnessError {
    HarnessError::new(HarnessErrorKind::InvalidConfig, message)
}

fn internal(message: impl Into<String>) -> HarnessError {
    HarnessError::new(HarnessErrorKind::Internal, message)
}

/// Canonicalize `root`, creating it if needed.
pub async fn canonical_root(root: &Path) -> Result<PathBuf, HarnessError> {
    tokio::fs::create_dir_all(root).await.map_err(|e| {
        internal(format!(
            "could not create workspace {}: {e}",
            root.display()
        ))
    })?;
    tokio::fs::canonicalize(root).await.map_err(|e| {
        internal(format!(
            "could not resolve workspace {}: {e}",
            root.display()
        ))
    })
}

/// Ensure every existing path component from `root` down to `path` stays within `root`
/// and is not a symlink. Missing components are allowed (they will be created later).
/// `path` must be inside `root`.
async fn ensure_existing_components_safe(root: &Path, path: &Path) -> Result<(), HarnessError> {
    let relative = path.strip_prefix(root).map_err(|_| {
        invalid(format!(
            "path {} escapes the execution workspace",
            path.display()
        ))
    })?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        match tokio::fs::symlink_metadata(&current).await {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    return Err(invalid(format!(
                        "refusing to follow symlinked path {} inside the workspace",
                        current.display()
                    )));
                }
                let resolved = tokio::fs::canonicalize(&current).await.map_err(|e| {
                    internal(format!("could not resolve {}: {e}", current.display()))
                })?;
                if !resolved.starts_with(root) {
                    return Err(invalid(format!(
                        "path {} escapes the execution workspace",
                        current.display()
                    )));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // Remaining components don't exist yet.
                return Ok(());
            }
            Err(error) => {
                return Err(internal(format!(
                    "could not inspect {}: {error}",
                    current.display()
                )));
            }
        }
    }
    Ok(())
}

/// Read a UTF-8 file under `root` if it exists and is a regular file.
/// Returns `Ok(None)` when missing or when the path is a symlink (never followed).
pub async fn safe_read_to_string_if_regular(
    root: &Path,
    path: &Path,
) -> Result<Option<String>, HarnessError> {
    if !path.starts_with(root) {
        return Err(invalid(format!(
            "path {} escapes the execution workspace",
            path.display()
        )));
    }
    match tokio::fs::symlink_metadata(path).await {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                return Ok(None);
            }
            if !meta.is_file() {
                return Ok(None);
            }
            let resolved = tokio::fs::canonicalize(path)
                .await
                .map_err(|e| internal(format!("could not resolve {}: {e}", path.display())))?;
            if !resolved.starts_with(root) {
                return Err(invalid(format!(
                    "path {} escapes the execution workspace",
                    path.display()
                )));
            }
            let content = tokio::fs::read_to_string(path)
                .await
                .map_err(|e| internal(format!("could not read {}: {e}", path.display())))?;
            Ok(Some(content))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(internal(format!(
            "could not inspect {}: {error}",
            path.display()
        ))),
    }
}

/// Create parent directories and write `contents` to `path` inside `root`, refusing
/// to follow symlinks or escape the workspace.
pub async fn safe_write(
    root: &Path,
    path: &Path,
    contents: impl AsRef<[u8]>,
) -> Result<(), HarnessError> {
    if !path.starts_with(root) {
        return Err(invalid(format!(
            "path {} escapes the execution workspace",
            path.display()
        )));
    }
    let Some(parent) = path.parent() else {
        return Err(internal(format!("path {} has no parent", path.display())));
    };
    ensure_existing_components_safe(root, parent).await?;
    tokio::fs::create_dir_all(parent).await.map_err(|e| {
        internal(format!(
            "could not create directory {}: {e}",
            parent.display()
        ))
    })?;
    // Re-check after creation in case of races/symlink swaps on existing components.
    ensure_existing_components_safe(root, parent).await?;
    if let Ok(meta) = tokio::fs::symlink_metadata(path).await {
        if meta.file_type().is_symlink() {
            return Err(invalid(format!(
                "refusing to write through symlink {} inside the workspace",
                path.display()
            )));
        }
    }
    tokio::fs::write(path, contents)
        .await
        .map_err(|e| internal(format!("could not write {}: {e}", path.display())))?;
    Ok(())
}

/// Create `dir` inside `root`, refusing to follow symlinks or escape.
pub async fn safe_create_dir_all(root: &Path, dir: &Path) -> Result<(), HarnessError> {
    if !dir.starts_with(root) {
        return Err(invalid(format!(
            "path {} escapes the execution workspace",
            dir.display()
        )));
    }
    ensure_existing_components_safe(root, dir).await?;
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| internal(format!("could not create directory {}: {e}", dir.display())))?;
    ensure_existing_components_safe(root, dir).await?;
    Ok(())
}

/// Set executable permission on a regular file inside `root`.
#[cfg(unix)]
pub async fn safe_set_executable(root: &Path, path: &Path) -> Result<(), HarnessError> {
    if !path.starts_with(root) {
        return Err(invalid(format!(
            "path {} escapes the execution workspace",
            path.display()
        )));
    }
    match tokio::fs::symlink_metadata(path).await {
        Ok(meta) if meta.file_type().is_symlink() => Err(invalid(format!(
            "refusing to chmod symlink {} inside the workspace",
            path.display()
        ))),
        Ok(_) => {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = tokio::fs::metadata(path)
                .await
                .map_err(|e| internal(format!("could not stat {}: {e}", path.display())))?
                .permissions();
            perms.set_mode(0o755);
            tokio::fs::set_permissions(path, perms)
                .await
                .map_err(|e| internal(format!("could not chmod {}: {e}", path.display())))?;
            Ok(())
        }
        Err(error) => Err(internal(format!(
            "could not inspect {}: {error}",
            path.display()
        ))),
    }
}
#[cfg(test)]
mod tests {
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
