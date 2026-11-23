# 0015 — Agent Team workflows: a graph run by the control plane, routed by declared outcomes

Date: 2026-11-23

## Context

Issue #9 asks for conditional branching, parallel work and retry loops inside an Agent
Team run, with a visual editor. Until now a team was either a `pipeline` (one execution
per agent, in order) or `native` (one OpenCode execution with subagents).

Facts that shaped the answer:

- A pipeline run is already a small state machine in `orbit-api`: a background loop
  takes a per-run lease, looks at the last step's execution and creates the next
  execution. Steps are ordinary executions, so workers, policy, approvals, licensing and
  history need no special case.
- `orbit-api` must not reason about the work (rule 2 in `AGENTS.md`). Whatever decides
  "tests passed, so deploy" cannot be a model call in the control plane.
- An execution that "succeeds" says nothing about what the agent found. An agent that
  runs a failing test suite still ends its run successfully.
- The web app had no canvas component and must stay usable on a phone (rule 8).

## Decision

- **A third team mode, `workflow`**, next to `pipeline` and `native`. The team
  definition gains `workflow: { nodes, edges }`. Existing teams are untouched; no data
  is migrated. For a workflow the API derives `steps` (each agent once) from the graph,
  so access checks, autonomy elevation, the queue and exports keep working unchanged.
- **Steps and lines.** A node (a *step*) runs one agent as a normal execution. An edge
  (a *line*) carries one condition: `always`, `success`, `failure` or `outcome` with a
  label. Lines that leave the reserved id `start` mark where a run begins. There are no
  other node types: no gateways, no timers, no script steps.
- **Outcomes are declared, reported and parsed; never inferred.** A step can list
  outcome labels such as `pass` and `fail`. The API adds an instruction to that step's
  system context: end the answer with `OUTCOME: <label>`. When the execution ends the
  API reads the last such line of the answer. A missing or unknown label makes the step
  *failed*, so a `failure` line (or the run failing) covers it. The control plane parses
  one line of text; it never judges the work.
- **One start rule instead of join types.** A finished step delivers on every matching
  line. A step with deliveries waiting starts when no step that is still running can
  reach it through the graph; it then takes everything that is waiting. This gives
  parallel branches (several lines match), joins that wait for every branch that is
  still alive, either/or branches that do not wait for the branch not taken, and loops,
  without the user choosing between "wait for all" and "wait for any".
- **Bounded loops.** Each step has `maxRuns` (default 3, at most 10) per run, and a run
  creates at most 100 steps. Reaching a limit fails the run with a message that names
  the step.
- **Failure handling.** A failed step with no matching line fails the run and the
  run's other executions are asked to stop. A `failure` or `always` line handles the
  failure and the run continues. A worker that the platform ended (lost heartbeat,
  eviction) still fails the whole run without a queue retry, as for pipelines.
- **State is the steps table.** `team_steps` gains `node_id`, `attempt`, `outcome` and
  `activated_by` (the deliveries a step took). The plan is a pure function of the
  definition snapshot on the run and those rows (`orbit_domain::workflow::plan`), so a
  crashed tick or a second replica simply plans again. A workflow step's execution is
  keyed `team:{run}:{step id}:{attempt}`, so a repeated start finds the same execution.
- **The rules live in `orbit-domain`**, without I/O, and are unit-tested there.
  `crates/orbit-api/src/team_workflow.rs` only records results and creates executions.
- **Editor: React Flow (`@xyflow/react`, MIT) for the canvas, plus a list view.** The
  canvas is the picture and takes drag gestures; a form panel edits the selected step or
  line; the list view offers the same fields without a canvas and is the default on
  phones. The library's attribution link stays visible.
- **Where it lives in the app.** Workflow is a choice in Team Studio's Agents section,
  not a new menu item: queues, work sources, runs, access and exports all belong to the
  team, and a second place to define "several agents working together" would split them.

## Consequences

- Pipelines and native teams run exactly as before; their code path is unchanged apart
  from sharing `start_step_with`.
- A workflow's parallel steps are separate executions on separate workers. They do not
  share a workspace; they meet again through their outputs and through the repository.
- Routing depends on agents following the outcome instruction. The step fails visibly
  when they do not, and the run page shows the reported outcome per step.
- Not included: waiting for a human at a step (approvals stay per tool call), timers,
  sub-workflows, and editing a workflow while a run is in flight (a run uses the graph it
  started with).
- The web bundle gains React Flow on the team edit, team and team-run pages.
