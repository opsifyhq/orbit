//! Agent Team workflow runs (#9).
//!
//! The rules live in `orbit_domain::workflow`: [`plan`] reads the steps a run has created
//! and says which steps start now and whether the run is over. This module is the part
//! that touches the database: it records how each step ended, creates the executions the
//! plan asks for and finishes the run. It never looks at the work itself; the only thing
//! it reads from a step's answer is the `OUTCOME:` line the step was told to end with.
//!
//! Everything here runs under the per-run advance lease (see `handlers::teams::advance`),
//! and every write is conditional or idempotent, so a tick that dies half-way is simply
//! repeated: the plan is recomputed from the steps that exist.

use serde_json::Value;

use orbit_domain::{
    final_steps, outcome_directive, parse_outcome, plan, WorkflowActivation, WorkflowDefinition,
    WorkflowNode, WorkflowStepState, WorkflowStepStatus, WorkflowVerdict,
};
use orbit_store::{TeamRun, TeamStep};

use crate::error::ApiError;
use crate::handlers::teams::{
    announce_cancel, auto_launch_step, execution_error_message, fail_run_after_launch_error,
    finalize_run, is_temporary_limit, start_step_with, FirstSteps, StepStart, TeamDefinition,
    TeamModelOverride, WorkflowStepStart, ADVANCE_LEASE_SECS, LIMITED_STEP_RETRY_WINDOW_SECS,
    START_STEP_RETRY_WINDOW_SECS,
};
use crate::state::AppState;

/// The configured context for one workflow step: the team goal, what this step is for
/// and, when the step has outcomes, how to report one.
pub(crate) fn step_goal(goal: &str, node: &WorkflowNode) -> String {
    let mut out = goal.trim().to_string();
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(&format!("Your step in this workflow: {}", node.name));
    if !node.instructions.is_empty() {
        out.push_str("\n\n");
        out.push_str(&node.instructions);
    }
    if let Some(directive) = outcome_directive(&node.outcomes) {
        out.push_str("\n\n");
        out.push_str(&directive);
    }
    out
}

fn states(steps: &[TeamStep]) -> Vec<WorkflowStepState> {
    steps
        .iter()
        .map(|step| WorkflowStepState {
            index: step.step_index,
            node_id: step.node_id.clone().unwrap_or_default(),
            status: WorkflowStepStatus::parse(&step.status),
            outcome: step.outcome.clone(),
            activated_by: serde_json::from_value(step.activated_by.clone()).unwrap_or_default(),
        })
        .collect()
}

/// The answer text of an execution output: `text` for real harnesses, `summary` for the
/// mock one.
fn answer_text(output: Option<&Value>) -> &str {
    let Some(output) = output else { return "" };
    output
        .as_str()
        .or_else(|| output.get("text").and_then(Value::as_str))
        .or_else(|| output.get("summary").and_then(Value::as_str))
        .unwrap_or("")
}

/// One value for the outputs of several steps: the output itself when there is one,
/// otherwise an object keyed by step name.
fn combined_output(steps: &[&TeamStep]) -> Option<Value> {
    match steps {
        [] => None,
        [only] => only.output.clone(),
        many => Some(Value::Object(
            many.iter()
                .map(|step| {
                    (
                        step.name.clone(),
                        step.output.clone().unwrap_or(Value::Null),
                    )
                })
                .collect(),
        )),
    }
}

async fn is_terminal(state: &AppState, execution_id: uuid::Uuid) -> anyhow::Result<bool> {
    let execution = state.store.get_execution_by_id(execution_id).await?;
    Ok(matches!(
        orbit_domain::ExecutionStatus::parse(&execution.status),
        Some(status) if status.is_terminal()
    ))
}

/// Whether advancing the run can change anything: a running step's execution ended, a
/// step is due to start, or the run is over.
pub(crate) async fn has_work(
    state: &AppState,
    run: &TeamRun,
    workflow: &WorkflowDefinition,
) -> anyhow::Result<bool> {
    let steps = state.store.list_team_steps(run.id).await?;
    for step in steps.iter().filter(|step| step.status == "running") {
        if let Some(execution_id) = step.execution_id {
            if is_terminal(state, execution_id).await? {
                return Ok(true);
            }
        }
    }
    let plan = plan(workflow, &states(&steps));
    Ok(!plan.start.is_empty() || plan.verdict != WorkflowVerdict::Running)
}

/// Record how each running step ended, once its execution is terminal. Returns whether
/// any step changed.
async fn settle(
    state: &AppState,
    workflow: &WorkflowDefinition,
    steps: &[TeamStep],
) -> anyhow::Result<bool> {
    let mut changed = false;
    for step in steps.iter().filter(|step| step.status == "running") {
        let Some(execution_id) = step.execution_id else {
            continue;
        };
        let execution = state.store.get_execution_by_id(execution_id).await?;
        if !matches!(
            orbit_domain::ExecutionStatus::parse(&execution.status),
            Some(status) if status.is_terminal()
        ) {
            continue;
        }
        let declared = step
            .node_id
            .as_deref()
            .and_then(|id| workflow.node(id))
            .map(|node| node.outcomes.as_slice())
            .unwrap_or_default();
        let (status, outcome, error) = if execution.status != "succeeded" {
            ("failed", None, Some(execution_error_message(&execution)))
        } else if declared.is_empty() {
            ("succeeded", None, None)
        } else {
            match parse_outcome(answer_text(execution.output.as_ref()), declared) {
                Some(outcome) => ("succeeded", Some(outcome), None),
                // The step cannot be routed, so it counts as failed. A "failed" line
                // from this step still handles it.
                None => (
                    "failed",
                    None,
                    Some(format!(
                        "the step finished without reporting an outcome (expected one of: {})",
                        declared.join(", ")
                    )),
                ),
            }
        };
        changed |= state
            .store
            .finish_team_step_with_outcome(
                step.id,
                status,
                execution.output.as_ref(),
                error.as_deref(),
                outcome.as_deref(),
            )
            .await?
            .is_some();
    }
    Ok(changed)
}

async fn cancel_live_executions(state: &AppState, run: &TeamRun) {
    match state
        .store
        .request_team_run_cancel(run.org_id, run.id)
        .await
    {
        Ok(cancelled) => announce_cancel(state, run.org_id, &cancelled).await,
        Err(error) => {
            tracing::warn!(%error, run_id = %run.id, "failed to stop executions of an ended workflow run")
        }
    }
    if let Err(error) = state
        .store
        .close_running_team_steps(run.id, "cancelled", Some("stopped because the run ended"))
        .await
    {
        tracing::warn!(%error, run_id = %run.id, "failed to close steps of an ended workflow run");
    }
}

async fn start_activation(
    state: &AppState,
    run: &TeamRun,
    definition: &TeamDefinition,
    workflow: &WorkflowDefinition,
    steps: &[TeamStep],
    activation: &WorkflowActivation,
    step_index: i32,
) -> Result<TeamStep, ApiError> {
    let node = workflow
        .node(&activation.node_id)
        .ok_or_else(|| ApiError::internal("workflow step is missing from the run definition"))?;
    // What the step receives: the output of the step that handed over, or of each of
    // them (by step name) when several did.
    let sources: Vec<&TeamStep> = activation
        .tokens
        .iter()
        .filter_map(|token| token.step)
        .filter_map(|index| steps.iter().find(|step| step.step_index == index))
        .collect();
    let previous = combined_output(&sources);
    let model_override = node
        .model
        .as_ref()
        .map(|model| TeamModelOverride {
            provider: model.provider.clone(),
            model: model.model.clone(),
            management: model.management.clone(),
        })
        .or_else(|| definition.model.clone());
    start_step_with(
        state,
        run,
        definition,
        StepStart {
            step_index: step_index as usize,
            agent_id: node.agent_id,
            model_override,
            previous_output: previous.as_ref(),
            workflow: Some(WorkflowStepStart {
                node,
                attempt: activation.attempt,
                activated_by: serde_json::to_value(&activation.tokens).unwrap_or_default(),
            }),
        },
    )
    .await
}

/// Start a new run's first steps. Takes the advance lease so the background loop cannot
/// start the same steps at the same moment. The loop may have started them already (a
/// workflow run without steps counts as work for it), so this plans from the steps that
/// exist and reports only the ones it created itself as `started`.
pub(crate) async fn start_run(
    state: &AppState,
    run: &TeamRun,
    definition: &TeamDefinition,
    workflow: &WorkflowDefinition,
) -> Result<FirstSteps, ApiError> {
    let Some((_, lease)) = state
        .store
        .claim_team_run_advance(run.id, ADVANCE_LEASE_SECS)
        .await?
    else {
        return Ok(FirstSteps {
            steps: state.store.list_team_steps(run.id).await?,
            started: Vec::new(),
        });
    };
    let result = async {
        let mut steps = state.store.list_team_steps(run.id).await?;
        let mut started = Vec::new();
        let first_index = steps
            .iter()
            .map(|step| step.step_index + 1)
            .max()
            .unwrap_or(0);
        let activations = plan(workflow, &states(&steps)).start;
        for (offset, activation) in activations.iter().enumerate() {
            let step = start_activation(
                state,
                run,
                definition,
                workflow,
                &steps,
                activation,
                first_index + offset as i32,
            )
            .await?;
            started.push(step);
        }
        steps.extend(started.iter().cloned());
        Ok(FirstSteps { steps, started })
    }
    .await;
    if let Err(error) = state.store.release_team_run_advance(run.id, lease).await {
        tracing::warn!(%error, run_id = %run.id, "failed to release team run advance lease");
    }
    result
}

/// Move a workflow run forward. Called with the advance lease held.
pub(crate) async fn advance(
    state: &AppState,
    run: &TeamRun,
    definition: &TeamDefinition,
    workflow: &WorkflowDefinition,
) -> anyhow::Result<()> {
    let mut steps = state.store.list_team_steps(run.id).await?;
    if settle(state, workflow, &steps).await? {
        steps = state.store.list_team_steps(run.id).await?;
    }
    let step_states = states(&steps);
    let plan = plan(workflow, &step_states);
    match plan.verdict {
        WorkflowVerdict::Succeeded => {
            let finals = final_steps(workflow, &step_states);
            let outputs: Vec<&TeamStep> = steps
                .iter()
                .filter(|step| finals.contains(&step.step_index))
                .collect();
            let output = combined_output(&outputs);
            return finalize_run(state, run, "succeeded", output.as_ref(), None).await;
        }
        WorkflowVerdict::Failed(failure) => {
            // Other branches may still be working; the run is over, so stop them.
            cancel_live_executions(state, run).await;
            let failed = failure
                .step
                .and_then(|index| steps.iter().find(|step| step.step_index == index));
            let message = match failed {
                Some(step) => format!(
                    "step '{}' finished with {}",
                    step.name,
                    step.error.as_deref().unwrap_or("failure")
                ),
                None => failure.message,
            };
            return finalize_run(
                state,
                run,
                "failed",
                failed.and_then(|step| step.output.as_ref()),
                Some(&message),
            )
            .await;
        }
        WorkflowVerdict::Running => {}
    }

    let mut next_index = steps
        .iter()
        .map(|step| step.step_index + 1)
        .max()
        .unwrap_or(0);
    for activation in &plan.start {
        let name = workflow
            .node(&activation.node_id)
            .map(|node| node.name.as_str())
            .unwrap_or(&activation.node_id);
        let error = match start_activation(
            state, run, definition, workflow, &steps, activation, next_index,
        )
        .await
        {
            Ok(created) => {
                next_index += 1;
                if let Err(error) = auto_launch_step(state, run, definition, &created, None).await {
                    fail_run_after_launch_error(state, run, &created, &error).await;
                    return Ok(());
                }
                continue;
            }
            Err(error) => error,
        };
        // Same policy as a pipeline's next step: a refusal fails the run, a limit that
        // clears by itself is waited for, anything else is retried for a while.
        let limited = is_temporary_limit(&error);
        if error.status.is_client_error() && !limited {
            cancel_live_executions(state, run).await;
            return crate::preflight::fail_team_run(state, run, &error).await;
        }
        // Waiting is counted from the moment the step could have started.
        let since = activation
            .tokens
            .iter()
            .filter_map(|token| token.step)
            .filter_map(|index| steps.iter().find(|step| step.step_index == index))
            .filter_map(|step| step.finished_at)
            .max()
            .unwrap_or(run.created_at);
        let waited = (chrono::Utc::now() - since).num_seconds();
        let window = if limited {
            LIMITED_STEP_RETRY_WINDOW_SECS
        } else {
            START_STEP_RETRY_WINDOW_SECS
        };
        if waited >= window {
            cancel_live_executions(state, run).await;
            let message = format!("could not start step '{name}': {}", error.message);
            return finalize_run(state, run, "failed", None, Some(&message)).await;
        }
        if limited {
            tracing::debug!(run_id = %run.id, step = name, reason = %error.message, "workflow step waits for a limit");
            continue;
        }
        return Err(anyhow::anyhow!(error.message));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn node(outcomes: &[&str], instructions: &str) -> WorkflowNode {
        WorkflowNode {
            id: "test".into(),
            name: "Run the tests".into(),
            agent_id: uuid::Uuid::nil(),
            instructions: instructions.into(),
            outcomes: outcomes.iter().map(|o| o.to_string()).collect(),
            max_runs: None,
            model: None,
            position: None,
        }
    }

    #[test]
    fn a_step_goal_keeps_the_team_goal_and_adds_the_step() {
        let goal = step_goal("Ship the fix.", &node(&["pass", "fail"], "Run cargo test."));
        assert!(goal.starts_with("Ship the fix.\n\nYour step in this workflow: Run the tests"));
        assert!(goal.contains("Run cargo test."));
        assert!(goal.contains("OUTCOME: <pass | fail>"));
        let plain = step_goal("", &node(&[], ""));
        assert_eq!(plain, "Your step in this workflow: Run the tests");
    }

    #[test]
    fn the_answer_is_read_from_text_or_summary() {
        assert_eq!(answer_text(Some(&json!({ "text": "a" }))), "a");
        assert_eq!(answer_text(Some(&json!({ "summary": "b" }))), "b");
        assert_eq!(answer_text(Some(&json!("c"))), "c");
        assert_eq!(answer_text(Some(&json!({ "other": 1 }))), "");
        assert_eq!(answer_text(None), "");
    }
}
