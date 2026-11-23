<div align="center">
<img src="docs/assets/brand/orbit-logo.png" alt="ORBIT" width="520">

# ORBIT

**Self-hosted control plane for persistent AI agents**

[![CI](https://github.com/opsifyhq/orbit/actions/workflows/ci.yml/badge.svg)](https://github.com/opsifyhq/orbit/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/opsifyhq/orbit)](https://github.com/opsifyhq/orbit/releases)
[![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](./LICENSE)

[Quick start](#quick-start) · [Documentation](./docs/README.md) · [Deployment](./docs/development/deployment.md) · [Architecture](./docs/architecture/overview.md) · [Changelog](./CHANGELOG.md)

</div>

ORBIT lets a team define AI agents once and run them on infrastructure it controls. You
describe an agent: its instructions, its model, the tools it may use and how much it may
do without asking. ORBIT stores every version of that definition, starts runs by hand or
on a schedule, hands each run to a worker, enforces what the agent may touch, pauses for
a human approval where one is required, and keeps the full history.

ORBIT is not an agent framework. The reasoning loop belongs to an existing coding-agent
harness ([OpenCode](https://opencode.ai) and Pi today) that runs in a separate worker
process. ORBIT owns identity, configuration, orchestration, policy and history, and the
API process never executes agent work.

![How ORBIT runs an agent: the control plane decides and remembers, workers do the work, and every tool call passes a policy check](docs/architecture/orbit-overview.svg)

## What you can do with it

| Put an agent to work on | Start it with | What happens |
|---|---|---|
| An issue that should become a pull request | A manual run, a schedule, or an agent team's issue queue | The agent reads the issue, makes the change, runs the tests, pushes a branch and opens a pull request |
| Dependency updates and daily briefings | A cron schedule | Saved instructions run on schedule; every run has its own history |
| Work that needs several roles | An agent team | Agents hand off in sequence or as native subagents; each step is a normal run |
| Monitoring and triage | A schedule | The agent classifies, drafts replies and proposes actions for approval |
| Research and analysis | A manual run | The agent gathers information, summarizes it and marks what it is unsure of |

## Features

- **Versioned agents.** Instructions, model, connectors, skills and limits are stored as
  immutable versions. Agents can be exported and imported as YAML.
- **Autonomy enforced by the platform.** Four levels (Observe, Recommend, Prepare,
  Autonomous), per-connector capability allowlists and tool policies decide what a run
  may do. Prompts are never trusted to enforce permissions.
- **Approvals.** A write that needs sign-off pauses the run until someone approves or
  rejects it in the web app.
- **Agent teams.** A primary agent with subagents, run step by step or as native
  subagents, optionally fed from a GitHub or GitLab issue queue.
- **Workers you control.** Runs execute in a local process, a Docker container or a
  Kubernetes pod. The launcher starts and stops workers and gives each one a scoped,
  revocable token.
- **Brokered tools.** GitHub, GitLab and Kubernetes access goes through a tool broker on
  the worker, so credentials stay out of prompts and out of the browser.
- **Model providers.** OpenAI-compatible endpoints, Anthropic, or the harness's own
  provider configuration.
- **Live runs and history.** A streaming timeline per run, schedules, skills, per-agent
  memory and outputs to webhooks, Slack, Mattermost and Telegram.
- **Administration.** OIDC single sign-on, roles and permissions, API tokens and an audit
  log.
- **Built to be operated.** A responsive web app that works on a phone, a documented
  REST API (OpenAPI) with a generated TypeScript client, a Helm chart and documentation
  served by your own installation.

## Status

ORBIT is pre-1.0. Minor releases may change the API and the database schema; read the
[changelog](./CHANGELOG.md) before you upgrade.

| Area | Works today | Not yet |
|---|---|---|
| Harnesses | Mock, OpenCode (tested with v2.3.0), Pi (tested with v1.5.2) | Claude Code, generic harness |
| Starting a run | Manual, cron schedule, issue queue of an agent team | Event and webhook triggers (they can be saved but do not fire) |
| Connectors | GitHub, GitLab and Kubernetes through the tool broker; X post search and posting | Telegram connector invocation, generic webhook, OAuth sign-in flows for connectors |
| Where runs execute | A local worker, Docker, Kubernetes (long-lived workers or a Job per run) | |
| Enterprise features | Custom roles, access teams, resource sharing ([licensed](#editions-and-license)) | Shared connections, quotas, marketplace |

GitHub read access and approvals are verified end to end. GitHub writes are implemented
but have not been verified against a live repository yet.

## Quick start

You need Docker with Compose v2, `curl` and `jq`. The first build compiles the Rust
services and takes a few minutes.

```bash
git clone https://github.com/opsifyhq/orbit.git && cd orbit
cp .env.example .env
touch .orbit-auth-export.json   # mounted by the launcher; only OpenCode workers need real content

# 1. Database, NATS and API. The worker token does not exist yet, so pass a placeholder.
ORBIT_WORKER_TOKEN=pending docker compose --env-file .env -f deploy/compose/compose.yaml \
  up -d --build db nats api

# 2. Issue a worker token for the mock harness and save it.
TOKEN=$(curl -sf --retry 30 --retry-delay 2 --retry-connrefused \
  -X POST http://localhost:8080/api/v1/worker-tokens \
  -H 'Authorization: Bearer orbit-dev-token' \
  -H 'content-type: application/json' \
  -d '{"name":"local-mock","harnesses":["mock"]}' | jq -r .token)
echo "ORBIT_WORKER_TOKEN=$TOKEN" >> .env

# 3. Worker, launcher and web app.
docker compose --env-file .env -f deploy/compose/compose.yaml up -d --build
```

`--env-file .env` is required: Compose looks for `.env` next to the Compose file, not in
the directory you run it from.

| | |
|---|---|
| Web app | http://localhost:3000 |
| API | http://localhost:8080/api/v1 |
| API reference | http://localhost:8080/docs |

The stack seeds demo agents that use the mock harness, so nothing needs a model key.
Choose **Start work** in the sidebar, pick a demo agent and watch the run's live
timeline. To run a real harness, continue with
[workers and profiles](./docs/guide/worker-profiles.md) and
[model providers](./docs/guide/providers.md).

This stack uses development authentication: anyone who can reach it is signed in as the
owner. Keep it on your own machine. [Run ORBIT locally](./docs/development/local.md)
covers day-to-day development, including running the services without containers.

## Deploy on Kubernetes

Each release publishes container images to `ghcr.io/opsifyhq/orbit` and the Helm chart as
a release asset, so a test installation needs no build:

```bash
ORBIT_VERSION=0.28.0
helm upgrade --install orbit \
  "https://github.com/opsifyhq/orbit/releases/download/v${ORBIT_VERSION}/orbit-${ORBIT_VERSION}.tgz" \
  --namespace orbit --create-namespace \
  --set ingress.host=orbit.example.com \
  --set auth.acknowledgeInsecureDev=true
```

This installs the API, the web app, the worker launcher and a bundled PostgreSQL and
NATS, with development authentication. On first start the API seeds a mock worker
profile; start it under **Workers** and the demo agents can run. No worker token is
needed, because the API issues one per launched worker and revokes it when the worker
stops.

`auth.acknowledgeInsecureDev=true` accepts that anyone who can reach the ingress is
signed in as the owner. Use it only on a throwaway cluster. For production, start from
[`values-prod.example.yaml`](./deploy/helm/orbit/values-prod.example.yaml): OIDC, TLS, an
external database and NATS, and secrets you manage yourself. The details are in the
[deployment guide](./docs/development/deployment.md) and the
[chart README](./deploy/helm/orbit/README.md). To build and push the images to your own
registry, use `scripts/build-images.sh` and `scripts/push-images.sh`.

## Documentation

The handbook lives in [`docs/`](./docs/README.md) and is also served inside the web app
under **Documentation**.

| If you want to | Read |
|---|---|
| Learn the concepts | [How ORBIT fits together](./docs/guide/README.md) |
| Build agents, teams, skills and policies | [Agents](./docs/guide/agents.md), [agent teams](./docs/guide/agent-teams.md), [skills](./docs/guide/skills.md), [tool policies](./docs/guide/tool-policies.md) |
| Run and supervise work | [Start work](./docs/guide/start-work.md), [executions](./docs/guide/executions.md), [approvals](./docs/guide/approvals.md), [connections](./docs/guide/connections.md) |
| Install, configure and upgrade | [Deployment](./docs/development/deployment.md), [single sign-on](./docs/development/oidc.md), [roles and permissions](./docs/guide/rbac.md) |
| Understand the design | [Architecture](./docs/architecture/overview.md), [diagrams](./docs/architecture/diagram.md), [decision records](./docs/decisions/README.md) |
| Review the security model | [Threat analysis](./docs/security/threat-analysis.md), [access matrix](./docs/security/idor-matrix.md) |
| Call the API | `/docs` on your installation, or [`packages/api-client/openapi.json`](./packages/api-client/openapi.json) |

## Security

- Autonomy levels, capability allowlists and tool policies are enforced by the platform
  and fail closed. They do not depend on what a prompt says.
- Provider and connector credentials are encrypted at rest with AES-256-GCM under a
  master key (`ORBIT_MASTER_KEY`). The API accepts them but never returns them, and they
  are kept out of logs and events.
- The control plane never runs agent work. Runs execute on workers, and in production in
  isolated containers or Kubernetes Jobs.
- Content an agent reads from outside is treated as untrusted. Trusted instructions
  travel separately and every input carries its provenance.
- `.env.example` contains development-only keys and tokens. Replace all of them before
  ORBIT is reachable by anyone else.

Please do not report a suspected vulnerability in a public issue. Contact the maintainers
privately first.

## Development

[Run ORBIT locally](./docs/development/local.md) explains how to run the API, a worker
and the web app without containers. [`CONTRIBUTING.md`](./CONTRIBUTING.md) describes how
to propose a change, and [`AGENTS.md`](./AGENTS.md) is the operating manual for
contributors and coding agents: the rules that are not negotiable, how to add a harness,
provider or connector, and which tests a change needs.

The checks CI runs:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
ORBIT_TEST_DATABASE_URL=postgres://orbit:orbit@localhost:5432/orbit cargo test --workspace
pnpm --filter @orbit/web typecheck && pnpm --filter @orbit/web test && pnpm --filter @orbit/web build
pnpm api:generate --check
helm lint deploy/helm/orbit
```

Rust 1.100 is pinned in `rust-toolchain.toml` (minimum supported version 1.94). The web
app needs Node 20 or newer and the pnpm version pinned in `package.json` (`corepack enable`
installs it).

```
apps/web/               Next.js web app (App Router, React, Tailwind)
packages/api-client/    OpenAPI document and generated TypeScript client
packages/contracts/     Zod agent-definition schemas used by web (and a future mobile app)
services/api/           orbit-api: the control plane's HTTP server
services/worker/        orbit-worker: runs harnesses, brokers tools
services/launcher/      Starts and stops workers on Docker and Kubernetes
services/mcp-broker/    MCP endpoint that exposes brokered tools to a harness
services/license-cli/   Issues and inspects offline licenses
crates/orbit-*/         domain · contracts · store · events · security · harness · kubernetes · api
deploy/                 Dockerfiles, Compose stacks and the Helm chart
docs/                   Guide, architecture, decision records, security, development
```

## Editions and license

The source code is licensed under [Apache-2.0](./LICENSE). ORBIT runs as the community
edition by default, with no account, activation or outbound call. An offline enterprise
license unlocks custom roles, access teams and resource sharing; see
[licensing](./docs/guide/licensing.md). The ORBIT name and logo are covered by the
[trademark policy](./TRADEMARK.md).
