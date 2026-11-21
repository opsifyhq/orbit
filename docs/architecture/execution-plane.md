# Execution Plane

The execution plane is everything that runs agent work: the worker, the launcher that
starts workers, the harness processes, and the tool broker that sits between a harness and
the outside world. `orbit-api` never runs agent tasks; it persists intent and serves the
worker protocol.

## Processes and privileges

| Process | Binary | Holds | Does not hold |
|---|---|---|---|
| Worker | `orbit-worker` (`services/worker`) | a scoped worker token, `ORBIT_API_URL`, and per execution the leased credentials it fetched | database URL, master key, event bus access, Docker or cluster rights |
| Launcher | `services/launcher` | Docker socket or Kubernetes workload RBAC, a shared secret (`ORBIT_LAUNCHER_TOKEN`) the API uses to call it, and the worker token of each launch only while starting it | the database, the master key, agent data |
| API | `orbit-api` | database, `ORBIT_MASTER_KEY`, secret decryption | Docker socket and workload RBAC |
| Harness | child of the worker | the execution workspace, a scrubbed environment and the broker URL and token of its own execution | the worker token and API URL, connector credentials |
| Tool broker | thread of the worker | one execution-scoped bearer token, the leased connector credential | anything beyond that execution |

The worker is token-only: it claims work, reports events and fetches what it needs
through `/api/v1/worker/*` (see the API page). The launcher is optional; it is the only
component with container or cluster privileges, and it receives already-authorized launch
requests from the API.

## Worker

A worker registers with a heartbeat (`GET /api/v1/workers` lists them), claims
executions it supports (harness and target match), resolves the harness adapter and runs
it. Execution always selects one agent version; the worker fetches the agent spec,
skills, connector grants and the effective tool policies through the token API for each
execution (`GET /api/v1/worker/executions/{id}/context`).

- **Long-lived mode** (the default): a worker polls and runs up to
  `ORBIT_WORKER_CONCURRENCY` executions (default 4).
- **Single-execution mode**: when `ORBIT_SINGLE_EXECUTION_ID` is set, the worker runs that
  one execution, reports `offline` and exits. It handles SIGTERM as a cancel, because it
  is PID 1 in its container.
- **Execution driver** (`ORBIT_EXECUTION_DRIVER`): `process` runs the harness as a child
  process. A `docker` driver runs each execution in a short-lived container started by the
  worker itself, which needs the Docker socket; that is root-equivalent and intended for
  single-user installations.

## Launcher and worker launches

A worker profile describes a worker (harness, image, target, resources). Launching a
profile (`POST /api/v1/worker-profiles/{id}/launch`) creates a `WorkerLaunch`:

1. The API issues a new scoped worker token for the launch (`orbit_wk_…`, bound to the
   profile) and records the launch.
2. The API calls the launcher (`POST /launches`) with the token and the profile's
   settings. The launcher starts the workload and passes the token and profile
   environment through a per-launch Secret (Kubernetes) or an env file (Docker), never
   through the Deployment or Job spec or the command line.
3. The worker heartbeats and appears in the worker list.

Two shapes exist on Kubernetes:

| | Long-lived worker | Single-execution worker |
|---|---|---|
| Created for | a profile (manual launch) | one execution (a team with `autoLaunch` launches one per step) |
| Workload | Deployment, recreated after eviction | Job with `backoffLimit: 0`, a deadline derived from the execution timeout plus the approval budget, and a 300 second TTL |
| Reuse | an existing live launch of the profile is reused | always a new, isolated launch |

Worker pods do not mount a service-account token, run as non-root, and drop all
capabilities.

Token lifecycle: the token is revoked when the launch is stopped, when launching fails,
and by the API's reaper once the single execution of a launch has finished. Stopping or
archiving a launch is done through `POST /api/v1/worker-launches/{id}/{stop,archive}`.

## Tool broker and MCP bridge

When an agent has connector tools or tool policies apply, the worker starts a tool broker
for the execution: an HTTP server on `127.0.0.1` with a random port and a random bearer
token that exists only for that execution.

- `GET /tools` and `POST /tools/call` serve brokered connector tools (GitHub, GitLab,
  Kubernetes). The broker only exposes capabilities that are in both the connector
  manifest and the agent's binding. It checks autonomy, approval mode, grants and tool
  policy rules before every call, creates approvals through the API and waits for the
  decision, and fetches the connector credential from the API only for the call
  (`GET /api/v1/worker/connections/{id}`). The credential never reaches the harness.
- The harness reaches those tools over MCP. The harness spawns a stdio bridge,
  `orbit-worker mcp-broker` by default (`ORBIT_MCP_BROKER_BIN` overrides it), which
  forwards `tools/list` and `tools/call` to the broker with `ORBIT_BROKER_URL` and
  `ORBIT_BROKER_TOKEN`. `services/mcp-broker` builds the same bridge as a separate
  binary, `orbit-mcp-broker`.
- `POST /policy` is the hook for native harness tools. The OpenCode plugin and the Pi
  extension call it before each native tool runs; any deny, error or missing broker blocks
  the call. The decision combines the platform decision (autonomy, approval mode) with
  tool policy rules, and an approval is bound to a hash of the arguments. The runtime
  contract page lists what is enforced for native tools.

## Environment mappings

An environment mapping maps a connection credential or a secret to an environment
variable. Mappings are managed under `/api/v1/env-mappings` and attached to a worker
profile. At execution start the worker calls `GET /api/v1/worker/env`; the API resolves
the profile's mappings, decrypts the referenced credentials and returns name and value
pairs. The worker passes them to the harness as `extra_env` so skill scripts can use
them. Mappings store references only, and the values are never put in events or logs.
Agent Teams can add mappings to every step with `envMappingIds`.
