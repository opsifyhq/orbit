# Domain Model

## Aggregate relationships

```
organization 1─* user
organization 1─* agent 1─* agent_version
agent        1─* agent_instance *─1 harness       (legacy, see AgentInstance)
agent        0..1 agent_runtime                   (continuous runtime, see AgentRuntime)
agent        1─* trigger
agent        1─* execution *─1 agent_version
execution    1─* execution_event
execution    1─* approval
organization 1─* connection *─1 connector
organization 1─* provider
organization 1─* policy                           (legacy, see Connector section)
organization 1─* tool_policy
organization 1─* custom_role
organization 1─* access_team 1─* team_member *─1 user
organization 1─* output 1─* output_delivery
organization 1─* env_mapping
organization 1─* audit_log
organization 1─* agent_team 1─* team_run 1─* team_step *─1 execution
agent_team   1─* team_queue_item *─1 team_run
organization 1─* skill
organization 1─* worker_profile 1─* worker_launch
organization 1─* worker_token
organization 1─* api_token
organization 1─* secret
agent        1─* agent_memory
```

## Tables

Every table is scoped by `org_id` unless it is global infrastructure.

| Table | Entity |
|---|---|
| `organizations`, `users` | tenant and members |
| `agents`, `agent_versions`, `harnesses` | agent definitions, immutable versions, harness registry |
| `agent_runtimes` | continuous "keep running" runtime of an agent (one live row per agent) |
| `agent_instances` | **legacy**: read through `GET /api/v1/agent-instances`, but no code inserts rows |
| `providers`, `connectors`, `connections`, `secrets` | model providers, connector manifests, authorized accounts, encrypted secrets |
| `executions`, `execution_events`, `approvals` | runs, append-only events, human decisions |
| `triggers` | manual and schedule triggers (webhook and event kinds are stored, never fire) |
| `tool_policies` | per-call allow, require-approval and deny rules for tools |
| `policies` | **legacy**: created in migration 0001; no store or handler code reads or writes it |
| `processed_events` | **unused**: created in migration 0001, nothing writes to it |
| `agent_teams`, `team_runs`, `team_steps`, `team_queue_items` | Agent Teams, runs, steps, durable work queue |
| `agent_memory` | short-term/summary agent memory |
| `skills` | reusable SKILL.md packages |
| `worker_profiles`, `worker_tokens`, `worker_launches`, `worker_heartbeats` | execution-plane configuration, credentials, launches, liveness |
| `env_mappings`, `worker_profile_env_mappings` | connection or secret to environment-variable mappings and their use by profiles |
| `outputs`, `output_deliveries` | result destinations (webhook, Slack, Mattermost, Telegram) and their delivery attempts |
| `custom_roles` | organization-defined permission sets stored by name in `users.role` |
| `access_teams`, `team_members` | access teams (ownership and visibility groups), not Agent Teams |
| `api_tokens`, `audit_logs`, `platform_settings` | user tokens, audit trail, platform configuration |

Shareable resources (agents, skills, connections, providers, worker profiles, triggers
and tool policies) also carry `owner_user_id`, `owner_team_id` and
`visibility` (`private | team | org`, default `org`). See
[ADR 0011](../decisions/0011-multi-team-access-model.md).

Migrations live in `crates/orbit-store/migrations` and are append-only.

## Entities

### Organization
Tenant boundary. `mode = personal | enterprise`. A personal organization is created
automatically for each user in self-hosted deployments. All queries are scoped by
`org_id`; enterprise RBAC builds on `users.role` and `policies`.

### User
`role` is one of the built-in roles `owner`, `admin`, `operator`, `viewer`, or the name
of a custom role of the same organization (migration 0021 removed the CHECK constraint;
the API validates the value). Links to an external OIDC identity via
`(issuer, subject)`; never stores passwords.

### Agent (persistent definition)
| Field | Notes |
|---|---|
| `id`, `org_id`, `slug`, `name`, `description` | `slug` unique per org |
| `status` | `draft → active ⇄ paused → archived` |
| `current_version` | integer, points at an immutable `agent_versions` row |
| `labels` | JSONB for filtering |

A configuration change creates a new immutable version. Executions always reference the
exact `agent_version_id` they ran with. `owner_user_id`, `owner_team_id` and `visibility`
control who can see and use the agent.

### AgentVersion
`spec` JSONB is a validated `AgentSpec` (`apiVersion: orbit.io/v1`, `kind: Agent`).
`checksum` is the SHA-256 of canonical JSON. Only a spec identical to the *current*
version is refused (the API treats it as a metadata-only edit). Saving a spec equal to an
older version appends it as the next version with the same checksum, so history stays
append-only and shows the return (migration 0049 dropped `UNIQUE (agent_id, checksum)`).
The `definition_yaml` column retains the original YAML for export.

### AgentInstance (legacy)
The `agent_instances` table and the `GET /api/v1/agent-instances` endpoint exist
(`agent_version_id`, `harness_id`, `state`, `harness_instance_ref`, `health`, `config`),
but no code creates instances, so the list is always empty. The `create_instance`
family of methods on `HarnessAdapter` is likewise not used by the platform. Continuous
operation is modelled by `AgentRuntime`.

### AgentRuntime
The continuous "keep running" mode of an agent, controlled with
`GET/POST /api/v1/agents/{id}/runtime` and `POST …/runtime/stop`. It is not a harness
process: it is a row that makes the API start one normal execution after another.

| Field | Notes |
|---|---|
| `status` | `running`, `stopping`, `stopped`, `failed` (the code stops runtimes as `stopped` and never sets `failed`) |
| `task`, `harness_kind` | the task text and harness used for every run |
| `interval_seconds` | delay between runs, default 60, minimum 5 |
| `next_run_at`, `last_execution_id`, `last_heartbeat_at` | scheduling and liveness |
| `consecutive_failures`, `failed_execution_id`, `last_error` | failure accounting |

At most one `running` or `stopping` runtime exists per agent and organization. A loop in
`orbit-api` visits every `running` runtime every 5 seconds. When the last execution is
terminal and `next_run_at` is due, it claims the tick with a 5-second lease, runs the
shared preflight for the runtime creator and creates a new execution with
`trigger_source = manual`, then sets `next_run_at` to now plus `interval_seconds`.
While the last execution is still active, nothing starts.

A `failed` or `timed_out` execution counts as a failure. Each failure delays the next run
with exponential backoff (the interval, at least 30 seconds, doubled per failure, capped
at 15 minutes), and the runtime moves to `stopped` after 5 consecutive failures
(`RUNTIME_MAX_CONSECUTIVE_FAILURES`). A successful run resets the counter. The runtime
also stops with a recorded `last_error` when the agent is deleted, is no longer `active`,
the creator lost access, or the harness is unknown. Stop sets `stopping`, cancels the
active execution and becomes `stopped` once it is terminal.

### Harness (registration)
`kind ∈ {mock, opencode, claude_code, pi, generic}`. Carries a capability document
(`supports_sessions`, `supports_resume`, `supports_streaming`, `supports_cancel`,
`supports_instances`, `supports_tool_events`, `supports_usage_reporting`,
`harness_managed_providers`, `supports_platform_providers`, `execution_modes`,
`max_timeout_seconds`, `max_memory_mb`). Health is reported by workers via heartbeats;
the control plane never probes harness internals directly.

### Provider
Model provider configuration **separate from harnesses**. `kind ∈
{openai_compatible, anthropic, harness_managed}` (the database constraint currently
rejects `anthropic`, see the provider contract page). Secrets are `secret_ref` handles resolved by the
backend `SecretStore`; API responses redact them (`has_credential: true`).

### Connector / Connection / Capability / Policy
- **Connector**: integration definition (slug, version, capability catalog, auth kinds).
- **Connection**: authorized account for a connector in an org, with `credential_ref`,
  scopes and status (`active | expired | revoked | error`).
- **Capability**: `resource.action` (e.g. `issues.create`), classified by effect
  (`read | write | external_side_effect`).
- **Policy**: the `policies` table (JSON document scoped to org, agent or connection) is
  legacy and unused. Per-call rules are stored in `tool_policies` and evaluated by the
  platform, not by the model; see the runtime contract page.

### Trigger
`kind ∈ {manual, schedule, webhook, event}` (enforced by a CHECK constraint). Only
`manual` and `schedule` ever produce an execution: the scheduler claims due `schedule`
triggers, and nothing consumes `webhook` or `event` triggers (there is no inbound
webhook route and no bus consumer). They are accepted and stored.

Config is validated at creation for `schedule` (a 5-field cron expression plus an optional
IANA `timezone`, default UTC; unknown zones are rejected) and for `event` (a non-empty
`subject`). `webhook` config, including its `secret_ref`, is not validated. A schedule
trigger may also carry a `task`: the text given to the agent each time it fires. Without
one the run gets a generic "perform your configured task" prompt. Triggers can be disabled
without being deleted, and a trigger auto-disables after `max_failures` consecutive
failures. A trigger also stores an optional `harness_kind` and the shareable-resource
ownership columns. A due schedule is skipped when the agent is not `active` or its
`maxConcurrentExecutions` is reached.

Schedule semantics: the cron expression is evaluated in the trigger's `timezone`, so
`0 9 * * *` in `Europe/Amsterdam` fires at 09:00 local time across DST changes. The
scheduler claims a due trigger and stores its next `next_run_at` in the same transaction
(missed runs are not replayed), and a sweep repairs any enabled schedule trigger left
with a NULL `next_run_at`. Enabling a trigger recomputes `next_run_at` from now.

Triggers declared in an agent spec are *managed* (`triggers.managed = true`): each new
agent version replaces only managed rows, keeping the id and history of a spec trigger
whose kind and config are unchanged. Triggers created through `POST /api/v1/triggers`
are never modified by version sync.

### Execution
Lifecycle (`ExecutionStatus::can_transition_to`, `crates/orbit-domain/src/execution.rs`):

```
queued ──→ dispatched ──→ running ──→ succeeded | failed | timed_out | cancelled | rejected
  │            │             └──→ awaiting_approval ──→ running
  │            │                                    └─→ rejected | cancelled | timed_out | failed
  │            └──→ failed | timed_out | cancelled
  └──→ cancelled
```

Terminal states are `succeeded`, `failed`, `timed_out`, `cancelled` and `rejected`; they
are final and never mutated.

Fields: `agent_name`, `harness_kind`, `trigger_id`, `trigger_source`, `placement`,
`input`, `output`, `error`, `session_ref`, `usage`, `skills`, `timeout_seconds`,
`requested_by`, `worker_id`, `team_run_id`, `team_step_index`, `idempotency_key`
(unique per organization), and the timestamps `created_at`, `started_at`,
`running_started_at`, `finished_at`, `archived_at`.

- `started_at` is set when a worker claims the execution (dispatch).
- `running_started_at` is set when the worker reports `running` (the harness has
  started). The timeout reaper counts `timeout_seconds` from `running_started_at`, not from
  dispatch, so checkout and worker start-up do not consume the timeout. It falls back to
  `started_at` for rows created before migration 0050. Time spent waiting for an approval
  is added back; see the reaper section of the events page.

Claiming is a SQL compare-and-set (`UPDATE … WHERE status = 'queued'`), so concurrent
workers cannot double-claim. `archived_at` hides old terminal executions from the default
view without deleting history.

### ExecutionEvent
Append-only, per-execution monotonic `seq`: `kind ∈ {log, status, tool_call, output,
error, metric}`, `level`, `message`, `data`. Drives the SSE stream and the UI timeline.

### AuditLog
Append-only records of security-relevant actions (agent changes, secret changes,
connection grants, approvals, executions).

### Agent Team / TeamRun / TeamStep / TeamQueueItem
- **AgentTeam**: `definition` JSONB holds mode (`pipeline` | `native` | `workflow`), the
  `workflow` graph for that mode, instructions,
  ordered steps (agent + optional model override), skills, optional work source,
  multi-repository list, optional worker profile and `autoLaunch`. Tracks
  `last_poll_at` / `last_poll_error`.
- **TeamRun**: one orchestration run; terminal states `succeeded | failed | cancelled`.
  Carries `archived_at`.
- **TeamStep**: one agent step of a run (unique `run_id, step_index`), linked to the
  execution it created. Workflow runs also record `node_id` (the workflow step),
  `attempt`, the reported `outcome` and `activated_by` (the lines that started it).
- **TeamQueueItem**: durable work item, unique per `(team_id, external_key)` so an issue
  is never queued twice. Status `pending | running | succeeded | failed | cancelled`;
  tracks `attempts` / `max_attempts` for retry.

### Skill
Reusable SKILL.md package: `slug`, `content`, JSONB `files` (path → text, including
`scripts/*`), monotonic `version`.

### WorkerProfile / WorkerToken / WorkerLaunch / WorkerHeartbeat
- **WorkerProfile**: pins harness kind, target, execution driver, image + tag, agent
  scope (`all` dynamic or `selected` with an explicit allowlist), skill allowlist, optional
  provider/model, optional auth secret name, static `env`, `env_mappings`
  (connection/secret → env var), typed CPU/memory requests/limits + workspace size
  (`WorkerResources`) and `enabled`. Worker sizing lives here, not on the agent.
- **WorkerToken**: scoped, revocable, hashed worker credential (optionally bound to a
  profile and a single execution).
- **WorkerLaunch**: a launcher-managed instance (`pending | starting | running |
  stopped | failed`) with runtime id, worker id and `archived_at`.
- **WorkerHeartbeat**: live worker registry (harnesses, target, hostname, last seen).

### ApiToken / Secret / PlatformSetting / ProcessedEvent (unused)
- **ApiToken**: hashed `orbit_sk_…` token with scopes and revocation.
- **Secret**: AES-256-GCM ciphertext keyed by an opaque `reference`.
- **PlatformSetting**: key/value JSONB for platform config such as OIDC.
- **ProcessedEvent**: reserved idempotency ledger for bus delivery. The table exists, but
  no code writes to it; duplicate execution claims are prevented by the SQL compare-and-set
  and the unique `idempotency_key`.

### AgentMemory
Short-term or summary memory for an agent, with optional `expires_at`; pruned hourly.

### ToolPolicy, CustomRole, AccessTeam, EnvMapping, Output
- **ToolPolicy**: a rule (tool, capability, effect, connection, path and command
  matchers) with the decision `allow`, `require_approval` or `deny`, scoped to an
  organization and optionally an agent. See the runtime contract page.
- **CustomRole**: an organization-level permission set referenced by name from
  `users.role`.
- **AccessTeam**: a group of users with the roles `lead`, `member` and `viewer`; used for
  ownership and visibility. It is separate from an Agent Team.
- **EnvMapping**: maps a connection or secret to an environment variable for worker
  profiles and Agent Teams; see the execution plane page.
- **Output**: a result destination with an encrypted credential in `secret_ref` (a webhook
  URL for Slack/Mattermost/generic webhooks, a bot token for Telegram), with per-execution
  or per-team-run deliveries.

## Invariants

1. Every agent row has at least one immutable version.
2. Executions are never mutated in terminal states.
3. A capability invocation must be allowed by the agent's autonomy level **and** the
   connection's capability allowlist.
4. Secrets are write-only through the API.
5. Cross-organization references are rejected at the store layer.
