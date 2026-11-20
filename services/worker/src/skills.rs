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
            let dir = workspace.join(base).join(&skill.slug);
            tokio::fs::create_dir_all(&dir)
                .await
                .with_context(|| format!("could not create skill directory {}", dir.display()))?;
            tokio::fs::write(dir.join("SKILL.md"), &skill.content)
                .await
                .with_context(|| format!("could not write {}", dir.join("SKILL.md").display()))?;
            if let Some(files) = skill.files.as_object() {
                for (path, content) in files {
                    if path.starts_with('/') || path.contains("..") || path.contains('\\') {
                        anyhow::bail!("skill {} contains invalid file path {path:?}", skill.slug);
                    }
                    if let Some(content) = content.as_str() {
                        let target = dir.join(path);
                        if let Some(parent) = target.parent() {
                            tokio::fs::create_dir_all(parent).await.with_context(|| {
                                format!(
                                    "could not create skill file directory {}",
                                    parent.display()
                                )
                            })?;
                        }
                        tokio::fs::write(&target, content)
                            .await
                            .with_context(|| format!("could not write {}", target.display()))?;
                        // Make scripts executable.
                        let is_script = path.starts_with("scripts/") || path.contains("/scripts/");
                        if is_script {
                            #[cfg(unix)]
                            {
                                use std::os::unix::fs::PermissionsExt;
                                let mut perms = tokio::fs::metadata(&target).await?.permissions();
                                perms.set_mode(0o755);
                                tokio::fs::set_permissions(&target, perms).await?;
                            }
                            tracing::info!(skill = %skill.slug, path = %path, "materialized executable script");
                        }
                    } else {
                        anyhow::bail!(
                            "skill {} file {path:?} must contain string content",
                            skill.slug
                        );
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

        assert!(error
            .to_string()
            .contains("could not create skill directory"));
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
}
