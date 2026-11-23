//! Agent Team workflows: a graph of agent steps instead of a fixed line.
//!
//! A workflow is a set of **steps** (each runs one agent as a normal execution) joined by
//! **lines**. A line carries a condition; when a step finishes, every line whose condition
//! matches hands the work to the step it points at. That gives branching (different lines
//! for different results), parallel work (several lines match at once), joins (a step
//! with several incoming lines) and loops (a line that points back), with a limit on how
//! often one step may run.
//!
//! This module is pure. It validates a definition and, given the steps a run has created
//! so far, decides what happens next ([`plan`]). The control plane persists the steps and
//! creates the executions; it never reasons about the work itself, so every condition
//! here is deterministic: the execution's status, or an outcome label the agent reported.
//!
//! ## When a step starts
//!
//! A finished step *delivers* on each matching line. A step that has deliveries waiting
//! starts as soon as no step that is still busy could deliver more to it; it then takes
//! everything that is waiting. So a step after two parallel branches waits for both, a
//! step after an either/or branch does not wait for the branch that was not taken, and a
//! loop simply delivers to an earlier step again.
//!
//! ## When a run ends
//!
//! - **Failed** when a step fails and no line handles the failure, or when a step would
//!   run more often than its limit.
//! - **Succeeded** when nothing is running and nothing is waiting.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::ValidationIssue;

/// The reserved id that lines leave from to mark where a run begins.
pub const WORKFLOW_START: &str = "start";
/// Team mode that runs a workflow.
pub const WORKFLOW_MODE: &str = "workflow";
pub const WORKFLOW_MAX_NODES: usize = 30;
pub const WORKFLOW_MAX_EDGES: usize = 120;
/// How often one step may run in one run, unless the step says otherwise.
pub const WORKFLOW_DEFAULT_MAX_RUNS: u32 = 3;
pub const WORKFLOW_MAX_RUNS_LIMIT: u32 = 10;
/// Upper bound on the steps one run may create, whatever the per-step limits allow.
pub const WORKFLOW_MAX_STEPS_PER_RUN: usize = 100;
const MAX_OUTCOMES: usize = 8;
const MAX_NAME_CHARS: usize = 60;
const MAX_INSTRUCTION_CHARS: usize = 4_000;

/// When a line hands work on.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default, utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowWhen {
    /// The step finished, whatever the result.
    Always,
    /// The step's execution succeeded.
    #[default]
    Success,
    /// The step's execution failed. A line like this handles the failure, so the run goes on.
    Failure,
    /// The step succeeded and reported the outcome named in `outcome`.
    Outcome,
}

/// A model override for one step. Same shape as the team-level override.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowModel {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub management: Option<String>,
}

/// Where a step sits on the canvas. Presentation only; the engine ignores it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct WorkflowPosition {
    pub x: f64,
    pub y: f64,
}

/// One step of a workflow: an agent and what it should do here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowNode {
    /// Stable id inside the workflow (lower-case letters, digits, `-` and `_`).
    pub id: String,
    pub name: String,
    pub agent_id: Uuid,
    /// What this step should do, added to the team goal for this step only.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub instructions: String,
    /// Outcome labels the agent chooses from when it finishes, such as `pass` and `fail`.
    /// Empty when the step has no outcome lines.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outcomes: Vec<String>,
    /// How often this step may run in one run. Only matters inside a loop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_runs: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<WorkflowModel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<WorkflowPosition>,
}

impl WorkflowNode {
    pub fn max_runs(&self) -> u32 {
        self.max_runs.unwrap_or(WORKFLOW_DEFAULT_MAX_RUNS)
    }
}

/// A line from one step (or from `start`) to another.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowEdge {
    pub id: String,
    /// A step id, or `start`.
    pub from: String,
    pub to: String,
    /// Ignored on lines that leave `start`.
    #[serde(default)]
    pub when: WorkflowWhen,
    /// The outcome label, when `when` is `outcome`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowDefinition {
    pub nodes: Vec<WorkflowNode>,
    #[serde(default)]
    pub edges: Vec<WorkflowEdge>,
}

fn is_slug(value: &str, max: usize) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_lowercase() || first.is_ascii_digit())
        && value.len() <= max
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

impl WorkflowDefinition {
    pub fn node(&self, id: &str) -> Option<&WorkflowNode> {
        self.nodes.iter().find(|node| node.id == id)
    }

    /// The agents the workflow uses, each once, in step order.
    pub fn agent_ids(&self) -> Vec<Uuid> {
        let mut seen = HashSet::new();
        self.nodes
            .iter()
            .filter(|node| seen.insert(node.agent_id))
            .map(|node| node.agent_id)
            .collect()
    }

    /// Steps reachable from each step by following one or more lines.
    fn reach(&self) -> HashMap<&str, HashSet<&str>> {
        let mut next: HashMap<&str, Vec<&str>> = HashMap::new();
        for edge in &self.edges {
            if edge.from != WORKFLOW_START {
                next.entry(edge.from.as_str())
                    .or_default()
                    .push(edge.to.as_str());
            }
        }
        let mut reach = HashMap::new();
        for node in &self.nodes {
            let mut seen: HashSet<&str> = HashSet::new();
            let mut stack: Vec<&str> = next.get(node.id.as_str()).cloned().unwrap_or_default();
            while let Some(current) = stack.pop() {
                if seen.insert(current) {
                    stack.extend(next.get(current).into_iter().flatten().copied());
                }
            }
            reach.insert(node.id.as_str(), seen);
        }
        reach
    }

    /// Check the definition. Paths are relative to the team body (`workflow.…`).
    pub fn validate(&self) -> Vec<ValidationIssue> {
        let mut issues = Vec::new();
        if self.nodes.is_empty() {
            issues.push(ValidationIssue::new(
                "workflow.nodes",
                "a workflow needs at least one step",
            ));
            return issues;
        }
        if self.nodes.len() > WORKFLOW_MAX_NODES {
            issues.push(ValidationIssue::new(
                "workflow.nodes",
                format!("a workflow can have at most {WORKFLOW_MAX_NODES} steps"),
            ));
        }
        if self.edges.len() > WORKFLOW_MAX_EDGES {
            issues.push(ValidationIssue::new(
                "workflow.edges",
                format!("a workflow can have at most {WORKFLOW_MAX_EDGES} lines"),
            ));
        }
        let mut ids = HashSet::new();
        let mut names = HashSet::new();
        for (index, node) in self.nodes.iter().enumerate() {
            let path = |field: &str| format!("workflow.nodes[{index}].{field}");
            if node.id == WORKFLOW_START || !is_slug(&node.id, 40) {
                issues.push(ValidationIssue::new(
                    path("id"),
                    "use up to 40 lower-case letters, digits, '-' or '_'; 'start' is reserved",
                ));
            } else if !ids.insert(node.id.as_str()) {
                issues.push(ValidationIssue::new(
                    path("id"),
                    format!("step id '{}' is used twice", node.id),
                ));
            }
            let name = node.name.trim();
            if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
                issues.push(ValidationIssue::new(
                    path("name"),
                    format!("a step needs a name of at most {MAX_NAME_CHARS} characters"),
                ));
            } else if !names.insert(name.to_lowercase()) {
                issues.push(ValidationIssue::new(
                    path("name"),
                    format!("two steps are called '{name}'"),
                ));
            }
            if node.instructions.chars().count() > MAX_INSTRUCTION_CHARS {
                issues.push(ValidationIssue::new(
                    path("instructions"),
                    format!("at most {MAX_INSTRUCTION_CHARS} characters"),
                ));
            }
            if node.outcomes.len() > MAX_OUTCOMES {
                issues.push(ValidationIssue::new(
                    path("outcomes"),
                    format!("a step can have at most {MAX_OUTCOMES} outcomes"),
                ));
            }
            let mut outcomes = HashSet::new();
            for outcome in &node.outcomes {
                if !is_slug(outcome, 32) {
                    issues.push(ValidationIssue::new(
                        path("outcomes"),
                        format!("outcome '{outcome}' must be up to 32 lower-case letters, digits, '-' or '_'"),
                    ));
                } else if !outcomes.insert(outcome.as_str()) {
                    issues.push(ValidationIssue::new(
                        path("outcomes"),
                        format!("outcome '{outcome}' is listed twice"),
                    ));
                }
            }
            if node
                .max_runs
                .is_some_and(|runs| !(1..=WORKFLOW_MAX_RUNS_LIMIT).contains(&runs))
            {
                issues.push(ValidationIssue::new(
                    path("maxRuns"),
                    format!("must be 1 to {WORKFLOW_MAX_RUNS_LIMIT}"),
                ));
            }
        }
        let mut edge_ids = HashSet::new();
        let mut shapes = HashSet::new();
        let mut entries = Vec::new();
        for (index, edge) in self.edges.iter().enumerate() {
            let path = |field: &str| format!("workflow.edges[{index}].{field}");
            if edge.id.trim().is_empty() || !edge_ids.insert(edge.id.as_str()) {
                issues.push(ValidationIssue::new(
                    path("id"),
                    "every line needs its own id",
                ));
            }
            let from_start = edge.from == WORKFLOW_START;
            let source = self.node(&edge.from);
            if !from_start && source.is_none() {
                issues.push(ValidationIssue::new(
                    path("from"),
                    format!("unknown step '{}'", edge.from),
                ));
            }
            if self.node(&edge.to).is_none() {
                issues.push(ValidationIssue::new(
                    path("to"),
                    format!("unknown step '{}'", edge.to),
                ));
            }
            if from_start {
                entries.push(edge.to.as_str());
                if !shapes.insert((edge.from.as_str(), edge.to.as_str(), None, None)) {
                    issues.push(ValidationIssue::new(
                        path("to"),
                        "this step is already a starting step",
                    ));
                }
                continue;
            }
            match (edge.when, edge.outcome.as_deref()) {
                (WorkflowWhen::Outcome, None) => issues.push(ValidationIssue::new(
                    path("outcome"),
                    "choose the outcome this line follows",
                )),
                (WorkflowWhen::Outcome, Some(outcome)) => {
                    if source.is_some_and(|node| !node.outcomes.iter().any(|o| o == outcome)) {
                        issues.push(ValidationIssue::new(
                            path("outcome"),
                            format!(
                                "step '{}' has no outcome '{outcome}'",
                                source.map(|node| node.name.as_str()).unwrap_or_default()
                            ),
                        ));
                    }
                }
                (_, Some(_)) => issues.push(ValidationIssue::new(
                    path("outcome"),
                    "only a line that follows an outcome can name one",
                )),
                _ => {}
            }
            if !shapes.insert((
                edge.from.as_str(),
                edge.to.as_str(),
                Some(edge.when),
                edge.outcome.as_deref(),
            )) {
                issues.push(ValidationIssue::new(
                    path("to"),
                    "the same line exists twice",
                ));
            }
        }
        if entries.is_empty() {
            issues.push(ValidationIssue::new(
                "workflow.edges",
                "choose at least one starting step",
            ));
        }
        if issues.is_empty() {
            let reach = self.reach();
            let mut reachable: HashSet<&str> = HashSet::new();
            for entry in &entries {
                reachable.insert(entry);
                reachable.extend(reach.get(entry).into_iter().flatten().copied());
            }
            for (index, node) in self.nodes.iter().enumerate() {
                if !reachable.contains(node.id.as_str()) {
                    issues.push(ValidationIssue::new(
                        format!("workflow.nodes[{index}]"),
                        format!(
                            "step '{}' can never run: no line leads to it",
                            node.name.trim()
                        ),
                    ));
                }
            }
        }
        issues
    }
}

/// A delivery: one line that handed work on, and the step that sent it (`None` for `start`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct WorkflowToken {
    pub edge: String,
    #[serde(default)]
    pub step: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowStepStatus {
    Running,
    Succeeded,
    Failed,
}

impl WorkflowStepStatus {
    pub fn parse(status: &str) -> Self {
        match status {
            "succeeded" => Self::Succeeded,
            "running" | "pending" => Self::Running,
            _ => Self::Failed,
        }
    }
}

/// What the engine needs to know about a step a run has created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowStepState {
    pub index: i32,
    pub node_id: String,
    pub status: WorkflowStepStatus,
    pub outcome: Option<String>,
    pub activated_by: Vec<WorkflowToken>,
}

/// A step to start now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowActivation {
    pub node_id: String,
    /// 1 for the first time the step runs in this run.
    pub attempt: u32,
    /// The deliveries this start takes.
    pub tokens: Vec<WorkflowToken>,
}

/// Why a run failed. `step` is the step that failed, when one did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowFailure {
    pub message: String,
    pub step: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowVerdict {
    /// Steps are running or about to start.
    Running,
    Succeeded,
    Failed(WorkflowFailure),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowPlan {
    pub start: Vec<WorkflowActivation>,
    pub verdict: WorkflowVerdict,
}

fn edge_matches(edge: &WorkflowEdge, step: &WorkflowStepState) -> bool {
    match (edge.when, step.status) {
        (_, WorkflowStepStatus::Running) => false,
        (WorkflowWhen::Always, _) => true,
        (WorkflowWhen::Success, status) => status == WorkflowStepStatus::Succeeded,
        (WorkflowWhen::Failure, status) => status == WorkflowStepStatus::Failed,
        (WorkflowWhen::Outcome, status) => {
            status == WorkflowStepStatus::Succeeded
                && edge.outcome.is_some()
                && edge.outcome == step.outcome
        }
    }
}

/// The lines a finished step delivered on.
pub fn delivered_edges<'a>(
    workflow: &'a WorkflowDefinition,
    step: &WorkflowStepState,
) -> Vec<&'a WorkflowEdge> {
    workflow
        .edges
        .iter()
        .filter(|edge| edge.from == step.node_id && edge_matches(edge, step))
        .collect()
}

/// Decide what a run does next from the steps it has created so far.
///
/// Pure and repeatable: the same steps always give the same plan, so a control-plane
/// replica that crashed half-way simply plans again.
pub fn plan(workflow: &WorkflowDefinition, steps: &[WorkflowStepState]) -> WorkflowPlan {
    let failed_at = |message: String, step: Option<i32>| WorkflowPlan {
        start: Vec::new(),
        verdict: WorkflowVerdict::Failed(WorkflowFailure { message, step }),
    };
    let failed = |message: String| failed_at(message, None);
    let name = |node_id: &str| {
        workflow
            .node(node_id)
            .map(|node| node.name.trim().to_string())
            .unwrap_or_else(|| node_id.to_string())
    };

    // 1. Everything delivered so far, minus what a step has already taken.
    let mut tokens: BTreeSet<(String, WorkflowToken)> = BTreeSet::new();
    for edge in &workflow.edges {
        if edge.from == WORKFLOW_START {
            tokens.insert((
                edge.to.clone(),
                WorkflowToken {
                    edge: edge.id.clone(),
                    step: None,
                },
            ));
        }
    }
    for step in steps {
        if step.status == WorkflowStepStatus::Running {
            continue;
        }
        let delivered = delivered_edges(workflow, step);
        // 2. A failure nobody handles ends the run.
        if step.status == WorkflowStepStatus::Failed && delivered.is_empty() {
            return failed_at(
                format!("step '{}' failed", name(&step.node_id)),
                Some(step.index),
            );
        }
        for edge in delivered {
            tokens.insert((
                edge.to.clone(),
                WorkflowToken {
                    edge: edge.id.clone(),
                    step: Some(step.index),
                },
            ));
        }
    }
    let taken: HashSet<&WorkflowToken> = steps
        .iter()
        .flat_map(|step| step.activated_by.iter())
        .collect();
    let mut waiting: BTreeMap<String, Vec<WorkflowToken>> = BTreeMap::new();
    for (target, token) in tokens {
        if !taken.contains(&token) {
            waiting.entry(target).or_default().push(token);
        }
    }

    let running: HashSet<&str> = steps
        .iter()
        .filter(|step| step.status == WorkflowStepStatus::Running)
        .map(|step| step.node_id.as_str())
        .collect();
    if waiting.is_empty() {
        return WorkflowPlan {
            start: Vec::new(),
            verdict: if running.is_empty() {
                WorkflowVerdict::Succeeded
            } else {
                WorkflowVerdict::Running
            },
        };
    }

    // 3. A waiting step starts once nothing busy can still deliver to it.
    let reach = workflow.reach();
    let reaches = |from: &str, to: &str| reach.get(from).is_some_and(|set| set.contains(to));
    let mut start = Vec::new();
    for (node_id, node_tokens) in &waiting {
        let target = node_id.as_str();
        if running.contains(target) {
            continue;
        }
        let blocked_by_running = running.iter().any(|busy| reaches(busy, target));
        // Two waiting steps on one loop would wait for each other; let both go.
        let blocked_by_waiting = waiting
            .keys()
            .any(|other| other != node_id && reaches(other, target) && !reaches(target, other));
        if blocked_by_running || blocked_by_waiting {
            continue;
        }
        let Some(node) = workflow.node(target) else {
            return failed(format!("the workflow has no step '{target}'"));
        };
        let runs = steps.iter().filter(|step| step.node_id == *node_id).count() as u32;
        if runs >= node.max_runs() {
            return failed(format!(
                "step '{}' was due to run again but reached its limit of {} {}",
                name(target),
                node.max_runs(),
                if node.max_runs() == 1 { "run" } else { "runs" }
            ));
        }
        start.push(WorkflowActivation {
            node_id: node_id.clone(),
            attempt: runs + 1,
            tokens: node_tokens.clone(),
        });
    }
    if steps.len() + start.len() > WORKFLOW_MAX_STEPS_PER_RUN {
        return failed(format!(
            "the run reached the limit of {WORKFLOW_MAX_STEPS_PER_RUN} steps"
        ));
    }
    WorkflowPlan {
        start,
        verdict: WorkflowVerdict::Running,
    }
}

/// The steps whose output is the result of a finished run: succeeded steps that handed
/// the work to nobody. Latest run of each step, in the order they were created.
pub fn final_steps(workflow: &WorkflowDefinition, steps: &[WorkflowStepState]) -> Vec<i32> {
    let mut latest: BTreeMap<&str, i32> = BTreeMap::new();
    for step in steps {
        if step.status == WorkflowStepStatus::Succeeded
            && delivered_edges(workflow, step).is_empty()
        {
            let entry = latest.entry(step.node_id.as_str()).or_insert(step.index);
            *entry = (*entry).max(step.index);
        }
    }
    let mut indexes: Vec<i32> = latest.into_values().collect();
    indexes.sort_unstable();
    indexes
}

/// The instruction a step with outcomes gets, so the agent knows how to report one.
pub fn outcome_directive(outcomes: &[String]) -> Option<String> {
    if outcomes.is_empty() {
        return None;
    }
    Some(format!(
        "When you are done, end your final answer with one line of exactly this form, choosing one value:\n\nOUTCOME: <{}>\n\nThe platform reads that line to decide which step runs next. Without it this step counts as failed.",
        outcomes.join(" | ")
    ))
}

/// Read the outcome a step reported: the last `OUTCOME: <label>` line of its answer.
/// `None` when there is no such line or the label is not one of `declared`.
pub fn parse_outcome(text: &str, declared: &[String]) -> Option<String> {
    const DECORATION: &[char] = &['*', '_', '`', '#', '>', ' ', '\t', '-'];
    for line in text.lines().rev() {
        let line = line.trim().trim_start_matches(DECORATION);
        let Some(head) = line.get(..8) else { continue };
        if !head.eq_ignore_ascii_case("outcome:") {
            continue;
        }
        let label = line[8..]
            .trim_matches(|c: char| {
                DECORATION.contains(&c) || matches!(c, '<' | '>' | '.' | '"' | '\'')
            })
            .to_ascii_lowercase();
        return declared.iter().find(|outcome| **outcome == label).cloned();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str) -> WorkflowNode {
        WorkflowNode {
            id: id.into(),
            name: id.to_uppercase(),
            agent_id: Uuid::nil(),
            instructions: String::new(),
            outcomes: Vec::new(),
            max_runs: None,
            model: None,
            position: None,
        }
    }

    fn edge(from: &str, to: &str, when: WorkflowWhen, outcome: Option<&str>) -> WorkflowEdge {
        WorkflowEdge {
            id: format!("{from}>{to}:{when:?}:{}", outcome.unwrap_or("")),
            from: from.into(),
            to: to.into(),
            when,
            outcome: outcome.map(str::to_string),
        }
    }

    fn ok(from: &str, to: &str) -> WorkflowEdge {
        edge(from, to, WorkflowWhen::Success, None)
    }

    /// Drives a run in memory: `results` says how each started step ends.
    struct Sim<'a> {
        workflow: &'a WorkflowDefinition,
        steps: Vec<WorkflowStepState>,
        log: Vec<String>,
    }

    impl<'a> Sim<'a> {
        fn new(workflow: &'a WorkflowDefinition) -> Self {
            assert_eq!(workflow.validate(), Vec::new());
            Self {
                workflow,
                steps: Vec::new(),
                log: Vec::new(),
            }
        }

        /// Plan once and create the steps it starts. Returns the verdict.
        fn tick(&mut self) -> WorkflowVerdict {
            let plan = plan(self.workflow, &self.steps);
            let mut started: Vec<String> = Vec::new();
            for activation in plan.start {
                started.push(activation.node_id.clone());
                self.steps.push(WorkflowStepState {
                    index: self.steps.len() as i32,
                    node_id: activation.node_id,
                    status: WorkflowStepStatus::Running,
                    outcome: None,
                    activated_by: activation.tokens,
                });
            }
            if !started.is_empty() {
                self.log.push(started.join("+"));
            }
            plan.verdict
        }

        fn finish(&mut self, node: &str, status: WorkflowStepStatus, outcome: Option<&str>) {
            let step = self
                .steps
                .iter_mut()
                .find(|step| step.node_id == node && step.status == WorkflowStepStatus::Running)
                .unwrap_or_else(|| panic!("{node} is not running"));
            step.status = status;
            step.outcome = outcome.map(str::to_string);
        }

        fn succeed(&mut self, node: &str) {
            self.finish(node, WorkflowStepStatus::Succeeded, None);
        }
    }

    #[test]
    fn a_line_of_steps_runs_in_order() {
        let workflow = WorkflowDefinition {
            nodes: vec![node("a"), node("b"), node("c")],
            edges: vec![ok("start", "a"), ok("a", "b"), ok("b", "c")],
        };
        let mut sim = Sim::new(&workflow);
        assert_eq!(sim.tick(), WorkflowVerdict::Running);
        sim.succeed("a");
        sim.tick();
        sim.succeed("b");
        sim.tick();
        // Planning again while c runs starts nothing new.
        assert_eq!(sim.tick(), WorkflowVerdict::Running);
        sim.succeed("c");
        assert_eq!(sim.tick(), WorkflowVerdict::Succeeded);
        assert_eq!(sim.log, ["a", "b", "c"]);
        assert_eq!(final_steps(&workflow, &sim.steps), [2]);
    }

    #[test]
    fn parallel_branches_run_together_and_the_join_waits_for_both() {
        let workflow = WorkflowDefinition {
            nodes: vec![node("plan"), node("web"), node("api"), node("review")],
            edges: vec![
                ok("start", "plan"),
                ok("plan", "web"),
                ok("plan", "api"),
                ok("web", "review"),
                ok("api", "review"),
            ],
        };
        let mut sim = Sim::new(&workflow);
        sim.tick();
        sim.succeed("plan");
        sim.tick();
        sim.succeed("web");
        // api is still busy and can deliver to review, so review waits.
        assert_eq!(sim.tick(), WorkflowVerdict::Running);
        assert_eq!(sim.log, ["plan", "api+web"]);
        sim.succeed("api");
        sim.tick();
        assert_eq!(sim.log, ["plan", "api+web", "review"]);
        let review = sim.steps.last().unwrap();
        assert_eq!(review.activated_by.len(), 2);
        sim.succeed("review");
        assert_eq!(sim.tick(), WorkflowVerdict::Succeeded);
    }

    #[test]
    fn an_outcome_chooses_the_branch_and_the_join_does_not_wait_for_the_other() {
        let mut triage = node("triage");
        triage.outcomes = vec!["bug".into(), "feature".into()];
        let workflow = WorkflowDefinition {
            nodes: vec![triage, node("fix"), node("build"), node("report")],
            edges: vec![
                ok("start", "triage"),
                edge("triage", "fix", WorkflowWhen::Outcome, Some("bug")),
                edge("triage", "build", WorkflowWhen::Outcome, Some("feature")),
                ok("fix", "report"),
                ok("build", "report"),
            ],
        };
        let mut sim = Sim::new(&workflow);
        sim.tick();
        sim.finish("triage", WorkflowStepStatus::Succeeded, Some("bug"));
        sim.tick();
        sim.succeed("fix");
        sim.tick();
        sim.succeed("report");
        assert_eq!(sim.tick(), WorkflowVerdict::Succeeded);
        assert_eq!(sim.log, ["triage", "fix", "report"]);
    }

    fn test_fix_loop(max_runs: Option<u32>) -> WorkflowDefinition {
        let mut test = node("test");
        test.outcomes = vec!["pass".into(), "fail".into()];
        test.max_runs = max_runs;
        WorkflowDefinition {
            nodes: vec![node("build"), test, node("fix"), node("deploy")],
            edges: vec![
                ok("start", "build"),
                ok("build", "test"),
                edge("test", "deploy", WorkflowWhen::Outcome, Some("pass")),
                edge("test", "fix", WorkflowWhen::Outcome, Some("fail")),
                ok("fix", "test"),
            ],
        }
    }

    #[test]
    fn a_loop_repeats_until_the_outcome_changes() {
        let workflow = test_fix_loop(None);
        let mut sim = Sim::new(&workflow);
        sim.tick();
        sim.succeed("build");
        sim.tick();
        sim.finish("test", WorkflowStepStatus::Succeeded, Some("fail"));
        sim.tick();
        sim.succeed("fix");
        sim.tick();
        sim.finish("test", WorkflowStepStatus::Succeeded, Some("pass"));
        sim.tick();
        sim.succeed("deploy");
        assert_eq!(sim.tick(), WorkflowVerdict::Succeeded);
        assert_eq!(sim.log, ["build", "test", "fix", "test", "deploy"]);
        assert_eq!(final_steps(&workflow, &sim.steps), [4]);
    }

    #[test]
    fn a_loop_stops_at_the_step_limit() {
        let workflow = test_fix_loop(Some(2));
        let mut sim = Sim::new(&workflow);
        sim.tick();
        sim.succeed("build");
        for _ in 0..2 {
            sim.tick();
            sim.finish("test", WorkflowStepStatus::Succeeded, Some("fail"));
            sim.tick();
            sim.succeed("fix");
        }
        assert_eq!(
            sim.tick(),
            WorkflowVerdict::Failed(WorkflowFailure {
                message: "step 'TEST' was due to run again but reached its limit of 2 runs".into(),
                step: None,
            })
        );
    }

    #[test]
    fn an_unhandled_failure_fails_the_run_and_a_handled_one_does_not() {
        let unhandled = WorkflowDefinition {
            nodes: vec![node("a"), node("b")],
            edges: vec![ok("start", "a"), ok("a", "b")],
        };
        let mut sim = Sim::new(&unhandled);
        sim.tick();
        sim.finish("a", WorkflowStepStatus::Failed, None);
        assert_eq!(
            sim.tick(),
            WorkflowVerdict::Failed(WorkflowFailure {
                message: "step 'A' failed".into(),
                step: Some(0),
            })
        );

        let handled = WorkflowDefinition {
            nodes: vec![node("a"), node("b"), node("cleanup")],
            edges: vec![
                ok("start", "a"),
                ok("a", "b"),
                edge("a", "cleanup", WorkflowWhen::Failure, None),
            ],
        };
        let mut sim = Sim::new(&handled);
        sim.tick();
        sim.finish("a", WorkflowStepStatus::Failed, None);
        sim.tick();
        sim.succeed("cleanup");
        assert_eq!(sim.tick(), WorkflowVerdict::Succeeded);
        assert_eq!(sim.log, ["a", "cleanup"]);
    }

    #[test]
    fn a_step_that_starts_the_run_and_closes_a_loop_does_not_wait_for_itself() {
        // start -> a -> b -> a (always). a must start although b can reach it.
        let mut a = node("a");
        a.max_runs = Some(2);
        let workflow = WorkflowDefinition {
            nodes: vec![a, node("b")],
            edges: vec![ok("start", "a"), ok("a", "b"), ok("b", "a")],
        };
        let mut sim = Sim::new(&workflow);
        sim.tick();
        sim.succeed("a");
        sim.tick();
        sim.succeed("b");
        sim.tick();
        sim.succeed("a");
        sim.tick();
        sim.succeed("b");
        assert!(matches!(sim.tick(), WorkflowVerdict::Failed(_)));
        assert_eq!(sim.log, ["a", "b", "a", "b"]);
    }

    #[test]
    fn two_starting_steps_on_one_loop_do_not_deadlock() {
        let workflow = WorkflowDefinition {
            nodes: vec![node("a"), node("b")],
            edges: vec![
                ok("start", "a"),
                ok("start", "b"),
                edge("a", "b", WorkflowWhen::Failure, None),
                edge("b", "a", WorkflowWhen::Failure, None),
            ],
        };
        let mut sim = Sim::new(&workflow);
        sim.tick();
        assert_eq!(sim.log, ["a+b"]);
        sim.succeed("a");
        sim.succeed("b");
        assert_eq!(sim.tick(), WorkflowVerdict::Succeeded);
    }

    #[test]
    fn planning_is_repeatable() {
        let workflow = test_fix_loop(None);
        let first = plan(&workflow, &[]);
        assert_eq!(first, plan(&workflow, &[]));
        assert_eq!(first.start.len(), 1);
        assert_eq!(first.start[0].attempt, 1);
    }

    #[test]
    fn validation_reports_what_is_wrong() {
        let messages = |workflow: &WorkflowDefinition| -> Vec<String> {
            workflow
                .validate()
                .into_iter()
                .map(|issue| issue.message)
                .collect()
        };
        assert!(messages(&WorkflowDefinition::default())[0].contains("at least one step"));

        let no_start = WorkflowDefinition {
            nodes: vec![node("a")],
            edges: vec![],
        };
        assert!(messages(&no_start)[0].contains("starting step"));

        let orphan = WorkflowDefinition {
            nodes: vec![node("a"), node("b")],
            edges: vec![ok("start", "a")],
        };
        assert!(messages(&orphan)[0].contains("can never run"));

        let bad_outcome = WorkflowDefinition {
            nodes: vec![node("a"), node("b")],
            edges: vec![
                ok("start", "a"),
                edge("a", "b", WorkflowWhen::Outcome, Some("pass")),
            ],
        };
        assert!(messages(&bad_outcome)[0].contains("no outcome 'pass'"));

        let mut reserved = node("start");
        reserved.name = "Start".into();
        let reserved = WorkflowDefinition {
            nodes: vec![reserved],
            edges: vec![],
        };
        assert!(messages(&reserved)[0].contains("reserved"));

        let unknown = WorkflowDefinition {
            nodes: vec![node("a")],
            edges: vec![ok("start", "a"), ok("a", "ghost")],
        };
        assert!(messages(&unknown)[0].contains("unknown step 'ghost'"));
    }

    #[test]
    fn agents_are_listed_once() {
        let mut second = node("b");
        second.agent_id = Uuid::from_u128(7);
        let workflow = WorkflowDefinition {
            nodes: vec![node("a"), second, node("c")],
            edges: vec![],
        };
        assert_eq!(workflow.agent_ids(), [Uuid::nil(), Uuid::from_u128(7)]);
    }

    #[test]
    fn outcomes_are_read_from_the_last_marker_line() {
        let declared = vec!["pass".to_string(), "fail".to_string()];
        assert_eq!(
            parse_outcome("All green.\n\nOUTCOME: pass", &declared).as_deref(),
            Some("pass")
        );
        assert_eq!(
            parse_outcome("OUTCOME: pass\nlater\n**Outcome: FAIL**\n", &declared).as_deref(),
            Some("fail")
        );
        assert_eq!(
            parse_outcome("- outcome: `pass`.", &declared).as_deref(),
            Some("pass")
        );
        assert_eq!(parse_outcome("OUTCOME: maybe", &declared), None);
        assert_eq!(parse_outcome("The outcome: pass", &declared), None);
        assert_eq!(parse_outcome("", &declared), None);
        assert!(outcome_directive(&declared)
            .unwrap()
            .contains("pass | fail"));
        assert_eq!(outcome_directive(&[]), None);
    }
}
