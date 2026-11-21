# Agents

An agent is a versioned definition (`orbit.io/v1`) of *what* should run. It does not
contain an execution loop; a worker runs it through a harness adapter.

## Definition

| Field | Purpose |
|---|---|
| `metadata.name` / `description` / `labels` | Identity and grouping |
| `spec.harness` | Deprecated. New versions omit it; older versions may carry a harness `type` and `placement`. The harness is chosen per run (see [Harness and placement](#harness-and-placement)) |
| `spec.harnessConfig` | Harness-specific options, keyed by harness kind (`opencode`, `pi`) |
| `spec.model` | Harness-managed provider/model or a platform `openai_compatible` provider |
| `spec.instructions` | Trusted system prompt, kept separate from untrusted input |
| `spec.runDefaults.task` | Optional task pre-filled for manual runs; never used as system instructions |
| `spec.connectors` | Connector bindings and the capabilities the agent may request |
| `spec.triggers` | `manual`, `schedule` (cron, an optional IANA `timezone` that is UTC when omitted, and an optional `task`), `event` or `webhook`. Only manual and schedule start runs today; see [Triggers](#triggers) |
| `spec.autonomy` | `level` 1–4 (default 2, Recommend), `approvalMode` and `maxExecutionsPerHour` (1–10000) |
| `spec.resources` | Execution timeout, concurrency, max output tokens. Worker CPU/memory live on the worker profile. `network` (egress mode and allowed hosts) is validated and recorded but **not yet enforced** (see below) |
| `spec.memory` | `enabled`, `strategy` (`none`, `short_term` or `summary`; required when memory is enabled) and `retentionDays` (1–3650). When enabled, the worker adds stored memory entries to the run's payload. Agent Studio has no control for memory; set it through the API or an import |
| `spec.skills` | SKILL.md bundles materialized into the workspace |

## Autonomy

| Level | Behaviour |
|---|---|
| 1 Observe | Read-only capabilities; writes and side effects are denied |
| 2 Recommend | Analyse and propose; writes and side effects are denied (no approval path) |
| 3 Prepare | Writes/side effects require approval |
| 4 Autonomous | Allowlisted writes execute without approval |

Levels 1 and 2 deny writes regardless of approval mode; an approver cannot override that.
To allow approval-gated writes, raise the agent to level 3 (Prepare). Reads that are on the
connector allowlist are allowed at every level.

`approvalMode` is `auto`, `always` or `writes`. Agent Studio labels them "Follow the
autonomy level", "Ask for approval every time" and "Ask before external side effects".
Today `always` and `writes` behave the same: at level 3 and above, both ask for approval
for every write and every external side effect (`evaluate_capability` in
`crates/orbit-domain/src/autonomy.rs`), so the "Ask before external side effects" label
describes a narrower rule than the platform applies. With `auto`, level 3 asks and level 4
runs allowlisted writes without approval. See [Approvals](approvals.md).

Approval is enforced by the platform (`evaluate_capability`), never by the prompt. The
tool policy layer can add argument-level restrictions and approvals on top
([tool-policies.md](tool-policies.md)).

## Network egress (not enforced yet)

`spec.resources.network` (`mode`: `default`, `allowlist`, `denied`; `allowedHosts`) is
validated and stored with the agent version, but **the platform does not enforce it**: no
worker or harness adapter reads it. Do not rely on it for isolation. Restrict outbound
traffic at the network layer instead, for example with a Kubernetes NetworkPolicy for the
worker or Job pods. `allowedHosts` accepts hostnames and `*.domain` patterns; a bare `*`,
empty or malformed entries are rejected.

## Triggers

An agent runs when someone starts it, or on a schedule. Only these two start runs today:

- A **manual** run starts from the agent page, [Start work](start-work.md) or the API.
- A **schedule** trigger fires on its cron expression, but only while the agent is
  **Active**. A schedule on a Draft, Paused or Archived agent is skipped. A schedule also
  skips a tick when the agent is at its concurrent-run limit. After five consecutive
  failures to start a run the trigger is disabled.

`event` and `webhook` triggers are saved with the agent, but nothing fires them yet. Agent
Studio says so next to each one. It keeps "+ Webhook" disabled, because
`GET /api/v1/meta` reports `features.webhookTriggers` as false.

## Lifecycle

A new agent is a **Draft**. Draft → Active ⇄ Paused → Archived. A Draft can be run by hand;
its triggers only fire once it is Active. Use **Pause** and **Resume** on the agent page to
stop and restart triggers. **Archive agent** stops triggers and means the agent can no
longer run or be edited; its versions and execution history are kept. **Restore agent**
(also offered in the Archived filter of the Agents list) activates an archived agent again.
**Delete permanently** is offered only for archived agents.

Editing creates a new immutable version; executions pin the version they started with.
Undoing a change (returning to an earlier configuration) is saved as a new version too;
saving without changes creates none, and a change to only the name, description or labels
does not create a version. Run an agent from its page, [Start work](start-work.md), or the
API.

`GET /api/v1/agents` accepts `status` as `draft`, `active`, `paused`, `archived` or
`current` (everything that is not archived).

## Agent Studio

Create an agent at **Agents → New agent** and change one with **Edit** on its page. Studio
shows one section at a time; the section rail on the left shows what is missing:

1. **Identity**: name, description and labels.
2. **Instructions**: the system instructions and constraints.
3. **Model**: who manages the model (harness or ORBIT platform provider), provider and model.
4. **Connectors and skills**: bind connectors to a saved connection and choose the
   "Allowed capabilities" (see [Connectors](connections.md)); attach skills.
5. **Triggers**: schedules, events and webhooks (see [Triggers](#triggers)).
6. **Autonomy and limits**: autonomy level, "Approval mode", and under "Run limits" the
   timeout, concurrent runs, runs per hour and output tokens.
7. **Access control**: owner team and visibility ([access teams](access-teams.md)).

If you have unsaved changes and try to leave, close the tab or follow a link, Studio asks
first. Saving does not prompt.

Studio remembers the version you started from and sends it as `expectedVersion`. If someone
else saved a newer version first, Studio shows "Someone saved version N while you were
editing" and the API refuses your save with 409: "this agent changed while you were
editing". Reload, then make your change again.

## Harness and placement

The harness is the execution environment, not the agent's identity. For a run, ORBIT picks
the harness in this order: the `harness` sent with the request (`harnessOverride` is an
older name for the same field), then the harness stored on an older agent version, then the
organization default harness. The default is set at **Settings → Platform → Default
harness** ([settings](settings.md)). If none of these exists, the run is refused with a
validation error on `harness`.

Placement (`auto`, `local`, `docker`, `kubernetes`) says which kind of worker may take the
run. `POST /api/v1/agents/{id}/executions` accepts `placement` for that run only;
"Where" in [Start work](start-work.md) sends it. Without it, the placement stored on an
older agent version applies, else `auto`. Team runs accept `harnessOverride`
([agent teams](agent-teams.md)).
