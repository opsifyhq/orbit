# Agent Teams

An Agent Team coordinates several agents for one outcome. Teams have two modes:

| Mode | How it runs |
|---|---|
| `pipeline` (default) | Every agent is a separate ORBIT execution, chained in order |
| `native` | The primary agent runs once and the remaining agents become harness-native subagents |

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

## Auto-approve

Starting a run with `autoApprove: true` requires the `approvals:decide` permission;
without it the API returns 403. The checkbox is "Auto-approve writes for this run" in the
"Run <team>" dialog. In [Start work](start-work.md) the "Auto-approve writes"
option is shown disabled. The API marks each step execution with a server-set
`approvalOverride: "auto"`. The worker applies it only to step agents whose own autonomy
level is Prepare (L3) or higher, and only by switching the approval mode to `Auto`. It
never raises the level: Observe (L1) and Recommend (L2) agents still deny every write. At
L3 `Auto` mode still requires approval for writes (only L4 runs them unattended), so in
practice the override lifts `Always` and `Writes` approval modes on L4 agents. The legacy
numeric `autonomyOverride` is ignored.

## Reruns

Rerunning a single step of a team run is refused with 409 ("this execution is a team step
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
