//! Materialize organization skills into an execution workspace.
//!
//! Pi discovers project skills under `.pi/skills`; the same files are also written to
//! `.opencode/skills` for OpenCode-style workspaces. Files are validated by the API
//! (relative paths, string content) and re-checked here.

use std::path::Path;

use anyhow::Context;
use orbit_domain::AgentSpec;
use orbit_store::Skill;

pub async fn materialize(
    spec: &AgentSpec,
    execution_skill_slugs: &[String],
    profile_skill_slugs: &[String],
    skills: &[Skill],
    workspace: &Path,
) -> anyhow::Result<Vec<String>> {
    let mut slugs: Vec<String> = spec.skills.clone();
    slugs.extend(execution_skill_slugs.iter().cloned());
    slugs.extend(profile_skill_slugs.iter().cloned());
    slugs.sort();
    slugs.dedup();
    if slugs.is_empty() {
        return Ok(Vec::new());
    }
    let skills: Vec<&Skill> = skills
        .iter()
        .filter(|skill| slugs.contains(&skill.slug))
        .collect();
    let mut materialized = Vec::new();
    for skill in skills {
        for base in [".pi/skills", ".opencode/skills"] {
            // The repository is untrusted: nothing here is written through a symlink it
            // shipped (a linked `.opencode/skills` would otherwise redirect these writes).
            let dir = Path::new(base).join(&skill.slug);
            let skill_file = dir.join("SKILL.md");
            orbit_harness::paths::write_workspace_file(workspace, &skill_file, &skill.content)
                .with_context(|| format!("could not write {}", skill_file.display()))?;
            if let Some(files) = skill.files.as_object() {
                for (path, content) in files {
                    if path.starts_with('/') || path.contains("..") || path.contains('\\') {
                        anyhow::bail!("skill {} contains invalid file path {path:?}", skill.slug);
                    }
                    let Some(content) = content.as_str() else {
                        anyhow::bail!(
                            "skill {} file {path:?} must contain string content",
                            skill.slug
                        );
                    };
                    let target = dir.join(path);
                    let is_script = path.starts_with("scripts/") || path.contains("/scripts/");
                    if is_script {
                        orbit_harness::paths::write_workspace_executable(
                            workspace, &target, content,
                        )
                        .with_context(|| format!("could not write {}", target.display()))?;
                        tracing::info!(skill = %skill.slug, path = %path, "materialized executable script");
                    } else {
                        orbit_harness::paths::write_workspace_file(workspace, &target, content)
                            .with_context(|| format!("could not write {}", target.display()))?;
                    }
                }
            }
        }
        materialized.push(skill.slug.clone());
    }
    Ok(materialized)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill() -> Skill {
        Skill {
            id: uuid::Uuid::now_v7(),
            org_id: uuid::Uuid::now_v7(),
            owner_user_id: None,
            owner_team_id: None,
            visibility: "org".into(),
            slug: "example".into(),
            name: "Example".into(),
            description: String::new(),
            content: "skill instructions".into(),
            files: serde_json::json!({"scripts/check.ps1": "Write-Output ok"}),
            version: 1,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    fn spec() -> AgentSpec {
        AgentSpec {
            harness: None,
            model: Default::default(),
            harness_config: Default::default(),
            instructions: orbit_domain::Instructions {
                system: "test".into(),
                constraints: Vec::new(),
            },
            connectors: Vec::new(),
            skills: vec!["example".into()],
            triggers: Vec::new(),
            autonomy: Default::default(),
            resources: Default::default(),
            memory: Default::default(),
            run_defaults: Default::default(),
        }
    }

    #[tokio::test]
    async fn reports_filesystem_failure_instead_of_reporting_skill_written() {
        let workspace = tempfile::tempdir().unwrap();
        tokio::fs::write(workspace.path().join(".pi"), "blocks directory creation")
            .await
            .unwrap();

        let error = materialize(&spec(), &[], &[], &[skill()], workspace.path())
            .await
            .unwrap_err();

        // The whole chain: which file could not be written, and why.
        let message = format!("{error:#}");
        assert!(message.contains("could not write"), "{message}");
        assert!(message.contains("not a directory"), "{message}");
    }

    #[tokio::test]
    async fn reports_skill_only_after_all_files_are_written() {
        let workspace = tempfile::tempdir().unwrap();

        let written = materialize(&spec(), &[], &[], &[skill()], workspace.path())
            .await
            .unwrap();

        assert_eq!(written, vec!["example".to_string()]);
        assert_eq!(
            tokio::fs::read_to_string(workspace.path().join(".pi/skills/example/SKILL.md"))
                .await
                .unwrap(),
            "skill instructions"
        );
        assert_eq!(
            tokio::fs::read_to_string(
                workspace
                    .path()
                    .join(".opencode/skills/example/scripts/check.ps1")
            )
            .await
            .unwrap(),
            "Write-Output ok"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn leaves_symlinked_opencode_skills_target_untouched() {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        tokio::fs::create_dir_all(workspace.path().join(".opencode"))
            .await
            .unwrap();
        std::os::unix::fs::symlink(outside.path(), workspace.path().join(".opencode/skills"))
            .unwrap();

        let error = materialize(&spec(), &[], &[], &[skill()], workspace.path())
            .await
            .unwrap_err();

        assert!(
            format!("{error:#}").contains("symlink"),
            "unexpected error: {error:#}"
        );
        assert!(!outside.path().join("example").exists());
        assert!(workspace
            .path()
            .join(".opencode/skills")
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink());
    }
}
