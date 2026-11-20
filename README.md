<div align="center">
<img src="docs/assets/brand/orbit-logo.png" alt="ORBIT" width="520">
</div>

# ORBIT — Open Agent Platform

<div align="center">

[![CI](https://github.com/opsifyhq/orbit/actions/workflows/ci.yml/badge.svg)](https://github.com/opsifyhq/orbit/actions/workflows/ci.yml)
[![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](./LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.94%2B-orange.svg)](https://www.rust-lang.org/)
[![Next.js](https://img.shields.io/badge/Next.js-16-black.svg)](https://nextjs.org/)

**Self-hosted control plane for persistent AI agents.**

Create agents, connect them to GitHub/GitLab, run them on Kubernetes or Docker,
and govern every action with policy-enforced approvals.

[Documentation](./docs/README.md) · [Architecture](./docs/architecture/overview.md) · [Deployment](./docs/development/deployment.md) · [Releases](https://github.com/opsifyhq/orbit/releases)

</div>

ORBIT is a self-hosted **control plane** for creating, configuring, connecting, deploying
and governing persistent AI agents. It is deliberately *not* another agent framework:
agent reasoning runs in dedicated harness processes behind a stable adapter contract,
while ORBIT owns identity, configuration, orchestration, policy and history.

```
Browser / future mobile app
        │  REST /api/v1 + SSE (OpenAPI contract)
┌───────▼────────────────────────────────────────────────┐
│ Control plane: orbit-api (Axum)                        │
│   agents · executions · providers · connectors · policy │
│   PostgreSQL (durable state) · NATS JetStream (events)  │
└───────▲────────────────────────────────────────────────┘
        │ HarnessAdapter contract
┌───────▼────────────────────────────────────────────────┐
│ Execution plane: orbit-worker                          │
│   mock (working) · opencode (available, v2.3.0)        │
│   pi (available, v1.5.2) · claude_code / generic (planned)│
└─────────────────────────────────────────────────────────┘
```

## Architecture

![ORBIT architecture overview](docs/architecture/orbit-overview.svg)

Detailed diagrams (system context, execution sequence, broker policy path, domain model,
deployment topology): [`docs/architecture/diagram.md`](docs/architecture/diagram.md).

## What can you do with ORBIT?

| Put an agent to work on… | Start it with… | What happens |
|---|---|---|
| **An issue → a pull request** | Manual, schedule, webhook, or queue | Agent reads the issue, implements the fix, runs tests, commits, pushes, opens a PR |
| **Dependency updates and daily briefings** | A cron schedule | Run saved instructions on a schedule, with separate history per run |
| **Agent Teams** | Manual or auto-dispatch from GitHub/GitLab issues | Sequential or native subagent handoffs, each step is a normal execution |
| **Monitoring and triage** | Event triggers (Telegram, webhooks) | Agent classifies, drafts replies, proposes actions for approval |
| **Research and analysis** | Manual execution | Agent gathers info, summarizes, marks uncertainty |

## Five answers describe the work

| | Your choice |
|---|---|
| **When** | Manual, schedule (cron), event, webhook, or connector-triggered |
| **Where** | Local worker, Docker, or Kubernetes — auto-routed by placement |
| **Who** | A single agent or an Agent Team (primary + subagents) |
| **What** | A task, repository, connectors, and skills |
| **Then** | Succeed, fail, retry, or stay running as a 24/7 agent |

## Quick start (Docker Compose)

Workers authenticate with a scoped token, so start the API first and issue one:

```bash
cp .env.example .env
# 1) infrastructure + API
docker compose -f deploy/compose/compose.yaml up -d --build db nats api

# 2) issue a worker token for the mock harness and save it
TOKEN=$(curl -s -X POST http://localhost:8080/api/v1/worker-tokens \
  -H 'Authorization: Bearer orbit-dev-token' \
  -H 'content-type: application/json' \
  -d '{"name":"local-mock","harnesses":["mock"]}' | jq -r .token)
echo "ORBIT_WORKER_TOKEN=$TOKEN" >> .env

# 3) start the worker and web app
docker compose -f deploy/compose/compose.yaml up -d
```

- Web UI: http://localhost:3000
- API: http://localhost:8080/api/v1 · docs: http://localhost:8080/docs
- PostgreSQL: `localhost:5432` (`orbit`/`orbit`, db `orbit`)
- NATS: `localhost:4222` (loopback only; the monitor port is not published)

The stack seeds demo agents (mock harness). Open an agent → **Run agent** → watch the
live execution timeline, or use **Work** to create a run through the wizard.

## OpenCode + X quick start

1. In **Connections**, add an X connection with a bearer token and optional
   `config.query` (for example `{"query":"#ORBIT"}`), selecting `posts.search`.
2. Create an agent with harness **OpenCode**, a harness-managed model
   (`opencode-go/<model>`), and the X connector bound to that connection with
   `posts.search`.
3. Run it with a task such as "Summarize recent X posts" and optionally
   `{"query":"#ORBIT"}` as payload.

OpenCode credentials are provided to the execution plane via an `opencode auth export`
Secret (see `docs/development/deployment.md`); credentials never pass through prompts or
the browser.

## Kubernetes (Helm)

```bash
# build and push linux/amd64 images to your registry
REGISTRY=registry.example.com:5000 TAG=<version> ./scripts/build-images.sh
PUSH_REGISTRY=registry.example.com:5000 TAG=<version> ./scripts/push-images.sh

# deploy (ingress host defaults to orbit.example.com)
helm upgrade --install orbit deploy/helm/orbit \
  --namespace orbit --create-namespace \
  -f deploy/helm/orbit/values.yaml \
  --set imageRegistry=registry.example.com:5000 \
  --set ingress.host=orbit.example.com \
  --set auth.acknowledgeInsecureDev=true
```

No worker token is needed: the launcher is enabled by default, a launchable mock worker
profile is seeded on first start, and the API mints a scoped token per launch (revoked on
stop). Tokens (`POST /api/v1/worker-tokens`) are only for custom/external workers or a
fixed worker Deployment (`worker.enabled=true`). `./scripts/deploy.sh` wraps the same
flow.

`values.yaml` bundles PostgreSQL and NATS for test clusters and uses dev auth. For
production, start from `deploy/helm/orbit/values-prod.example.yaml` (external DB/NATS,
OIDC, TLS, externally managed secrets, pinned tags). The `auth.acknowledgeInsecureDev=true`
flag above is only for throwaway local/test clusters; use OIDC for anything reachable
beyond your LAN.

Ingress routes: `/docs`, `/healthz`, `/readyz` → API; `/api` → web (same-origin proxy
that injects the dev token server-side or forwards OIDC/API bearer headers); `/` → web.
SSE-friendly annotations are included (`proxy-buffering: off`, long read/send timeouts).

Dev auth mode is enabled by default for local/test deployments. The web proxy injects
the dev token server-side, so do not expose a dev-auth deployment beyond your LAN
until OIDC is configured.

## Native development

```bash
docker compose -f deploy/compose/compose.dev.yaml up -d
cp .env.example .env

cargo run -p orbit-api-server      # terminal 1
cargo run -p orbit-worker          # terminal 2
pnpm install
pnpm --filter @orbit/web dev       # terminal 3 (proxies /api to localhost:8080)
```

## Tests

```bash
cargo test --workspace
ORBIT_TEST_DATABASE_URL=postgres://orbit:orbit@localhost:5432/orbit cargo test --workspace
pnpm --filter @orbit/web typecheck && pnpm --filter @orbit/web build
helm lint deploy/helm/orbit
helm template orbit deploy/helm/orbit -n orbit --set auth.acknowledgeInsecureDev=true >/dev/null
API_URL=http://localhost:8080 ./scripts/smoke-k8s.sh
```

## Repository layout

```
apps/web/                 Next.js web app (App Router, Tailwind, shadcn-style UI)
packages/api-client/      OpenAPI-generated typed client
packages/contracts/       Shared Zod schemas (web + future Expo app)
services/api|worker/      Rust binaries
crates/orbit-*/           domain · contracts · store · events · security · harness · api
deploy/docker/            Dockerfiles
deploy/compose/           Local stacks
deploy/helm/orbit/        Helm chart
docs/                     Architecture, ADRs, development and deployment guides
```

## Security

- Autonomy levels 1–4 are enforced by the platform, not by prompts.
- Provider/connector credentials are AES-256-GCM encrypted at rest with a master key
  (`ORBIT_MASTER_KEY`), write-only through the API, and never logged or serialized.
- The control plane never executes agent tasks; execution belongs to the worker and,
  in production, isolated harness containers/Jobs.
- External content is untrusted: trusted system instructions are transported separately
  with provenance metadata.
- Never commit real credentials; `.env` is ignored and the example key is development-only.

## Documentation

- `docs/architecture/overview.md` — layers, modules, flows
- `docs/architecture/domain-model.md` — entities and invariants
- `docs/architecture/runtime-contract.md` — adapter contract and implementation status
- `docs/architecture/provider-connector-contract.md`
- `docs/architecture/api.md` — REST/OpenAPI/SSE/auth conventions
- `docs/architecture/events.md` — event bus, durability, triggers
- `docs/decisions/` — ADRs
- `docs/development/local.md`, `docs/development/testing.md`, `docs/development/deployment.md`
- `AGENTS.md` — contributor and AI-agent operating manual

## License

Apache-2.0.
