# Agent Teams

An Agent Team coordinates several agents for one outcome. Teams have three modes:

| Mode | How it runs |
|---|---|
| `pipeline` (default) | Every agent is a separate ORBIT execution, chained in order |
| `native` | The primary agent runs once and the remaining agents become harness-native subagents |
| `workflow` | Steps joined by lines: branch on a result, run steps side by side, loop until the work is right. See [Workflows](#workflows) |

## Definition

- An ordered list of agents. The first agent is the primary agent; in the run page it is
  marked "Primary". Each step can have an optional model override, and the team can have
  a team-level model override. A step has no instructions of its own: each agent's own
  instructions define its role, and the team's shared instructions go to every step.
- Shared instructions (the goal), skills, connections and environment mappings
- `harness` for the team's steps, `timeoutSeconds` per step (one hour when empty), and
  an optional `workerProfileId` with `autoLaunch` for automatic worker launch
- Optional **work source**: GitHub or GitLab provider, connection, labels, claim, done and
  failed labels, `maxItems` (1-100), `maxAttempts` (1-10), `pollSeconds` (60 by default,
  at least 5) and `autoRun`. GitHub needs `repository` (owner/repo); GitLab needs a
  `project` or a `groupId`, and can use a `boardId`.

`native` mode requires the primary agent to use the OpenCode harness; use `pipeline` mode
for Pi and other harnesses. The API and Team Studio both refuse a native team whose
primary agent is not OpenCode.

## Workflows

A workflow is a team whose agents are arranged as a graph instead of a line. Choose
**Workflow** in Team Studio's Agents section.

- A **step** runs one agent as a normal execution. It has a name, an agent, optional
  instructions for this step only, and optional **outcomes**. The same agent can be used
  in several steps.
- A **line** connects two steps and says when the work is handed on: when the step
  **succeeds**, when it **fails**, **always**, or when it reports a certain **outcome**.
- A line from **Start** marks a step that begins the run. Several steps can.

| You want | Draw it like this |
|---|---|
| One step after another | A line "when it succeeds" from each step to the next |
| A branch | Give the step outcomes (`pass`, `fail`) and draw one line per outcome |
| Parallel work | Two lines from one step that both match; both targets start together |
| A join | Draw lines from each branch to the same step; it waits until no earlier step is still working, then receives all their results |
| A loop | Draw a line back to an earlier step, for example `fix` → `test` |
| A fallback | A line "when it fails" to a step that cleans up or reports |

**Outcomes.** An agent that runs a failing test suite still finishes its run
successfully, so "succeeded" cannot mean "the tests passed". A step with outcomes is
told to end its answer with a line `OUTCOME: <label>`, and ORBIT reads that line to
choose the next step. If the agent does not report one of the listed outcomes, the step
counts as failed.

**Limits.** A step on a loop runs at most 3 times in one run unless you set another
limit (1 to 10) on the step. When a step would run more often, the run fails and says
which step reached its limit; the run page marks that step on the graph. A run creates
at most 100 steps, and a workflow has at most 30 steps.

**When a run ends.** It succeeds when nothing is running and nothing is waiting; its
result is the output of the step or steps that finished last. It fails when a step fails
and no line "when it fails" or "always" leaves that step; steps that are still working
are then stopped. A step whose worker was lost (stopped by the runtime, or no longer
reporting) always fails the run, even with a line "when it fails": the lost worker may
have changed things, so nothing runs after it automatically.

**What a step receives.** The run's task, the team goal, its own instructions, and the
output of the step that handed over. After a join it receives the outputs of all of
them, by step name.

Parallel steps are separate executions on separate workers, so they do not share files.
Let them push to branches, and let the joining step combine the branches.

In the editor, the **Canvas** shows the graph: drag a step to move it, drag from the dot
on its right side to another step to connect them, select a step or a line to edit it,
and press Backspace to remove it. **Add step** continues from the selected step (or from
the end of the line) and follows the next outcome that has no line yet. Saving a
workflow with a problem opens the step that has it. **List** shows the same steps as a form, which is the
default on a phone. **Examples** inserts a ready-made workflow (review loop, parallel
work, test before deploy) to adapt. The run page draws the graph with the state of every
step and the lines the run followed; select a step there to find its row in the list.
When a run is cancelled, or fails while other branches are still working, those steps
are shown as stopped.

**Trying a workflow without a model.** Agents on the mock harness follow the outcome
instruction too: a step with outcomes reports the first one listed. To steer it, put
`[outcome:changes,approved]` in the run's task: the first label is reported the first
time a step runs, the second the next time, and so on. A task that contains `fail`
makes every mock step fail, and `slow` makes each take about 25 seconds.

In the API a workflow team is created with `mode: "workflow"` and a `workflow` object
with `nodes` and `edges`; `steps` can be left out. A run uses the graph it started with,
so editing the team does not change a run that is in flight.

## Queue and dispatcher

The API background dispatcher polls enabled work sources under a per-team advisory lock:

```text
list matching issues → skip items already queued → claim label → insert queue item
→ optionally start the run → release lock
```

Queue items are deduplicated by `(team_id, external_key)`, so two API replicas cannot
double-dispatch. Manual add, run one, retry, requeue and cancel are available in the UI and API
(`/api/v1/agent-teams/{id}/queue`); "run pending" in the UI starts the pending items one
after another. On success the issue gets a comment
with PR/MR links and the done label; failures apply the failed label or release the claim.

## Multi-repository workspace

A team can define `repositories[]` with `repository` (owner/repo), `connectionId`, `path`,
`primary` and `branch`.
Repositories are checked out sequentially (paths are validated against traversal), the
prompt gets a manifest, and git tools accept a `repository` argument. Multi-repo runs
operate on the worker workspace, not the control plane.

## Execution and limits

Team runs are executions (and count against the license's executions/day limit) and use
the same worker profiles, skills, tool policy and approvals as single agents.

## Ownership and visibility

Agent Teams are shareable resources (separate from access teams). They carry
`ownerUserId`, `ownerTeamId` and `visibility` (`private | team | org`). Team lists, team
runs and run detail are filtered by the owning team, and starting a run requires
`executions:create` and view access. Creating, updating and deleting a team requires the
`settings:write` permission (owners, or a custom role that has it) as well as manage access;
Admin and Operator roles do not have `settings:write` ([roles](rbac.md)).

## Team connections and environment mappings

A team can attach `connectionIds` and `envMappingIds` directly in the Agent Team editor.
Team connections are validated against visibility and merged into every step's grant set
(capabilities come from the connection scopes, or read-only manifest defaults). Team
environment mappings are resolved into the execution environment for every step. This
avoids duplicating the same GitLab/cloud integration on each step agent; credentials stay
in the broker and are never passed to the harness.

## Team autonomy

A team can either follow each step agent's own autonomy (`follow_primary`, the default)
or declare one fixed level for the whole run. The fixed policy applies to every pipeline
step and, for native teams, to the primary and all subagents.

Setting a fixed level above a step agent's own level is an explicit elevation: it requires
manage access to that agent and is written to the audit log as
`team.autonomy_elevated`. Lowering autonomy never needs extra access. The run stores a
snapshot of the policy, so editing an agent or the team later cannot change an in-flight
run.

## Auto-approve

Starting a run with `autoApprove: true` requires the `approvals:decide` permission;
without it the API returns 403. The checkbox is "Auto-approve approvals for this run" in
the "Run <team>" dialog. In [Start work](start-work.md) the "Auto-approve writes"
option is shown disabled. The API marks each step execution with a server-set
`approvalOverride: "auto"`. The worker applies it only to step agents whose own autonomy
level is Prepare (L3) or higher, and only by switching the approval mode to `Auto`. It
never raises the level: Observe (L1) and Recommend (L2) agents still deny every write. At
L3 `Auto` mode still requires approval for writes (only L4 runs them unattended), so in
practice the override lifts `Always` and `Writes` approval modes on L4 agents. A fixed
team autonomy is applied after this override and therefore wins. The legacy numeric
`autonomyOverride` is ignored.

## Reruns

Rerunning a single step of a team run (pipeline or workflow) is refused with 409 ("this execution is a team step
and cannot be rerun on its own"). The execution page shows "Open team run" instead of
"Run again". Start a new team run to repeat the work.

## Screens

- **Agent teams** lists teams with search, a status filter and bulk access, and has
  **Import** and **New agent team**. **Team runs** (`/agent-teams/runs`) lists runs; see
  [Executions](executions.md#team-runs).
- A team page shows Agents, Goal, Latest run, Repository and work, Worker and limits and
  skills shared with every agent, and an **In the queue** card.
- In the queue, **Add to queue** takes one item per line: `key` or `key|repository`.
  Items with the same key are added once. A pending item has **Run**; **Run N pending**
  starts all pending items. Failed or cancelled items offer **Retry** and **Requeue** in
  their "More actions" menu, and a running item offers **Cancel item**. **Open run** goes
  to the team run of an item.
- Team Studio (`/agent-teams/new`, `/agent-teams/<id>/edit`) asks before you leave with
  unsaved changes.
