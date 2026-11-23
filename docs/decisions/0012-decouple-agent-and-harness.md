# 0012 — Decouple agents from harnesses

Status: **Accepted**, partly implemented (see the amendment at the end). Tracking issue: #51.

## Decision

An agent describes **behaviour and capabilities**, never a runtime. The harness is chosen
per execution (or per Agent Team step) and constrained by the worker profile.

`spec.harness` is removed entirely from `AgentSpec`. There is no `defaultHarness` field.

## Model

- **Agent**: instructions, constraints, skills, connector bindings, autonomy, resources,
  model preference, memory.
- **Execution**: agent + harness + worker profile/target + model resolution.
- **Agent Team step**: agent + optional harness (native mode requires OpenCode when
  subagents are used).
- **Worker profile**: declares supported harnesses/capabilities and placement.
- Harness-specific options live under `harnessConfig.<kind>` and are validated by the
  selected adapter when the agent is saved, not stored as part of the agent identity.
  Settings that choose what the worker executes (`binary`, `extraArgs`), carry the policy
  hook (`orbitMcp`) or point at a shared server (`serverUrl`) are the platform's and are
  refused in agent specs (#303).

## Resolution

1. Explicit execution/team-step harness.
2. Otherwise the worker profile's harness.
3. There is no agent-level fallback; a run without a resolvable harness fails closed.

Capability negotiation runs before queueing:

- harness supports the agent's execution mode, skills, connector/broker tools and model
- native subagents require a harness that supports them (currently OpenCode)
- unsupported combinations return a validation error, not a runtime surprise

## Migration

- Existing agent versions keep their old `spec.harness` for reproducibility, but new
  versions omit it.
- Bootstrap/demo agents get an explicit harness in their bootstrap profile/launch.
- API clients must send `harness` on execution/team-run creation when no profile default
  exists.
- OpenAPI/schema, web editor, YAML import/export and docs change in one release; this is a
  breaking spec change (pre-1.0 minor + changelog/ADR note).

## Consequences

- One agent can run on OpenCode, Pi or future harnesses if capabilities allow.
- Harness-specific config cannot silently leak into the agent definition.
- The execution plane must resolve harness + profile consistently.
- Agent Teams native mode becomes a harness capability check, not an agent property.

## Amendment (2026-11-21): what the code does today
Execution and team-run requests accept `harness` (and `harnessOverride` as a legacy
alias). `AgentSpec.harness` still exists as a deprecated field that new versions omit, and
resolution is: explicit request or team `harness`, then the deprecated `spec.harness` of
an old version, then the organization default harness (`GET/PUT /api/v1/settings/harness`).
A run with no resolvable harness is rejected with a validation error. The "worker
profile's harness" step and the removal of the agent-level fallback described above are
not implemented.
