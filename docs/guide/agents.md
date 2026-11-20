# Agents

An agent is a versioned definition (`orbit.io/v1`) of *what* should run. It does not
contain an execution loop; a worker runs it through a harness adapter.

## Definition

| Field | Purpose |
|---|---|
| `metadata.name` / `description` / `labels` | Identity and grouping |
| `spec.harness.kind` | Default harness (`mock`, `opencode`, `pi`, future `claude_code`) |
| `spec.model` | Harness-managed provider/model or a platform `openai_compatible` provider |
| `spec.instructions` | Trusted system prompt, kept separate from untrusted input |
| `spec.runDefaults.task` | Optional task pre-filled for manual runs; never used as system instructions |
| `spec.connectors` | Connector bindings and the capabilities the agent may request |
| `spec.triggers` | Manual, schedule (cron plus an optional IANA `timezone`, UTC when omitted), event or webhook |
| `spec.autonomy` | Level 1–4, approval mode and per-hour quota |
| `spec.resources` | Execution timeout, concurrency, max output tokens. Worker CPU/memory live on the worker profile. `network` (egress mode and allowed hosts) is validated and recorded but **not yet enforced** (see below) |
| `spec.memory` | Whether the agent reads/writes durable memory |
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

## Lifecycle

Draft → Active → Paused → Archived. Editing creates a new immutable version; executions
pin the version they started with. Agent Studio shows the resolved harness/provider/model
and the execution limits. Run an agent from its page, the Work wizard, or an API/trigger.

## Harness vs agent

The harness is the execution environment, not the agent's identity. Work and team runs may
pass a `harnessOverride` for a run; otherwise the agent's stored harness is used.
Worker placement (`auto`, `local`, `docker`, `kubernetes`) is a routing constraint.
