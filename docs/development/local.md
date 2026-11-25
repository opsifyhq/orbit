# Local Development

## Prerequisites
- Docker (Compose v2) and make
- Rust 1.100.0 (pinned via `rust-toolchain.toml`, installed automatically by rustup; MSRV 1.94). Docker images use `rust:1.100-bookworm`; bump both together
- Node.js >= 20 and pnpm 12 (the version is pinned by `packageManager` in the root
  `package.json`; `corepack enable` installs it)

## Fast path (everything in containers)

Compose does not read `.env` from the repository root when you use
`-f deploy/compose/compose.yaml`, so every command passes `--env-file .env`. The worker
service also refuses to load without `ORBIT_WORKER_TOKEN`, and a token can only be issued
by the running API. Start the infrastructure and the API first, issue a token, then start
everything:

```bash
cp .env.example .env
# The launcher mounts this file; Docker creates a directory if it does not exist.
touch .orbit-auth-export.json   # or: opencode auth export > .orbit-auth-export.json

# 1. Infrastructure and API (the placeholder only lets Compose load the file)
ORBIT_WORKER_TOKEN=pending docker compose --env-file .env \
  -f deploy/compose/compose.yaml up -d --build db nats api

# 2. Issue a scoped worker token and store it in .env
TOKEN=$(curl -sf --retry 30 --retry-delay 2 --retry-connrefused -X POST \
  http://localhost:8080/api/v1/worker-tokens \
  -H 'Authorization: Bearer orbit-dev-token' -H 'content-type: application/json' \
  -d '{"name":"local-mock","harnesses":["mock"]}' | jq -r .token)
echo "ORBIT_WORKER_TOKEN=$TOKEN" >> .env

# 3. Everything else
docker compose --env-file .env -f deploy/compose/compose.yaml up -d --build
docker compose --env-file .env -f deploy/compose/compose.yaml ps
docker compose --env-file .env -f deploy/compose/compose.yaml logs -f --tail=100 api worker web
```

The auth export (`.orbit-auth-export.json`) is gitignored and contains secrets when it
comes from `opencode auth export`. An empty file is enough if you do not launch OpenCode
workers.

| Service | URL / port | Notes |
|---|---|---|
| Web | http://localhost:3000 | Next.js + PWA; API proxied same-origin |
| API | http://localhost:8080/api/v1 · /docs · /readyz | OpenAPI + Swagger UI |
| PostgreSQL | `localhost:5432` (loopback only) | `orbit`/`orbit`, database `orbit` |
| NATS | `localhost:4222` | JetStream enabled (monitor port 8222 is not published) |
| worker | — | `ORBIT_WORKER_HARNESSES=mock` (mock harness only) |
| launcher | internal, port 8090 | Docker socket access; not published on the host |

The first `--build` takes a few minutes (Rust release build); later restarts are seconds.
Migrations and demo seeding are automatic. `localhost` is a secure context, so the web
UI (Web Crypto, clipboard, service worker) works over HTTP. Use TLS for a LAN hostname.

Common commands:

```bash
docker compose --env-file .env -f deploy/compose/compose.yaml restart api worker
docker compose --env-file .env -f deploy/compose/compose.yaml down        # keep data
docker compose --env-file .env -f deploy/compose/compose.yaml down -v     # reset DB/NATS volumes
docker compose --env-file .env -f deploy/compose/compose.yaml build api worker web
```

After `down -v` the issued worker token is gone. Repeat steps 1 and 2 of the fast path and
replace the `ORBIT_WORKER_TOKEN` line in `.env`.

The Makefile targets `make compose-up`, `compose-down`, `compose-logs` and
`compose-reset` run the same commands with `--env-file .env`, and stop with a hint when
`.env` is missing.

## Fast iteration without rebuilding Rust images

Stop the full stack first (the dev infrastructure file uses the same ports; the dev file
needs no `--env-file` because it has no required variables), then use Compose for infrastructure and run the API/worker/web natively:

```bash
docker compose --env-file .env -f deploy/compose/compose.yaml down
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

## Host workers need their own token

`scripts/dev-worker-opencode.sh`, `dev-worker-pi.sh` and `dev-worker-docker.sh` do not set
`ORBIT_WORKER_TOKEN`, and the worker refuses to start without it. The worker also loads
`.env` from the repository root, which after the fast path holds the token for the
`mock` harness. Issue a token for the harness you run and export it before starting the
script (an exported variable wins over `.env`):

```bash
export ORBIT_WORKER_TOKEN=$(curl -sf -X POST http://localhost:8080/api/v1/worker-tokens \
  -H 'Authorization: Bearer orbit-dev-token' -H 'content-type: application/json' \
  -d '{"name":"local-opencode","harnesses":["opencode"]}' | jq -r .token)
```

Use `["pi"]` for the Pi worker.

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

## Pi locally

Run a host-side Pi worker the same way. It uses the Pi CLI and credentials installed on
your machine (`~/.pi/agent`) and claims only `pi` executions:

```bash
./scripts/dev-worker-pi.sh
# or: make worker-pi
```

## The MCP broker binary

Images, the Kubernetes launcher and the host scripts (`dev-worker-opencode.sh`,
`dev-worker-docker.sh`, `dev-worker-pi.sh`) run the tool broker as a subcommand of the
worker (`orbit-worker mcp-broker`); the scripts point `ORBIT_MCP_BROKER_BIN` at
`target/debug/orbit-worker`. There is no separate broker binary. If `ORBIT_MCP_BROKER_BIN`
ends in `orbit-worker` the worker appends `mcp-broker`; otherwise it runs the binary as
given. The default is `/usr/local/bin/orbit-worker`.

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
(`ORBIT_DB_AUTO_MIGRATE`, default `true`).

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

`./scripts/dump-openapi.sh [file]` writes the same document through the `openapi_parity`
test; `ORBIT_OPENAPI_SPEC=<file> pnpm api:generate` generates from such a file.

## Useful commands

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                     # DB tests are skipped without the variable below
ORBIT_TEST_DATABASE_URL=postgres://orbit:orbit@localhost:5432/orbit cargo test --workspace
pnpm test                                  # web unit tests (pnpm --filter @orbit/web test)
pnpm typecheck
make helm-lint
docker compose --env-file .env -f deploy/compose/compose.yaml config   # validate the stack
```

[testing.md](testing.md) explains each test layer and the opt-in suites. Do not add
`-- --include-ignored` to `cargo test`: it also runs the tests that need NATS, a live
Kubernetes cluster or a real OpenCode/Pi install with model credentials.
