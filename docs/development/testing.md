# Testing

This page explains how ORBIT is tested, how to run each layer and which checks CI
enforces. A change is ready when the layers it touches pass locally and the CI gates at
the end of this page would pass.

## Test layers

| Layer | Command | What it covers |
|---|---|---|
| Rust unit tests | `cargo test --workspace` | Domain rules, autonomy and tool-policy matrices, state transitions, security helpers, launcher status mapping, worker git and skill handling, OIDC and licence logic |
| Harness contracts | `cargo test -p orbit-harness` | Every registered adapter passes the shared contract suite (`crates/orbit-harness/tests/contracts.rs`). `process_supervision.rs` runs OpenCode and Pi against shell-script stubs: exit codes, process-group cleanup, symlink-safe workspace writes (Unix only) |
| Store | `ORBIT_TEST_DATABASE_URL=… cargo test -p orbit-store` | CRUD, organization scoping, optimistic version conflicts, execution claim compare-and-set, OIDC identity linking (`crates/orbit-store/tests/`) |
| API | `ORBIT_TEST_DATABASE_URL=… cargo test -p orbit-api` | Router with `tower::ServiceExt::oneshot`: authentication, error envelope, pagination, agent lifecycle, executions, SSE replay, access isolation, portable export and import (`crates/orbit-api/tests/`) |
| OpenAPI drift | part of `cargo test -p orbit-api` | See "OpenAPI drift gates" below |
| Documentation consistency | `pnpm docs:check` | See "Documentation consistency" below; needs only Node |
| Web unit tests | `pnpm --filter @orbit/web test` (or `pnpm test`) | Pure helpers in `apps/web/lib/*.test.ts`, run with `node --test` |
| Web types and build | `pnpm --filter @orbit/web typecheck`, `pnpm --filter @orbit/web build` | Type-safe API use on every page and build-time correctness |
| Helm | `helm lint deploy/helm/orbit`, `helm template …`, `make helm-lint` | Chart validity, required values, probes, external PostgreSQL and NATS |
| Smoke test | `API_URL=http://localhost:8080 ./scripts/smoke-k8s.sh` (or `make smoke-k8s`) | A running stack: health, create an agent, run it, stream events, terminal status. Works against Compose, a port-forward or the ingress |
| UI acceptance | [ui-test-plan.md](ui-test-plan.md) | Manual flows for every screen |

## Database-backed tests

The store and API integration tests need PostgreSQL. They are skipped, not failed, when
`ORBIT_TEST_DATABASE_URL` is unset, so plain `cargo test` works without infrastructure and
can give a false sense of coverage. Run them with a database:

```bash
docker compose -f deploy/compose/compose.dev.yaml up -d db
ORBIT_TEST_DATABASE_URL=postgres://orbit:orbit@localhost:5432/orbit cargo test --workspace
# or: make test-db
```

The API tests apply the migrations themselves and create a fresh organization per test,
so they can share the development database. CI runs the same command against a PostgreSQL 17
service.

### Query plans at execution scale

The test databases are tiny, so plan tests only prove that an index *can* serve a query
(they turn sequential scans off for one plan). To check real plans, seed a scratch
database with every migration applied (never a real one) and run the EXPLAIN script:

```bash
psql "$SCRATCH_URL" -v ON_ERROR_STOP=1 -f scripts/perf/execution-history-seed.sql
psql "$SCRATCH_URL" -f scripts/perf/execution-history-explain.sql
```

The seed writes 1M executions over a year, 200k approvals and 60 runs in flight, then
archives everything older than 30 days as the retention job does. The EXPLAIN script covers
the reapers, deleting a trigger, the active-run count, the dashboard counts, the default
executions list total and the retention job's archive step.

## Opt-in suites

Some tests are marked `#[ignore]` because they need something that CI does not have.
Never add `-- --include-ignored` to a general `cargo test` run.

| Suite | Needs | Command |
|---|---|---|
| NATS event bus (`crates/orbit-events/tests/nats_integration.rs`) | NATS with JetStream. `ORBIT_TEST_NATS_URL` defaults to `nats://localhost:4222` | `cargo test -p orbit-events -- --ignored` |
| Real OpenCode (`crates/orbit-harness/tests/opencode_e2e.rs`) | OpenCode CLI and provider credentials. Set `ORBIT_OPENCODE_E2E=1`; `ORBIT_OPENCODE_TEST_MODEL` overrides the model | `cargo test -p orbit-harness --test opencode_e2e -- --ignored --nocapture` |
| Real Pi (`crates/orbit-harness/tests/pi_e2e.rs`) | Pi CLI and provider credentials. Set `ORBIT_PI_E2E=1`; `ORBIT_PI_TEST_MODEL` overrides the model | `cargo test -p orbit-harness --test pi_e2e -- --ignored --nocapture` |
| Live Kubernetes (`crates/orbit-kubernetes/tests/live.rs`) | A reachable cluster. `ORBIT_TEST_KUBECONFIG` holds the kubeconfig contents; the second test also needs `ORBIT_TEST_KUBECTL_CONTEXT` | `ORBIT_TEST_KUBECONFIG="$(kubectl config view --raw --minify)" cargo test -p orbit-kubernetes --test live -- --ignored` |
| Live X post (`services/worker/src/x.rs`) | A user-context X token with `tweet.write` and `ORBIT_X_LIVE=1`. Publishes a real tweet | `ORBIT_X_LIVE=1 ORBIT_X_USER_TOKEN=… cargo test -p orbit-worker --bin orbit-worker live_post_requires_user_context_token -- --ignored --nocapture` |

The OpenCode and Pi tests send a real prompt to a model, so they spend model credits.
Without their opt-in variable these tests return early and report a pass; read the
output before trusting a green result.

A harness adapter is only called "available" after these tests have passed against the
real CLI. The OpenCode adapter is tested against OpenCode v2.3.0 and the Pi adapter
against Pi v1.5.2 (see [the runtime contract](../architecture/runtime-contract.md)).
Claude Code and the generic adapter are registered as planned and fail closed.

## Documentation consistency

`pnpm docs:check` runs `scripts/check-docs.mjs` (Node 20+, no dependencies, no network).
It fails with a sorted list of problems when:

- a relative Markdown link or image, including a reference definition, points to a missing
  file or a heading that does not exist;
- `docs/navigation.yaml` lists a missing or duplicate page, or omits a served page
  (`docs/planning/` is intentionally unlisted, like the embedded docs);
- a repository path written in inline code no longer exists. Only unambiguous paths are
  checked: root-anchored paths (for example `crates/orbit-api/src/docs.rs`), common
  `apps/web`-relative paths and root files. Glob, placeholder and runtime-generated paths
  are skipped.

The Rust tests in `crates/orbit-api/src/docs.rs` cover links and navigation inside the
embedded handbook; this check also covers root Markdown, the changelog and planning drafts,
and runs without the Rust toolchain. The Web job in CI runs it, so documentation drift
fails pull requests.

## OpenAPI drift gates

`packages/api-client/openapi.json` and `packages/api-client/src/schema.d.ts` are generated
and committed. Three checks keep them in step with the code:

- `openapi_document_matches_the_router` in `crates/orbit-api/src/routes.rs` fails when the
  router serves an operation the document does not describe, or the reverse.
- `committed_openapi_document_is_current` in the same module fails when the committed
  `openapi.json` differs from the document the code produces (the version field is
  ignored; `./scripts/check-versions.sh` keeps it equal to the release version).
- `node scripts/generate-api-client.mjs --check` (also `pnpm api:generate --check`) fails
  when `schema.d.ts` differs from what `openapi.json` generates.

Run `pnpm api:generate` after any API change and commit both files. No running API or
database is needed. `crates/orbit-api/tests/openapi_parity.rs` pins a list of operations
and schemas that drifted in the past; `./scripts/dump-openapi.sh` writes the document
through it (`ORBIT_OPENAPI_DUMP`).

## What CI enforces

`.github/workflows/ci.yml` runs on every pull request and push to `main`, and on `v*`
tags:

| Job | Checks |
|---|---|
| Rust | `./scripts/check-versions.sh`, the tag matches the chart `appVersion` (tags only), `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo audit`, `cargo test --workspace` with PostgreSQL, and the NATS integration tests against a JetStream server |
| Rust MSRV | `cargo check --workspace --all-targets --locked` on the `rust-version` declared in `Cargo.toml` (1.94) |
| Web | `pnpm -r typecheck` (web app, API client and contracts), `pnpm docs:check`, `pnpm --filter @orbit/web test`, the API client check, `pnpm --filter @orbit/web build`, `pnpm audit --audit-level=high` |
| Secret scan | TruffleHog, failing on verified secrets only |
| Helm | `helm lint`, and `helm template` for the default values, `values-prod.example.yaml`, a variant with the network policy and a fixed worker, and one with the ServiceMonitor; every render is validated with `kubeconform -strict` against Kubernetes 1.30 (and the default values without the launcher admission policies against 1.27, the chart's floor). ServiceMonitor, a CRD, is skipped |
| Migrations | Pull requests and pushes to `main`: a migration that already exists may not be edited, renamed or deleted |
| Image build check | Pull requests only: the api, worker, web and launcher Dockerfiles build, and the OpenCode and Pi worker images build on that worker image through a local registry (nothing is published) |

Container images, the Helm chart and the worker binary are published only from `v*`
tags after the Rust, MSRV and web jobs pass, and the chart and the GitHub release only after all six images were pushed; see
[releasing.md](releasing.md). Images are not scanned for vulnerabilities in CI.

## What to run for a change

| Change | Run at least |
|---|---|
| Domain rule | Unit tests in `crates/orbit-domain` |
| Schema or store | A new migration (never edit an applied one) plus a test in `crates/orbit-store/tests`, with `ORBIT_TEST_DATABASE_URL` set |
| API | A handler test in `crates/orbit-api/tests`, then `pnpm api:generate` |
| Harness | Contract tests, and real-harness evidence from the opt-in suites above |
| UI | `pnpm typecheck`, `pnpm test`, `pnpm build`, and the matching flow in the UI test plan at 360px width |
| Docs | `pnpm docs:check`; add a new page to `docs/navigation.yaml` |
| Chart | `make helm-lint` |
