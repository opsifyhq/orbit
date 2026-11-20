//! Prompt provenance for harness executions.
//!
//! #259: the run instruction, configured system instructions and delegated context
//! must stay separate from the control plane all the way to the adapter. Only
//! [`ExecutionInput::task`] is presented as the task; agent/team instructions travel in
//! the trusted instructions block, and subagent context is delegation metadata.

/// Push a labelled, trusted system section. Kept as one helper so every source of
/// configured instructions follows the same boundary.
fn push_section(out: &mut String, heading: &str, body: &str) {
    let body = body.trim();
    if body.is_empty() {
        return;
    }
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(heading);
    out.push_str("\n\n");
    out.push_str(body);
}

/// Build the trusted system instructions for this execution.
///
/// Sources, in order: the agent's configured instructions, the team goal (configuration,
/// never a user task) and native-team delegation guidance. The run instruction is not
/// included and must not be duplicated here.
pub fn system_instructions(spec_system: &str, raw_input: &serde_json::Value) -> String {
    let mut out = spec_system.trim_end().to_string();
    if let Some(goal) = raw_input
        .get("team")
        .and_then(|team| team.get("goal"))
        .and_then(|value| value.as_str())
    {
        push_section(&mut out, "Team goal (configured)", goal);
    }
    if let Some(delegation) = crate::subagents::system_instruction(raw_input) {
        push_section(&mut out, "Native subagents (delegation)", &delegation);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbit_contracts::ExecutionInput;
    use serde_json::json;

    const TASK: &str = "AURORA-259-UNIQUE-RUN-INSTRUCTION";
    const AGENT: &str = "AURORA-259-AGENT-SYSTEM-INSTRUCTIONS";
    const GOAL: &str = "AURORA-259-TEAM-CONFIGURED-GOAL";

    #[test]
    fn run_instruction_is_never_copied_into_system_instructions() {
        let raw = json!({
            "task": TASK,
            "instructions": GOAL,
            "team": { "mode": "pipeline", "goal": GOAL },
            "provenance": {
                "source": "user:test@example.com",
                "trusted": true,
                "receivedAt": chrono::Utc::now(),
            },
        });
        let system = system_instructions(AGENT, &raw);
        assert!(system.contains(AGENT));
        assert!(system.contains(GOAL));
        assert!(!system.contains(TASK));

        // The adapter still sees exactly one run instruction.
        let input: ExecutionInput = serde_json::from_value(raw).unwrap();
        assert_eq!(input.task, TASK);
        assert_eq!(input.task.matches(TASK).count(), 1);
    }

    #[test]
    fn native_team_keeps_delegation_separate_from_instructions() {
        let raw = json!({
            "task": TASK,
            "team": {
                "mode": "native",
                "goal": GOAL,
                "subagents": [{
                    "agentId": uuid::Uuid::now_v7(),
                    "name": "Reviewer",
                    "slug": "reviewer",
                    "description": "reviews code",
                    "instructions": "AURORA-259-SUBAGENT-INSTRUCTIONS"
                }]
            }
        });
        let system = system_instructions(AGENT, &raw);
        assert!(system.contains("Team goal (configured)"));
        assert!(system.contains("Native subagents (delegation)"));
        assert!(system.contains("@reviewer"));
        assert!(!system.contains("AURORA-259-SUBAGENT-INSTRUCTIONS"));
        assert!(!system.contains(TASK));
    }
}
