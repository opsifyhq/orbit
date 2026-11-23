# ORBIT Architecture Overview

ORBIT is a self-hosted control plane for creating, connecting, deploying and governing
persistent AI agents. It is **not** an agent framework: agent reasoning happens in
dedicated harness processes (OpenCode, Claude Code, Pi, custom) behind a stable adapter
contract.

## 1. Layers

```
┌──────────────────────────────────────────────────────────────────────┐
│ A. Agent Experience                                                  │
│    Next.js web app (apps/web)  ── future Expo/React Native apps      │
└───────────────▲──────────────────────────────────────────────────────┘
                │ REST /api/v1 + SSE (OpenAPI contract)
┌───────────────┴──────────────────────────────────────────────────────┐
│ B. Control Plane                                                     │
│    orbit-api (axum)  ── authn/z, agents, executions, policies, audit, │
│                         scheduler and other background loops          │
│    PostgreSQL        ── durable state                                 │
│    NATS JetStream    ── event notifications (dev: in-memory bus)      │
└───────────────▲──────────────────────────────────────────────────────┘
                │ worker API (/api/v1/worker/*, scoped worker token)
┌───────────────┴──────────────────────────────────────────────────────┐
│ C. Execution Plane                                                   │
│    orbit-worker      ── claims executions, runs a harness adapter,    │
│                         brokers connector tools                       │
│    orbit-launcher    ── starts and stops workers (Docker, Kubernetes) │
│    opencode · pi · mock (available) · claude-code / generic (planned) │
└──────────────────────────────────────────────────────────────────────┘
```

The control plane never embeds an agent loop. The worker owns the adapter instances
(`HarnessAdapter`, `crates/orbit-contracts`): the mock harness runs in-process in
`orbit-worker`, OpenCode and Pi run as supervised subprocesses in a per-execution
workspace. Workers themselves run as a local process, a container or a Kubernetes pod,
started by hand or by the launcher (`services/launcher`); the worker's own container
dispatcher (`services/worker/src/dispatch.rs`) can start one container per execution.

## 2. Harness topology (MVP)

| Component | Local (Compose) | Kubernetes (Helm) |
|---|---|---|
| `orbit-api` | container, port 8080 | Deployment + Service + Ingress |
| `orbit-worker` | container | Deployment (mock) or Job per execution |
| `orbit-worker-opencode` | container | Deployment (OpenCode CLI bundled) |
| `orbit-worker-pi` | container | Deployment (Pi CLI bundled) |
| web | container, port 3000 | Deployment + Service + Ingress |
| PostgreSQL | container | external or bundled (opt-in) |
| NATS JetStream | container | external or bundled (opt-in) |
| Harness processes | mock in-process; OpenCode/Pi as subprocess in a per-execution workspace | Pod/Job per worker; Job per single execution via the launcher |

## 3. Code structure

ORBIT is a Rust workspace with a Next.js frontend:

| Directory | Purpose |
|---|---|
| `crates/orbit-domain` | Domain types, validation, autonomy policy |
| `crates/orbit-contracts` | Cross-boundary traits and DTOs (harness, provider, connector, events) |
| `crates/orbit-store` | PostgreSQL repositories and migrations |
| `crates/orbit-events` | Event bus (in-memory and NATS JetStream) |
| `crates/orbit-security` | Authentication, permissions, secret encryption, licensing |
| `crates/orbit-harness` | Harness adapters (mock, OpenCode, Pi), process supervision, workspace confinement |
| `crates/orbit-kubernetes` | Read-only Kubernetes client behind the Kubernetes connector |
| `crates/orbit-api` | HTTP API (handlers, routes, OpenAPI, background loops) |
| `services/api` | `orbit-api`: the control-plane binary |
| `services/worker` | `orbit-worker`: the execution-plane binary and its tool broker |
| `services/launcher` | `orbit-launcher`: starts and stops workers; the only component with the Docker socket or workload RBAC |
| `services/license-cli` | Offline license signing tool (ADR 0010) |
| `apps/web` | Next.js web app |
| `packages/api-client` | Generated OpenAPI client and document |
| `packages/contracts` | Zod agent-definition schemas used by the web app |

See `AGENTS.md` in the repository root for the full repository map.

## 4. Request & execution flow

Manual execution:

1. `POST /api/v1/agents/{id}/executions` validates agent state, autonomy and quotas.
2. A row in `executions` is inserted with status `queued` and an idempotency key.
3. `orbit.execution.requested` is published on the event bus.
4. A worker polls the API and claims the execution (`queued → dispatched`,
   compare-and-set), resolves the harness adapter, creates or resumes a session and posts
   events back to the API.
5. The API persists events in `execution_events`; status transitions are persisted and
   published as `orbit.execution.updated`.
6. The frontend stream (`GET /api/v1/executions/{id}/events/stream`) replays persisted
   events and then tails new ones (SSE, `Last-Event-ID` supported).

Scheduled and team-driven executions use the same queued path. Background loops live in
`orbit-api`: the scheduler claims due cron triggers and stores their next run atomically (`FOR UPDATE SKIP LOCKED`), the team
advancer moves pipeline runs forward, the work-source dispatcher enqueues issues, and the
reaper fails orphaned executions. A worker only needs `ORBIT_API_URL` and a scoped
`ORBIT_WORKER_TOKEN` — no database, bus or master-key access. Deduplication lives in
PostgreSQL (the compare-and-set claim and the unique `idempotency_key`), not in memory.

## 5. Isolation & security

- Agent tasks are **never** executed inside `orbit-api`.
- Adapters receive a constrained `HarnessExecutionContext` (timeout, CPU/memory,
  network policy (recorded, not yet enforced), filesystem policy, injected secret handles — never raw secrets in
  prompts or logs).
- Autonomy is enforced by the platform before dispatch and at capability invocation
  time: connector capabilities are checked against per-agent allowlists and autonomy
  level; level ≤ 2 is read-only, level 3 creates approvals, level 4 executes allowed
  capabilities.
- All secret material is encrypted at rest (AES-256-GCM, `ORBIT_MASTER_KEY`) and is
  never returned by the API. `SecretStore` is pluggable (Vault later).
- External content is untrusted: harness input is wrapped with provenance metadata and
  trusted system instructions are transported separately.

## 6. Extensibility

- **New harness** → implement `HarnessAdapter` in `crates/orbit-harness`, register it in
  the registry; no domain/API changes.
- **New provider kind** → add a `ProviderKind` variant with a config struct and
  capability set; harness adapters translate it to their own provider format.
- **New connector** → add a manifest in `crates/orbit-api/src/connectors.rs` and a
  client in the worker; the API and UI read the registry automatically.
- **New infrastructure** → Compose and Helm consume the same env-var contract.

## 7. Mobile readiness

The backend is a pure HTTP/JSON + SSE API with bearer-token auth (OIDC access tokens,
`orbit_sk_…` API tokens and separate `orbit_wk_…` worker tokens), versioned under
`/api/v1`, with keyset pagination and a stable error envelope. No cookie-only or browser-specific behavior is required, so an
Expo/React Native client can share `packages/api-client` and reuse the agent-definition
validation in `packages/contracts`.

## 8. Observability

- Structured JSON logs (`tracing`) with request IDs and trace IDs.
- `GET /healthz` (liveness) and `GET /readyz` (readiness depends on the database; event bus health is reported but non-gating).
- `GET /metrics` on the API: Prometheus text endpoint covering HTTP, executions, workers,
  harnesses, background loops, the database pool, the event bus and licensing. Only the
  API is scraped; there is no OTLP metrics exporter
  ([ADR 0014](../decisions/0014-prometheus-metrics.md)). Catalogue and scrape setup:
  [Observability](../development/observability.md).
- Execution events carry `execution_id`, `agent_id`, `harness`, and timestamps for
  correlation across control and execution planes.
