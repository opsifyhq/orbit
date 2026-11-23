# Harness Adapter Contract

Rust source of truth: `crates/orbit-contracts/src/harness.rs`.
Adapters: `crates/orbit-harness/src/{mock,opencode,pi,planned}.rs`, registered in
`HarnessRegistry::builtin()` (`registry.rs`).

## Boundaries

- `orbit-api` never instantiates adapters. It persists intent and publishes events.
- `orbit-worker` resolves an adapter from the registry and executes it.
- Adapters may spawn subprocesses or containers, but receive no database handle.
- Adapters communicate progress only through the supplied event channel; they never
  write to `execution_events` directly.

## Trait

```rust
#[async_trait]
pub trait HarnessAdapter: Send + Sync {
    fn kind(&self) -> HarnessKind;
    fn descriptor(&self) -> HarnessDescriptor;
    fn capabilities(&self) -> HarnessCapabilities; // default: descriptor().capabilities

    // Returns all problems at once instead of failing on the first.
    async fn validate_config(&self, config: &HarnessConfig) -> ValidationReport;
    async fn health(&self, config: &HarnessConfig) -> Result<HarnessHealth, HarnessError>;

    // Instance lifecycle. Part of the contract, but the platform does not call it today.
    async fn create_instance(&self, request: CreateInstanceRequest) -> Result<HarnessInstance, HarnessError>;
    async fn start_instance(&self, instance: &HarnessInstanceRef) -> Result<(), HarnessError>;
    async fn stop_instance(&self, instance: &HarnessInstanceRef) -> Result<(), HarnessError>;
    async fn delete_instance(&self, instance: &HarnessInstanceRef) -> Result<(), HarnessError>;
    async fn instance_status(&self, instance: &HarnessInstanceRef) -> Result<HarnessInstanceStatus, HarnessError>;

    async fn execute(&self, request: ExecutionRequest) -> Result<ExecutionOutcome, HarnessError>;
    // Default implementation returns `Unsupported`.
    async fn resume(&self, request: ResumeRequest) -> Result<ExecutionOutcome, HarnessError>;
}
```

The platform's continuous mode does not use instances; it starts one execution after
another (see `AgentRuntime` in the domain model).

`execute` is a long-running future. Cancellation and timeouts are delivered through
`ExecutionRequest::cancel: CancellationToken` and `ResourceLimits::timeout_seconds`;
adapters must terminate child processes/containers on cancellation.

The timeout covers **active** time only. While the platform waits for a person to decide
on an approval it pauses `ExecutionRequest::pause` (`PauseClock`); adapters enforce their
timeout with `request.pause.deadline(timeout)` instead of a plain sleep, so a slow
approver never turns into a timed-out execution.

The control plane also enforces the timeout as a backstop. Its reaper counts from
`running_started_at` (the moment the worker reports `running`, not dispatch), adds the
time spent waiting for approvals and a grace period, and then marks a still-`running`
execution `timed_out`. See the reaper section of the events page.

## Capabilities

Advertised per adapter and surfaced to the UI (`GET /api/v1/harnesses`):

| Capability | Meaning |
|---|---|
| `supports_sessions` | harness can hold conversation/session state |
| `supports_resume` | a prior session can be continued |
| `supports_streaming` | incremental log/tool events are emitted |
| `supports_cancel` | an in-flight execution can be cancelled |
| `supports_instances` | long-running instances are meaningful |
| `supports_tool_events` | tool/connector invocations are reported |
| `supports_usage_reporting` | token/CPU usage is reported |
| `execution_modes` | `[on_demand]`, `[long_running]` or both |
| `max_timeout_seconds` | upper bound on requested timeouts |
| `harness_managed_providers` | harness owns provider inventory/auth |
| `supports_platform_providers` | harness can use a provider configured in ORBIT |
| `max_memory_mb` | upper bound on requested memory |

## Streaming events

`HarnessEvent` is a struct: `{ kind, level, message, data }`. `kind` is a
`HarnessEventKind`: `Started` (carries the session reference), `Status`, `Log`,
`ToolCall`, `OutputDelta`, `Usage` or `Error`. Variant-specific values such as tool
input and output or token counts travel in `data`.

Events are forwarded by the worker to `execution_events` (with a monotonic `seq`) and to
`orbit.execution.updated` for subscribers. Adapters should emit events incrementally,
not only at completion.

## Version skew between API and workers

Workers decode the context the API sends with strict types (`deny_unknown_fields` on the
agent spec). A field added to a type workers decode breaks every older worker that
receives it, so the API leaves out fields the execution plane never reads (for example the
schedule trigger's `task`, see `omit_fields_workers_ignore` in
`crates/orbit-api/src/handlers/worker_api.rs`). A worker that fails to decode a claim
leaves the execution `dispatched` while its heartbeat stays fresh. The API's background
reaper ends such runs: `failed` with error code `worker_did_not_start` after 10 minutes
without activity (claim, event or cancel request), or `cancelled` 60 seconds after an
unanswered cancel request (`reap_unstarted_executions` in `crates/orbit-store/src/workers.rs`).
A worker that reports `running` for an execution that has already ended gets `409 Conflict`
and must not start the harness.

### Execution liveness

A worker process has a per-process `bootId` (UUID v7) and `startedAt` timestamp in addition
to its routing `workerId`. Every worker heartbeat includes both and the ids of all active
executions. For each listed execution the API refreshes `last_execution_heartbeat_at` and
extends `execution_lease_until` by `ORBIT_EXECUTION_LEASE_SECONDS` (90 by default), which is
several heartbeat intervals. A missing field is treated as an older worker: its runs stay
on worker-level heartbeat liveness only, so mixed-version rolling upgrades are safe.

When a heartbeat arrives with a new `bootId`, executions still active under the previous
boot are failed with `worker_lost` and are not resumed: their side effects are unknown.
When the worker's own heartbeat stays fresh but an execution stops appearing in
`activeExecutionIds` past its lease, the API fails it with `harness_unresponsive` after the
same 90-second grace period. In both cases team steps, runs and queue items become terminal
without an automatic retry. Workers never derive permissions from these fields.

## Inputs

`ExecutionRequest` carries:

- `execution_id`, `agent_id`, `agent_version_id`, `agent_name`
- trusted `instructions` (system prompt) — kept separate from untrusted user/event content
- `input`: task text, structured payload, and provenance (`source`, `trusted: false`)
- `model`: an optional `ResolvedModel` (harness-managed selection or a platform endpoint
  with a leased credential, never serialized into logs); `subagent_models` holds the
  resolved models of native subagents
- `grants`: platform-approved connector capabilities (`connection_id`, `capability`,
  `effect`)
- `autonomy`: 1–4 with platform-enforced semantics
- `resources`: timeout, memory, CPU, max output tokens, max concurrent executions
- `harness_config`: adapter-owned JSON. On agent create/update the API calls
  `validate_config` on the adapter for each configured `spec.harnessConfig.<kind>` (and
  for the deprecated `spec.harness.config` of older versions), and refuses the settings
  only the platform may set (`binary`, `extraArgs`, `orbitMcp`, `serverUrl`;
  `orbit_contracts::PLATFORM_ONLY_HARNESS_SETTINGS`). The worker also removes them from
  agent specs saved before that check. Pi `skills` paths must stay inside the workspace
- `extra_env`: environment variables resolved by the control plane (for example
  environment mappings used by skill scripts); never serialized and never logged
- `session`: session reference to resume, if any
- `workspace`: per-execution scratch directory contract (`WorkspaceSpec`)
- `events`, `cancel`, `pause` (the `PauseClock` described above)

## Error model

`HarnessError { kind, message, retryable, details }` where `kind ∈ {invalid_config,
unauthorized, unavailable, unsupported, timeout, cancelled, execution_failed, protocol,
internal}`. Workers map `retryable` errors to bounded retries with exponential backoff;
non-retryable errors move the execution to `failed` immediately.

## Placement and worker targets

Agents may set `spec.harness.placement`:

| Value | Claimed by |
|---|---|
| `auto` | any worker supporting the harness |
| `local` | a worker running natively on the host (`ORBIT_WORKER_TARGET=local`) |
| `docker` | a Docker/Compose worker or the local container dispatcher (`docker`) |
| `kubernetes` | workers deployed by the Helm chart (`kubernetes`) |

Placement is a routing constraint, not an isolation guarantee. Isolation depends on the
worker's execution driver (process vs per-execution container/Job). Workers report target
and harnesses in `worker_heartbeats`; `GET /api/v1/workers` exposes them.

## Isolation requirements

Production adapters must run in a container/Job with: non-root user, read-only root
filesystem where possible, CPU/memory limits, `no_new_privileges`, a per-execution
writable workspace, outbound network restriction (target state; the per-agent `network` policy is not yet enforced, use a Kubernetes NetworkPolicy), secret injection via env or tmpfs
files, and forced teardown on timeout/cancel.

## Implementation status

`ImplementationStatus` has three values: `available` (implemented and exercised by
contract tests and a real-harness test), `scaffolded` (contract implemented, not yet
certified against the real harness) and `planned` (descriptor only). No adapter is
currently `scaffolded`.

- `mock` — `available`; used for local development and contract tests.
- `opencode` — **available**, tested against OpenCode **v2.3.0**:
  - `opencode run --standalone --format json` in a per-execution workspace
  - trusted instructions written to `AGENTS.md`; untrusted input passed in the prompt
  - NDJSON text/tool/usage/error events mapped to ORBIT harness events
  - session IDs captured; `--session` used for resume; cancel and timeout SIGTERM then SIGKILL the whole process group, and any exit SIGKILLs leftovers (shared supervisor in `crates/orbit-harness/src/process.rs`); an `error` event with no later answer fails the run even when the CLI exits 0; worker-written files never follow workspace symlinks
  - `PWD` and the working directory are pinned to the per-execution workspace
  - harness-managed providers via `opencode auth export` imported from a mounted Secret;
    ORBIT never passes raw provider keys in CLI arguments
  - **platform providers supported**: ORBIT-resolved OpenAI-compatible endpoints (vLLM,
    Ollama, gateways) are written into `opencode.json` with their base URL, headers and
    leased credential (`supports_platform_providers=true`)
  - GitHub tools (read and approval-gated write) verified end-to-end through the MCP
    tool broker
  - contract + real-harness test: `crates/orbit-harness/tests/opencode_e2e.rs`
    (`ORBIT_OPENCODE_E2E=1`, ignored by default to avoid model spend)
- `pi` — **available**, tested against Pi **1.5.2**:
  - `pi --print --mode json`, session capture/resume, JSON event mapping,
    cancellation/timeout and trusted `--append-system-prompt`
  - platform-enforced read-only tool allowlist (`read,grep,find,ls`) at autonomy L1/L2
  - **platform providers supported**: endpoint and leased credential injected as
    `OPENAI_API_KEY` / `OPENAI_BASE_URL`, with `--provider orbit-platform` when a platform model
    is selected (`supports_platform_providers=true`)
  - writes the execution-scoped `.pi/mcp.json` broker bridge and accepts `--skill` paths
  - ships as its own worker image (`deploy/docker/worker-pi.Dockerfile`)
- `claude_code` and `generic` — `planned`: registered as descriptors only
  (`planned.rs`); they cannot execute and fail closed.

## Process supervision, workspace and environment

The OpenCode and Pi adapters share the same supervisor, so their behaviour cannot drift.

- **Process supervision** (`process.rs`). The harness starts in its own process group.
  On cancel or timeout the supervisor sends SIGTERM to the whole group, waits 3 seconds
  and then sends SIGKILL. When a run ends for any reason it also SIGKILLs everything left
  in the group, so tool children or an MCP bridge cannot outlive the execution. After
  stdout closes the harness gets 10 seconds to exit, and stderr is captured with a
  16 KB bound. Both adapters treat an `error` event with no later answer as a failed run
  even when the CLI exits with code 0.
- **Workspace confinement** (`paths.rs`). `workspaceSubdir` and `sessionDir` must be
  relative paths without `..` and must resolve, symlinks included, inside the execution
  workspace. Files the worker writes into the workspace (instructions, `opencode.json`,
  skills, policy hooks) never follow a symlink: a symlinked parent directory is refused,
  a symlink at the file path is unlinked and replaced, and the file is opened with
  `O_NOFOLLOW`. A cloned repository can ship symlinks, so this prevents it from
  redirecting those writes elsewhere on the host.
- **Environment scrubbing** (`process_env.rs`). The harness does not inherit the worker's
  environment. Only an allowlist is passed through (`PATH`, `HOME`, `USER`, `LANG`,
  `TMPDIR`, TLS certificate variables, and names starting with `XDG_` or `LC_`), plus the
  variables the adapter sets explicitly. `ORBIT_WORKER_TOKEN` and `ORBIT_API_URL` are
  never passed to the harness, even as explicit variables.

## Tool policy layer

Native harness tools and brokered connector tools share one per-call policy:

```text
harness tool call ──► worker broker /policy or /tools/call
                        │
                        ├─ platform decision (autonomy × approval mode × grant)
                        └─ tool policy rules (org defaults + agent rules)
                              → allow | require_approval | deny
```

- Rules live in `tool_policies` (migration `0019`), scoped to an org and optionally an
  agent. Agents inherit org rules; agent rules are added, not replaced.
- `evaluate_tool_call` (`crates/orbit-domain/src/tool_policy.rs`) combines the platform
  decision with matching rules. Platform denial always wins; rules can only make the
  decision stricter. With no matching rule the platform decision is also the default.
- Matchers are AND-combined: tool glob, capability glob, effect, connection, path glob
  (lexically normalized, component-suffix aware) and command regex. Empty matchers match
  every call and are useful for a default `require_approval`.
- Precedence: platform `deny` → rule `deny` → `require_approval` → `allow`.
- Native and brokered connector tools both use their platform decision as floor and
  default. At L4/Auto an allowlisted native tool therefore runs without approval; at
  L3/Auto writes wait for approval; L1/L2 deny writes. Add an explicit
  `require_approval` rule to make a specific native tool ask again.
- The worker receives the effective rules in `WorkerExecutionContext.tool_policies` and
  starts the broker when rules exist even if the agent has no connector binding.
- `POST /policy` on the loopback broker (bearer: execution-scoped broker token) is the
  adapter hook for native tools: request `{ tool, arguments, argumentsHash? }` (legacy
  `capability`/`effect`/`connection` are ignored; the effect is derived from the tool name,
  unknown tools count as `write`, and autonomy plus approval mode are applied through
  `evaluate_capability`). The response is the decision with `ruleId`, `ruleName`, `reason`,
  `impact` and `argumentsHash`, which the approval is bound to; a presented hash that does
  not match the arguments is denied. See `docs/guide/tool-policies.md`.
  `require_approval` creates a durable approval and blocks until it is decided or its own
  15-minute window expires (the execution timeout is paused meanwhile); expiry and
  rejection deny the call. An action a person rejected is denied without a new approval
  if the agent requests the identical capability and arguments again in the same run.
- Management API: `GET/POST /api/v1/tool-policies`, `PATCH/DELETE
  /api/v1/tool-policies/{id}` (`settings:read` / `settings:write`).
- Approvals record the rule, reason, impact and arguments for the reviewer. Every deny
  and approval is audited and emitted as a `tool_call` execution event.

What is enforced for native tools today:

- The worker starts the broker for every execution of a harness with native tools
  (OpenCode, Pi), whether or not the agent has connector tools or there are tool policy
  rules; only the mock harness runs without one. The OpenCode adapter writes a plugin
  (`.opencode/plugins/orbit-policy.ts`) and the Pi adapter an extension
  (`.pi/extensions/orbit-policy.ts`). Both call `POST /policy` before **every** tool call,
  including tools they do not know by name, and block the call on any deny, error or
  missing broker. Unknown tools count as `write`, so a new tool fails closed.
- ORBIT's own MCP tools reach the hook under their MCP names (`orbit_<tool>`,
  `mcp__orbit__<tool>`). `/policy` lets them through because the broker checks them when
  they are called.
- Claude Code is `planned`; its `PreToolUse` integration does not exist.

**Autonomy baseline for native tools (enforced by the adapter, independent of the
broker).** Adapters derive a native-tool baseline from `request.autonomy` and never rely
on the broker or `/policy` being present:

- Pi: at `<= RECOMMEND` only `read,grep,find,ls,codemode` are enabled.
- OpenCode: at `<= RECOMMEND` the generated `opencode.json` denies `bash`, `edit`,
  `write`, `patch`, `multiedit` and `apply_patch` (`permission` = `deny`, and hidden via
  `tools` = `false`), and `--auto` is never passed, regardless of the `autoApprove`
  harness setting. Read-only tools (`read`, `grep`, `glob`, `list`) stay allowed; web
  tools keep their own default (denied unless enabled). Above `RECOMMEND` there is no
  baseline and `--auto` keeps its default.
- Worker-profile `nativeToolPermissions` are merged onto the baseline and can only make a
  tool stricter (`deny` > `ask` > `allow`); they never loosen it.

Approval mode therefore applies to native tools in every run of these harnesses, also
for an agent without connectors or tool policy rules (#303; previously the broker only
started when one of those existed, see #110).
