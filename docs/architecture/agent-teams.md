# Agent Teams and connector bindings

## Model

```text
Connection ──credential + config──► Agent connector binding ──capabilities──► Execution
                                           │
Agent Team ──ordered agents───────────────┘
     │
     └── TeamRun → one normal execution per agent, sequential
```

- **Connection**: one external account/credential, e.g. a GitHub token. It can be reused
  by many agents.
- **Agent connector binding**: the connector, `connectionId` and capability allowlist
  stored on the agent version. This is where a connection is attached.
- **Agent Team**: orchestration. Each step inherits the connections and capabilities of
  its agent. The team can additionally share connections with every step through
  `connectionIds`, and its work source connection is added when the work source is
  enabled (see "Team definition fields").
- **Team skills and instruction**: shared by every step. Agent-level instructions define
  the role. The first agent is the primary agent; later agents are subagents.
- **Team repository**: optional `repository` and `branch` are structured checkout
  settings. They are passed to the worker's `repo.checkout`; instructions alone do not
  drive checkout. A run can override the team repository. Teams that only use GitHub
  API capabilities leave this empty; checkout is skipped and issue list/comment/update
  tools work without a local clone.
- **Execution mode**:
  - `pipeline`: one ORBIT execution per agent, sequential.
  - `native`: one OpenCode execution; later team agents are materialized as
    `.opencode/agents/*.md` subagents and the primary delegates to them. This supports
    OpenCode's internal feedback loop and shared workspace. Subagent dispatches are not
    separate ORBIT executions.
  - `workflow`: a graph of steps and conditional lines; one ORBIT execution per step
    run, with parallel branches, joins and bounded loops. The rules are in
    `crates/orbit-domain/src/workflow.rs`, the run-time part in
    `crates/orbit-api/src/team_workflow.rs`; see
    [decision 0015](../decisions/0015-team-workflows.md).
- **Per-agent models**: each step already uses its own agent's model. A team-level
  `model` override applies to all steps, and a step-level `model` override wins over it:
  `{ "provider": "...", "model": "...", "management": "platform|harness" }`.
- **Multi-repository workspace**: `repositories[]` entries
  (`connectionId`, `repository`, `path`, `primary`, `branch`) are checked out
  sequentially into the execution workspace. A single repository keeps the current root
  layout; multiple repositories clone into their declared `path` subdirectories. Paths
  are validated so nothing can escape the workspace. Git tools accept a `repository`
  argument (`repo.commit`, `repo.push`, `pulls.create`, `merge_requests.create`).

## Team definition fields

The team `definition` (`TeamDefinition` in `crates/orbit-api/src/handlers/teams.rs`):

| Field | Meaning |
|---|---|
| `mode` | `pipeline` (default), `native` or `workflow` |
| `workflow` | for `workflow` mode: `nodes` (id, name, agentId, instructions, outcomes, maxRuns, model, position) and `edges` (id, from, to, when, outcome); `steps` is then derived |
| `instructions` | team goal, configured system context for every step |
| `steps` | ordered agents, each with an optional `model` override |
| `skills` | skill slugs materialized for every step |
| `harness` | harness for the steps; a run may override it with `harnessOverride`, and the fallbacks are the agent's legacy harness, then the organization default |
| `model` | team-level model override |
| `connectionIds` | connections injected into every step in addition to the agents' own bindings |
| `envMappingIds` | environment mappings injected into every step |
| `repository`, `branch`, `repositories[]` | checkout settings (see Model) |
| `workSource` | automatic issue discovery (see Work source and queue) |
| `workerProfileId`, `autoLaunch` | when `autoLaunch` is true, the API launches a worker from the profile for each step's execution |
| `timeoutSeconds` | wall-clock timeout per step, 1 to 86400, default 3600 |

## Attaching a connection

1. Create the credential under **Connections**.
2. Open each **Agent** in Agent Studio.
3. Enable the connector and select the same connection.
4. Select only the capabilities that agent needs.

Example for a GitHub coding team using one connection:

| Agent | Capabilities |
|---|---|
| Primary coder | `issues.list`, `issues.get`, `issues.update`, `repo.checkout` |
| Builder | `repo.checkout`, `repo.commit`, `repo.push`, `pulls.create` |
| Tester | `repo.checkout`, `checks.list` |
| Reviewer | `checks.list`, `pulls.get`, `pulls.list`, `issues.comment` |

The GitHub connection should set `defaultRepository` (and optionally
`allowedRepositories`). `repo.checkout` clones that repository before the harness starts.

## Minimal GitHub issue flow

1. Create a GitHub connection with a PAT that can read issues and push branches.
2. Create two agents:
   - **Primary / issue triage**: GitHub capabilities `issues.list`, `issues.get`,
     `issues.update`.
   - **Builder**: `repo.checkout`, `repo.commit`, `repo.push`, `pulls.create`.
3. Create a skill such as `github-issue-loop` that tells the agent:
   - list issues with label `agent-ready`,
   - choose one,
   - comment/assign/label it,
   - work only on that issue,
   - report the issue number and plan.
4. Create an **Agent Team** with the primary agent first and the builder second.
   Add the skill at team level.
5. Team instruction example:

   ```text
   Pick one open issue labelled agent-ready and fix it. Report the issue number,
   changed files, tests run and the branch. Do not pick another issue in this run.
   ```

6. Run the team. The primary lists/selects the issue; the builder receives the previous
   output, clones the repository, makes changes, commits and pushes.
7. Approve any write action according to the team's fixed autonomy policy, or the step
   agents' own settings when the team follows them.

## Native OpenCode team

A native team is created with `mode: "native"`. The primary agent is the first agent;
the remaining agents become OpenCode subagents. ORBIT materializes their instructions,
model and description into the execution workspace. The primary is instructed to use the
task/subagent tool.

The example loop skill lives at
`docs/examples/skills/github-issue-loop/SKILL.md`. It covers issue selection, builder
feedback, tests, commit/push, PR creation, checks and issue labels.

Native mode currently requires an OpenCode primary agent. Subagent tool calls are
visible as one ORBIT execution; connector policy is enforced against the primary agent's
binding for the whole native team.

Pi does **not** have an equivalent primary/subagent configuration in the tested 1.5.2
release, so native Agent Teams are rejected for Pi. Pi can still use **pipeline** mode:
each agent is a separate ORBIT execution, with explicit output handoff but without a
shared workspace or an internal feedback loop.

## Work source and queue

A team can discover its own work. Set `workSource` on the team definition:

```json
{
  "provider": "github",
  "enabled": true,
  "connectionId": "…",
  "repository": "owner/repo",
  "labels": ["agent-ready"],
  "claimLabel": "agent-in-progress",
  "doneLabel": "agent-done",
  "failedLabel": "agent-failed",
  "maxItems": 5,
  "maxAttempts": 1,
  "pollSeconds": 60,
  "autoRun": true
}
```

GitLab uses the same shape with `project`/`groupId` (and optional `boardId`) instead of
`repository`.

The API dispatcher (`dispatcher.rs`) runs every 15s:

1. Takes a per-team Postgres advisory lock (replicas never double-dispatch).
2. Lists matching open issues, skipping keys already in the queue.
3. Claims each issue by swapping the ready label for the claim label.
4. Inserts a `team_queue_item` (unique per `team_id, external_key`).
5. If `autoRun`, starts pending items via the same queue-item runner the UI uses.

Claim failures skip the item without enqueueing. `lastPollAt` / `lastPollError` are
visible in the UI. An issue whose repository or project is outside the connection's
`allowedRepositories` / `allowedProjects` is not claimed; the poll error names it.

### Repository scope of queue items

A queue run uses the team's connection credentials, so an item may only target the
team's own repositories, the same rule as `POST …/runs` (#384):

- `POST …/queue` refuses (422) an item whose `repository`, `payload.repository` or
  `payload.repositories[].repository` is not the team's `repository`, one of its
  `repositories[]`, or the work source's `repository` / `project`. For a GitLab work
  source with a `groupId` path, projects under that group path are accepted too.
- Starting an item checks the same rule again, so an item stored with a foreign
  repository is refused and marked `failed` before any run is created. For a GitLab
  group given by numeric id, items already in the queue (which the dispatcher found in
  that group) are accepted; new items from callers are not.
- The outcome writer below only reads and updates issues in the work source's own
  repository or project (or a project of its group), and only when the connection's
  `allowedRepositories` / `allowedProjects` (if set) includes it. Otherwise it skips the
  side effects and closes the item.

## Closing the loop

When a team run terminates (`teams::advance`), the outcome is mirrored to the source
issue through the connector:

| Outcome | Labels | Comment |
|---|---|---|
| succeeded | claim label removed, `doneLabel` added | Posted, including any PR/MR URLs found in the output |
| failed (attempts left) | claim label removed, ready labels restored | — |
| failed (exhausted) | claim label removed, `failedLabel` added | — |
| cancelled | claim label removed | — |

The queue item records `succeeded`, `failed` or `cancelled` accordingly, and failures
respect `maxAttempts`. Operators can retry, requeue or cancel an item from the UI or
`POST …/queue/{item_id}/{retry|requeue|cancel}`:

| Action | Allowed from | Effect |
|---|---|---|
| `retry` | `failed`, `cancelled`, and only while `attempts < maxAttempts` | back to `pending`, consumes an attempt |
| `requeue` | `pending`, `failed`, `cancelled` | back to `pending`, no attempt consumed; never revives `succeeded` or `running` items, and refused (409) while the previous run's execution is still stopping |
| `cancel` | `pending`, `running`, `failed` | `cancelled`; the item's run is cancelled and its live execution is asked to stop (a queued one is cancelled outright) |

Item status is limited to `pending`, `running`, `succeeded`, `failed` and `cancelled`
(a CHECK constraint since migration 0041).

## Run advancement and recovery

Every API replica runs the team-run loop every 2 seconds. These rules keep it safe and
bounded:

- **Advance lease.** A replica must hold a per-run lease (`team_runs.advance_claimed_at`)
  to advance a run. The lease expires after 120 seconds, so a crashed replica never
  strands a run, and a holder whose lease was taken over cannot release the new holder's
  lease. A run whose current step is still working is skipped with two reads, without
  taking the lease.
- **Step-start retry window.** When the next step cannot be started because of a server
  error, the finished step stays `succeeded` and the next tick retries. After 300 seconds
  since the finished step the run fails with "could not start step N". A step held back
  by its agent's `maxExecutionsPerHour` (429) or the licence's daily execution limit (402)
  waits the same way for up to 65 minutes, because those limits clear by themselves. Any
  other client-class refusal (access lost, harness unavailable) fails the run immediately.
- **Model overrides.** A step's model override, else the team's, is recorded with the step
  execution and applied when the worker fetches its context, so the step runs (and the
  provider access check covers) the overridden model. Native-team subagents get their
  step's override the same way.
- **Stale queue claims.** A queue item that is `running` but has no run attached for more
  than 600 seconds (for example the API died between claiming and creating the run)
  returns to `pending` with the error "start interrupted; re-queued". The same loop also
  re-applies run outcomes to queue items that were never closed.
- **Rerun.** `POST /api/v1/executions/{id}/rerun` replays one terminal execution. It
  returns 409 for a team step (an execution with a team run or team context), because
  replaying it would hand the team's connections, mappings and subagents to anyone who
  can view the execution. There is no team-run rerun endpoint; start a new run with
  `POST /api/v1/agent-teams/{id}/runs`.

## Current limitations

- Connections are attached per **agent**; `connectionIds` on the team adds shared ones.
- Pipeline steps are sequential and native subagents are not parallel ORBIT executions;
  use `workflow` mode for parallel steps. Parallel workflow steps do not share a workspace.
- Inbound webhook verification is still pending.
- GitHub/GitLab writes require a token and may require approvals depending on autonomy.
