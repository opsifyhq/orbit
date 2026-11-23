# AGENTS.md — working in the ORBIT repository

This file is the operating manual for AI coding agents and human contributors.

## What ORBIT is (and is not)

ORBIT is a self-hosted **control plane** for persistent AI agents. It manages agent
definitions, harnesses, model providers, connectors, triggers, executions and policy.

- ORBIT is **not** an agent framework and must not implement an agent reasoning loop.
- Agent execution belongs to harness adapters running in the **execution plane**
  (`orbit-worker` and, later, isolated containers/Jobs).
- `orbit-api` must never execute agent tasks.

## Repository map

```
apps/web/              Next.js + React + Tailwind web app (App Router)
packages/api-client/   Generated OpenAPI document and TypeScript client (do not hand-edit openapi.json or schema.d.ts)
packages/contracts/    Zod agent-definition schemas used by web (and a future mobile app)
services/api/          orbit-api binary (control plane HTTP)
services/worker/       orbit-worker binary (execution plane supervisor; also `orbit-worker mcp-broker`)
services/launcher/     orbit-launcher binary (starts worker pods, Jobs and containers for the API)
services/license-cli/  orbit-license-cli (vendor-side licence signing tool, not in runtime images)
crates/orbit-domain/   Pure domain types + validation + autonomy policy
crates/orbit-contracts/ Harness/provider/connector/event contracts (traits + DTOs)
crates/orbit-store/    PostgreSQL repositories + SQL migrations
crates/orbit-events/   Event bus (memory + NATS JetStream)
crates/orbit-kubernetes/ Kubernetes connector client (inspection and controlled recovery for agents)
crates/orbit-security/ Auth abstractions, permissions, secret encryption
crates/orbit-harness/  Harness registry + adapters (mock, opencode and pi available)
crates/orbit-api/      HTTP router, handlers, OpenAPI, SSE
deploy/                Compose, Docker files, Helm chart, seed data
scripts/               Image build/push, deploy, version bump/check, OpenAPI and dev-worker scripts
docs/                  Handbook (guide/), architecture, ADRs, development, security and deployment
                       docs (sidebar: docs/navigation.yaml)
```

## Commands

```bash
# backend
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
ORBIT_TEST_DATABASE_URL=postgres://orbit:orbit@localhost:5432/orbit cargo test --workspace

# database (SQLx CLI)
export DATABASE_URL=postgres://orbit:orbit@localhost:5432/orbit
sqlx migrate add <name> --source crates/orbit-store/migrations
sqlx migrate run --source crates/orbit-store/migrations

# frontend
pnpm install
pnpm --filter @orbit/web dev
pnpm --filter @orbit/web typecheck
pnpm --filter @orbit/web test        # web unit tests (node --test lib/*.test.ts); `pnpm test` runs all packages
pnpm --filter @orbit/web build
pnpm api:generate            # openapi.json + schema.d.ts from the Rust code (no running API needed)
pnpm api:generate --check    # what CI runs for schema.d.ts
pnpm docs:check              # links, navigation.yaml and repository paths in docs (Node only)
./scripts/check-versions.sh  # Rust, Node and chart versions must match (CI gate)
cargo audit                  # CI also runs pnpm audit --audit-level=high

# containers + Kubernetes (mkd.local conventions)
# Compose needs --env-file .env and ORBIT_WORKER_TOKEN; see docs/development/local.md
docker compose --env-file .env -f deploy/compose/compose.yaml up --build
TAG=0.1.0 ./scripts/build-images.sh
PUSH_REGISTRY=registry.mdk.local:5000 TAG=0.1.0 ./scripts/push-images.sh
KUBE_CONTEXT=microk8s NAMESPACE=orbit TAG=0.1.0 ./scripts/deploy.sh
helm lint deploy/helm/orbit
helm template orbit deploy/helm/orbit -f deploy/helm/orbit/values.yaml
```

- Registry: `192.168.1.201:5000` (reachable as `registry.mdk.local:5000` for Docker).
  Cluster image references use the IP form; repository paths are identical.
- Target cluster: microk8s context, ingress `orbit.mdk.local`, ingress class `nginx`,
  namespace `orbit`. Images must be `linux/amd64`.
- If `kubectl` reports `dial tcp 192.168.1.6:16443: connect: no route to host` while
  `curl https://192.168.1.6:16443/version` works, macOS Local Network privacy is blocking
  the kubectl binary. Enable it in System Settings → Privacy & Security → Local Network
  (or deploy from an allowed terminal). See `docs/development/deployment.md` for a
  loopback-relay workaround. Do not improvise a different cluster.

## Non-negotiable rules

1. **Contracts before adapters.** Change `crates/orbit-contracts` (and its docs) before
   changing an adapter or a handler.
2. **No agent logic in the control plane.** The API persists intent and publishes events.
3. **Autonomy is platform-enforced.** Never rely on prompts for permissions. Use
   `orbit_domain::evaluate_capability` and permission checks.
4. **Secrets are write-only.** Never return plaintext credentials from the API, log them,
   put them in events, or commit them. `SecretLease` refuses serialization by design.
5. **Durable state lives in PostgreSQL.** The event bus is an accelerator, not the source
   of truth. Any bus loss must be recoverable by polling/replay.
6. **Honesty about integrations.** An adapter is only "available" after integration tests
   against the real harness. `mock`, `opencode` and `pi` are available; Claude Code
   and generic are planned. Keep `ImplementationStatus` accurate.
7. **Migrations are append-only.** Never edit an applied migration; add a new one.
8. **UI must be responsive and mobile-first.** No desktop-only tables as the primary
   mobile experience.
9. **Errors use the envelope.** `{ "error": { code, message, details, requestId } }`.
10. **No silent technology substitutions.** Document deviations in `docs/decisions/`.

## Phase status (update this table when a phase completes)

| Phase | Scope | Status |
|---|---|---|
| 0 | Foundation, contracts, DB, API, worker, docs, containers, Helm | Complete (deployed and smoke-tested on microk8s at http://orbit.mdk.local) |
| 1 | Agent CRUD/Studio, OpenCode adapter verified, executions | OpenCode v2.3.0 + Pi v1.5.2 verified; platform-provider mapping done |
| 2 | Connectors invocation, Telegram/GitLab, webhooks, approvals, Claude Code | GitHub, GitLab and Kubernetes run through the tool broker with approvals; GitHub writes need live verification; X `posts.search` hydrates run context and `posts.create` posts through the broker (#265); Telegram **output delivery** works (#268), but Telegram connector tools are not implemented; webhook outputs deliver, while the generic webhook connector is manifest-only; inbound webhooks not started; Claude Code planned |
| 3 | Long-running agents, Pi, memory, scheduling hardening | Pi + skills + worker profiles + memory + scheduling hardening + K8s Jobs mode done |
| 4 | Enterprise RBAC, shared connections, quotas, marketplace | Custom roles, access teams, sharing and offline licensing done; shared connections, quotas and marketplace not started |

## Adding a harness

1. Add/extend `HarnessKind` only if a new family is required.
2. Implement `HarnessAdapter` in `crates/orbit-harness/src/<name>.rs`.
3. Add a descriptor with accurate `HarnessCapabilities` and `ImplementationStatus`.
4. Register it in `HarnessRegistry::builtin()`.
5. Pass `orbit_harness::testing::assert_adapter_contract` in `crates/orbit-harness/tests`.
6. Add a compliance test against the real harness binary/container and document the
   tested version in `docs/architecture/runtime-contract.md`.
7. Never hardcode provider names or model inventories.

## Adding a provider kind

1. Add a `ProviderKind` variant and config struct in `crates/orbit-contracts/src/provider.rs`.
2. Implement config validation and capability flags (explicit, not assumed).
3. Extend worker resolution (`services/worker/src/client.rs` and `crates/orbit-api/src/handlers/worker_api.rs`).
4. Update the Models UI and OpenAPI schemas.
5. Add contract tests for valid/invalid configs.
6. Add the kind to the `providers_kind_check` database constraint with a new migration and to
   the `every_provider_kind_can_be_saved` API test.

## Adding a connector (Phase 2+)

The `Connector` trait in `crates/orbit-contracts/src/connector.rs` is a target contract for
third-party connectors; the built-in connectors are not trait objects.

1. Add a `ConnectorManifest` (slug, auth, capabilities with effects, config schema) to
   `builtin_manifests()` in `crates/orbit-api/src/connectors.rs`; bootstrap seeds it into
   PostgreSQL.
2. Extend `services/worker/src/broker.rs`: add the slug to the binding filter, a tool-spec
   match arm and a `ConnectorClient` match arm, and implement the client in
   `services/worker/src/<name>.rs` using the connection config and leased credential.
3. Check URL-typed config fields in `validate_connection_urls` and, when a probe exists,
   `probe_connection` (`crates/orbit-api/src/handlers/catalog.rs`); use
   `orbit_security::guarded_client` for every HTTP call.
4. Store credentials only as encrypted `secret_ref` handles.
5. Enforce capability allowlists and autonomy in the platform (the broker), not in the client.
6. Add mock/webhook tests in `crates/orbit-api/tests`; never commit real credentials.

## Testing expectations for a change

- Domain rule change → unit tests in `crates/orbit-domain`.
- Store/schema change → migration + integration tests in `crates/orbit-store/tests`.
- API change → handler tests in `crates/orbit-api/tests` + regenerated OpenAPI client.
- Harness change → contract tests + real-harness integration evidence.
- UI change → `pnpm typecheck` + `pnpm build`; keep pages usable at 360px width and follow
  `docs/development/design-system.md` (tokens, primitives, patterns).
- Docs change → `pnpm docs:check`; new pages must be listed in `docs/navigation.yaml`.

## Local requirements

- Rust 1.100.0 (pinned in `rust-toolchain.toml`; CI and Docker use the same version; MSRV is 1.94). Bump all three together
- Node >= 20 and pnpm 12 (pinned by `packageManager`; `corepack enable`)
- Docker + Compose v2
- Helm 3 and (for cluster validation) kubectl + kind/k3d

## Communication style (default)

- Be concise. Lead with the answer; skip chain-of-thought, reasoning narration and
  “let me think” preambles.
- Do not narrate routine tool calls. Report outcomes and only the commands/results that
  matter.
- Prefer short bullets or a small table; expand only when asked for depth.
- No filler, no restating the request, no repeating the plan after executing it.
- When something fails, give the error, the cause and the fix — not a story.
