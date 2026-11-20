# Local Development

## Prerequisites
- Docker (Compose v2) and make
- Rust 1.100.0 (pinned via `rust-toolchain.toml`, installed automatically by rustup; MSRV 1.94). Docker images use `rust:1.100-bookworm`; bump both together
- Node.js >= 20 and pnpm 10+ (`corepack enable && corepack prepare pnpm@latest --activate`)

## Fast path (everything in containers)

Create the auth export first; it is gitignored and contains secrets:

```bash
cp .env.example .env
opencode auth export > .orbit-auth-export.json
docker compose -f deploy/compose/compose.yaml up --build -d
docker compose -f deploy/compose/compose.yaml ps
docker compose -f deploy/compose/compose.yaml logs -f --tail=100 api worker web
```

| Service | URL / port | Notes |
|---|---|---|
| Web | http://localhost:3000 | Next.js + PWA; API proxied same-origin |
| API | http://localhost:8080/api/v1 · /docs · /readyz | OpenAPI + Swagger UI |
| PostgreSQL | `localhost:5432` (loopback only) | `orbit`/`orbit`, database `orbit` |
| NATS | `localhost:4222` | JetStream enabled (monitor port 8222 is not published) |
| worker | — | `ORBIT_WORKER_HARNESSES=mock` (scheduler + mock) |

The first `--build` takes a few minutes (Rust release build); later restarts are seconds.
Migrations and demo seeding are automatic. `localhost` is a secure context, so the web
UI (Web Crypto, clipboard, service worker) works over HTTP. Use TLS for a LAN hostname.

Common commands:

```bash
docker compose -f deploy/compose/compose.yaml restart api worker
docker compose -f deploy/compose/compose.yaml down        # keep data
docker compose -f deploy/compose/compose.yaml down -v     # reset DB/NATS volumes
docker compose -f deploy/compose/compose.yaml build api worker web
```

## Fast iteration without rebuilding Rust images

Stop the full stack first (the dev infrastructure file uses the same ports), then use
Compose for infrastructure and run the API/worker/web natively:

```bash
docker compose -f deploy/compose/compose.yaml down
docker compose -f deploy/compose/compose.dev.yaml up -d
cp .env.example .env
# .env uses db:5432 / nats:4222 for containers; for native set:
export ORBIT_DATABASE_URL=postgres://orbit:orbit@localhost:5432/orbit
export ORBIT_NATS_URL=nats://localhost:4222
export ORBIT_HTTP_ADDR=127.0.0.1:8080

cargo run -p orbit-api-server      # terminal 1

# terminal 2: issue a scoped worker token, then run the worker with it
TOKEN=$(curl -s -X POST -H "Authorization: Bearer orbit-dev-token" \
  -H 'content-type: application/json' \
  -d '{"name":"local-mock","harnesses":["mock"]}' \
  http://localhost:8080/api/v1/worker-tokens | jq -r .token)
ORBIT_API_URL=http://localhost:8080 ORBIT_WORKER_TOKEN="$TOKEN" \
  ORBIT_WORKER_HARNESSES=mock ORBIT_WORKER_TARGET=local \
  cargo run -p orbit-worker        # terminal 2
pnpm --filter @orbit/web dev       # terminal 3 (http://localhost:3000)
```

Workers no longer need `ORBIT_DATABASE_URL`, `ORBIT_NATS_URL` or `ORBIT_MASTER_KEY`.
For Compose set `ORBIT_WORKER_TOKEN` in `.env` (see `.env.example`). The optional
launcher runs with the Docker socket (its port is not published on the host; the API
reaches it over the Compose network) and exposes Launch/Stop/Logs in Workers:

```bash
docker compose --env-file .env -f deploy/compose/compose.yaml up -d launcher api
```

If your registry is reachable under both an IP and a DNS name, Docker treats them as
different image references. Set `ORBIT_LAUNCHER_IMAGE_ALIASES` (comma-separated
`from=to` pairs) so the launcher rewrites the image name before starting a container.

For Docker-launched OpenCode/Pi workers, set `ORBIT_OPENCODE_AUTH_HOST_PATH` or
`ORBIT_PI_AGENT_HOST_DIR` to the **host** path the Docker daemon should mount. The
launcher container cannot translate its own mount paths into sibling mounts.

The Next.js dev server proxies `/api/*` to `ORBIT_API_INTERNAL_URL` (default
`http://localhost:8080`).

## Docker container per execution (OpenCode)

For a stronger local isolation story, run the host dispatcher. Each OpenCode execution
starts a **new Docker container** running the worker in single-execution mode (own
workspace, broker, MCP bridge, heartbeat), then is removed.

```bash
# one-time: credentials + local image (arm64 on Apple Silicon, amd64 on Linux)
opencode auth export > .orbit-auth-export.json   # gitignored, contains secrets
make compose-build-opencode

# start the dispatcher (Postgres/NATS/API/web come from Compose)
make worker-docker
```

The mock Compose worker stays running; the host dispatcher claims only `opencode`
executions. Requires Docker socket/CLI access; treat it as a single-user/homelab mode.

## OpenCode + GitHub locally

Keep the compose stack running (PostgreSQL, NATS, API, web, mock worker), then start a
host-side OpenCode worker. It uses the OpenCode CLI/auth installed on your machine and the
MCP bridge binary built from this repo:

```bash
./scripts/dev-worker-opencode.sh
# or: make worker-opencode
```

Then create a GitHub connection (PAT or public read-only) and an agent with harness
**OpenCode** and the GitHub connector. The mock worker stays responsible for mock agents;
the host worker claims only `opencode` executions.

## Native dev loop (recommended for coding)

```bash
# infrastructure only
docker compose -f deploy/compose/compose.dev.yaml up -d
cp .env.example .env

cargo run -p orbit-api-server   # terminal 1
cargo run -p orbit-worker       # terminal 2
pnpm install && pnpm --filter @orbit/web dev   # terminal 3
```

Auth in dev: `ORBIT_AUTH_MODE=dev`, user `dev@orbit.local` with role `owner`.
Set `ORBIT_DEV_AUTH_ALLOW_ANONYMOUS=true` to omit `Authorization` headers.

## Migrations

```bash
cargo install sqlx-cli --no-default-features --features postgres
export DATABASE_URL=postgres://orbit:orbit@localhost:5432/orbit
sqlx migrate add -r <name>          # new migration (in crates/orbit-store/migrations)
sqlx migrate run
sqlx migrate info
```

Migrations are embedded in the binaries and applied automatically at startup
(`ORBIT_DB_AUTO_MIGRATE=true`, default in dev).

## API client generation

```bash
pnpm api:generate            # after any API change
pnpm api:generate --check    # what CI runs for the TypeScript half
```

No running API or database is needed. The command prints the OpenAPI document from the
Rust code (`cargo run -p orbit-api --example openapi`), saves it as
`packages/api-client/openapi.json` and generates `packages/api-client/src/schema.d.ts`
from it. Both files are committed. CI fails when either is stale: a Rust test compares
`openapi.json` with the code (and the router with the document), and the web job
compares `schema.d.ts` with `openapi.json`.

## Useful commands

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                     # unit tests
ORBIT_TEST_DATABASE_URL=postgres://… cargo test --workspace -- --include-ignored
make helm-lint
make compose-config
```
