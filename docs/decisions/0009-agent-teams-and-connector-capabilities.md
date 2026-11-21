# ADR-0009: Agent teams, execution-level skills and connector capabilities

**Status:** Accepted and implemented; see the amendment at the end for where the code differs.

## Context

Coding workflows need more than one role: architect, builder, tester and reviewer. The
first implementation put agents and skills on the worker profile, which conflated
execution policy with infrastructure. Connector capabilities were also limited to GitHub
issues, so a coding agent could not complete a repository loop.

## Decision

### Execution-level agents and skills

- An execution always selects exactly one agent version.
- Skills are resolved at execution time from:
  1. the agent spec,
  2. explicit execution skills,
  3. optional worker-profile defaults (legacy compatibility).
- The worker profile is infrastructure only: harness, image, target, concurrency and auth
  mounts. New profiles should not require agent allowlists or shared skills.
- Agent definitions are never copied into a worker at launch. The worker fetches the
  agent spec, skills and connector grants through the token API per execution.

### Agent teams

- An `AgentTeam` is an ordered list of normal agents. The first agent is the primary
  agent; the agent's own name/instructions define its role.
- Skills belong to the team, not to individual steps, and are materialized for every
  step. There are no separate role records.
- A team has an `instructions` goal. It is configured system context for every step,
  never part of the explicit run instruction (task). The task is the single run
  instruction; later steps receive the previous step output as delegated context in the
  payload, not in the task. Native-team subagent instructions are their own configuration
  and never contain the primary agent's instructions unless a delegation explicitly
  includes them.
- Team definitions are editable; a run snapshots its definition so edits affect future
  runs only.
- A `TeamRun` creates one normal execution per step, sequentially. Executions carry
  `teamRunId`/`teamStepIndex` for grouped visibility.
- OpenCode's native primary/subagent feature is supported as native team mode. Pi has
  no equivalent verified feature; Pi teams use pipeline mode with separate executions.
  Pipeline execution gives each step its own policy, approvals, events and audit.
- `orbit-api` advances team runs as a coordinator. It does not reason or call model APIs;
  it only creates executions, records state and publishes events. Agent loops remain in
  the execution plane.
- Parallel steps, conditions, retries and artifact contracts are future extensions.

### Connector capabilities

- A connection holds a credential and configuration. It is reusable across all
  capabilities the credential allows.
- The connector manifest is the platform capability contract. Adding a capability means:
  manifest descriptor, tool schema/effect, invocation implementation and policy tests.
- An agent's connector binding selects the capability subset. The token/scope on the
  external credential is the hard external limit; ORBIT's binding is the platform limit.
- Agent Teams do not own connections; each step uses its agent's binding. See
  `docs/architecture/agent-teams.md` for the operator flow.
- Connection `scopes` remain informational. Enforcement uses the agent binding,
  autonomy level and approvals.
- GitHub is the reference connector. Its coding loop adds:
  - issues: list/get/comment/create/update (labels, state, assignees)
  - pulls: list/get/create
  - checks: list
  - repository: checkout before the harness, commit and push through the tool broker
- GitLab follows the same pattern and now covers projects, group issues/boards, project
  issues, comments/updates, merge requests, pipelines and repository checkout/commit/
  push. Group boards may span projects; a run still has one checked-out repository, so
  multi-project board items need repository selection per item.

### Worker provisioning

- Long-lived worker pools remain supported.
- The launcher can start ephemeral workers. The target model is queue/workflow-driven
  launch with single-execution tokens, not manual bootstrap.
- Manual Launch remains for debugging, warm pools and dedicated workers.

## Consequences

- Team runs are visible as normal executions, with normal events, approvals and audit.
- Skills must be materialized from all three sources for Pi/OpenCode workspaces.
- GitHub repo operations use short-lived git child processes with askpass credentials;
  credentials are not written into remote URLs.
- Team execution requires the selected worker image to provide the harness and `git`
  where repository capabilities are used.

## Amendment (2026-11-21)
- A team definition can also carry `connectionIds`, `envMappingIds`, `harness` and
  `timeoutSeconds`, and an enabled work source adds its own connection to every step. The
  statement "Agent Teams do not own connections" therefore holds only for the agents'
  bindings; see [Agent Teams](../architecture/agent-teams.md).
- Retries exist for work-queue items (`maxAttempts`, retry and requeue actions), not for
  individual steps. Parallel steps and conditions are still not implemented.
- The Kubernetes connector (read-only) was added after this decision.
