# ADR-0008: Harness / Provider / Model / Worker terminology

**Status:** Accepted. 0.4.0 rename, 0.5.0 worker-token API and 0.5.1 launcher implemented (see the amendment at the end).

## Context

"Runtime" is used for three different things: the agent loop/framework (Pi, OpenCode),
the process/container environment, and the adapter implementation. This makes the domain
and UI ambiguous. Provider and model are also conflated in the Models screen.

## Decision

| Term | Meaning |
|---|---|
| **Harness** | Agent loop/framework adapter: Pi, OpenCode, Claude Code, generic, mock |
| **Worker** | Execution-plane process/container that runs harnesses |
| **Sandbox / execution environment** | Per-execution container/Job/process isolation |
| **Provider** | Credentialed model endpoint (OpenAI-compatible) or harness-managed auth |
| **Model** | Model id + capabilities exposed by a provider or harness |
| **Execution** | One invocation of an agent version |
| **Placement/target** | Where a worker runs: local, docker, kubernetes |
| **Worker profile** | Declarative bootstrap spec (harness, image, target, agents, skills, auth) |

Never use "runtime" for a harness in user-facing text or new APIs. Use "sandbox" or
"execution environment" for the container/process boundary.

## API/DB migration (0.4.0, breaking, no aliases)

- DB: `runtimes` → `harnesses`, `runtime_kind` → `harness_kind`, `runtime_id` →
  `harness_id`; transform `agent_versions.spec.runtime` → `spec.harness`.
- API: `runtime`/`runtimeKind` → `harness`/`harnessKind`; `/api/v1/runtimes` →
  `/api/v1/harnesses`; regenerate `packages/api-client`.
- Worker env: `ORBIT_WORKER_RUNTIMES` → `ORBIT_WORKER_HARNESSES`,
  `ORBIT_EXECUTION_IMAGE_<RUNTIME>` → `ORBIT_EXECUTION_IMAGE_<HARNESS>`.
- Profiles: `runtime_kind` → `harness_kind`.

This is a clean cut before 1.0; no compatibility aliases.

## Providers and models

- UI navigation: **Providers**, not Models. The page is "Providers & models".
- A provider has an endpoint, credential reference and compatibility flags; a model id is
  a separate field/list.
- Harness-managed auth (OpenCode Go, Claude Code) is shown under the harness, not as a
  generic provider.

## Worker bootstrap and identity

Current worker bootstrap exposes `DATABASE_URL` + `MASTER_KEY`; this is too much
privilege. Target:

- Worker gets `ORBIT_API_URL`, a scoped/revocable `ORBIT_WORKER_TOKEN` and
  `ORBIT_WORKER_PROFILE_ID`. No DB, NATS or master key.
- Worker calls claim/heartbeat/events/complete/credential-lease endpoints.
- Control plane owns DB, secrets, policy, approvals and audit.
- A separate `orb-launcher` (Docker socket or namespaced K8s RBAC) starts workers from a
  profile; the API never gets container/cluster privileges.
- Launches are ephemeral `WorkerLaunch` records with one-time tokens, status and logs.

### Why this is staged

The worker currently hosts the tool broker, approvals polling, connector credential
leases, event persistence and skills materialization. Removing DB access requires moving
those behind worker-facing control-plane APIs (or a broker service), so it is a separate
milestone. The generated bootstrap must not claim token-only mode until that API exists.

## Consequences

- 0.4.0 is a breaking contract migration; regenerate clients and update deployments.
- 0.5 adds worker tokens, API-based claim/events/complete, credential leases and launcher.
- The UI already uses harness/provider wording; the API follows in 0.4.0.

## Amendment (2026-11-21): bootstrap is now token-only
The staged target above is reached. Workers hold no database URL, event-bus access or
master key; they use `ORBIT_API_URL` and a scoped worker token (`/api/v1/worker/*`), and
the launcher (`services/launcher`) is the only component with Docker or workload
privileges. ADR 0002, 0003 and 0007 still use the pre-0.4.0 "runtime" terms; read them as
"harness". See [Execution Plane](../architecture/execution-plane.md).
