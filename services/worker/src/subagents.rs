//! Materialize OpenCode-native subagents for a native Agent Team execution.
//!
//! The primary agent runs in one harness process; the remaining team agents become
//! project-local OpenCode subagents under `.opencode/agents`.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;
use uuid::Uuid;

fn slug(value: &str) -> String {
    let mut out = String::new();
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            out.push(character.to_ascii_lowercase());
        } else if matches!(character, '-' | '_' | ' ') && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

/// Primary-agent system instruction describing available native subagents.
pub fn system_instruction(input: &Value) -> Option<String> {
    let team = input.get("team")?;
    if team.get("mode").and_then(Value::as_str) != Some("native") {
        return None;
    }
    let subagents = team.get("subagents").and_then(Value::as_array)?;
    if subagents.is_empty() {
        return None;
    }
    let mut instruction = String::from(
        "You are the primary agent of an ORBIT Agent Team. Delegate specialist work to the following OpenCode subagents using the task/subagent tool. Do not do all specialist work yourself.\n\n",
    );
    for subagent in subagents {
        let name = subagent
            .get("slug")
            .and_then(Value::as_str)
            .or_else(|| subagent.get("name").and_then(Value::as_str))
            .unwrap_or("subagent");
        let description = subagent
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("specialist subagent");
        instruction.push_str(&format!("- @{name}: {description}\n"));
    }
    instruction.push_str(
        "\nUse their feedback to decide the next task and repeat until the team goal is complete and verified.\n",
    );
    Some(instruction)
}

/// Returns the materialized subagent names.
pub async fn materialize(
    input: &Value,
    workspace: &Path,
    models: &BTreeMap<Uuid, String>,
) -> anyhow::Result<Vec<String>> {
    let Some(team) = input.get("team") else {
        return Ok(Vec::new());
    };
    if team.get("mode").and_then(Value::as_str) != Some("native") {
        return Ok(Vec::new());
    }
    let Some(subagents) = team.get("subagents").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    if subagents.is_empty() {
        return Ok(Vec::new());
    }
    let directory = std::path::Path::new(".opencode/agents");
    let mut names = Vec::new();
    for subagent in subagents {
        let name = subagent
            .get("slug")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .or_else(|| subagent.get("name").and_then(Value::as_str))
            .unwrap_or("subagent");
        let file = slug(name);
        if file.is_empty() {
            continue;
        }
        let description = subagent
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("ORBIT team subagent");
        let instructions = subagent
            .get("instructions")
            .and_then(Value::as_str)
            .unwrap_or("You are a specialist subagent.");
        // Native subagents inherit the primary agent's model/provider. Writing a
        // per-subagent model here would point OpenCode at a provider whose credential was
        // only resolved for the primary run, which fails with "provider not found" or a
        // missing API key. Pipeline teams can still use per-step models.
        let mut document = String::from("---\n");
        document.push_str(&format!(
            "description: {}\n",
            serde_json::to_string(description).unwrap_or_else(|_| "\"subagent\"".into())
        ));
        document.push_str("mode: subagent\n");
        if let Some(model) = subagent
            .get("agentId")
            .and_then(Value::as_str)
            .and_then(|id| id.parse::<Uuid>().ok())
            .and_then(|id| models.get(&id))
        {
            document.push_str(&format!("model: {model}\n"));
        }
        document.push_str("---\n\n");
        document.push_str(instructions);
        document.push('\n');
        // Never through a symlink the repository shipped at `.opencode` or below.
        orbit_harness::paths::write_workspace_file(
            workspace,
            directory.join(format!("{file}.md")),
            document,
        )
        .map_err(|error| anyhow::anyhow!("could not write subagent {file}: {error}"))?;
        names.push(file);
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn writes_subagent_model_by_agent_id() {
        let agent_id = Uuid::now_v7();
        let dir = std::env::temp_dir().join(format!("orbit-subagents-{}", Uuid::now_v7()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let input = json!({
            "team": {
                "mode": "native",
                "subagents": [{
                    "agentId": agent_id,
                    "name": "Docs agent",
                    "description": "writes docs",
                    "instructions": "Be precise."
                }]
            }
        });
        let models = BTreeMap::from([(agent_id, "corp/docs-model".to_string())]);
        materialize(&input, &dir, &models).await.unwrap();
        let body = tokio::fs::read_to_string(dir.join(".opencode/agents/docs-agent.md"))
            .await
            .unwrap();
        assert!(body.contains("model: corp/docs-model"));
        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn subagents_do_not_receive_primary_instructions() {
        let dir = std::env::temp_dir().join(format!("orbit-subagents-{}", Uuid::now_v7()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let primary = "AURORA-259-PRIMARY-AGENT-INSTRUCTIONS";
        let own = "AURORA-259-SUBAGENT-OWN-INSTRUCTIONS";
        let input = json!({
            "task": "AURORA-259-RUN-INSTRUCTION",
            "team": {
                "mode": "native",
                "goal": primary,
                "primaryInstructions": primary,
                "subagents": [{
                    "agentId": Uuid::now_v7(),
                    "slug": "reviewer",
                    "name": "Reviewer",
                    "description": "reviews code",
                    "instructions": own
                }]
            }
        });
        materialize(&input, &dir, &BTreeMap::new()).await.unwrap();
        let body = tokio::fs::read_to_string(dir.join(".opencode/agents/reviewer.md"))
            .await
            .unwrap();
        assert!(body.contains(own));
        assert!(!body.contains(primary));
        assert!(!body.contains("AURORA-259-RUN-INSTRUCTION"));

        let delegation = system_instruction(&input).unwrap();
        assert!(delegation.contains("@reviewer"));
        assert!(!delegation.contains(primary));
        assert!(!delegation.contains(own));
        let _ = tokio::fs::remove_dir_all(dir).await;
    }
}
