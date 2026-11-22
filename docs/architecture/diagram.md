# ORBIT architecture diagrams

![How ORBIT runs an agent: people and API clients call orbit-api, which keeps all state in PostgreSQL; workers pull runs with a scoped token, run a harness and pass every tool call through the tool broker](orbit-overview.svg)

The image follows the reader's light or dark colour scheme. A light PNG export is at
[`orbit-overview.png`](orbit-overview.png); regenerate it when the SVG changes.

## Reading the diagram

| Part | What it shows |
|---|---|
| **Web app, API clients** | Everything enters through the versioned REST API (`/api/v1`) with an OIDC session or an API token. Runs stream back as server-sent events. |
| **Control plane** | `orbit-api` holds the definitions, the run queue, schedules, policy, approvals, secrets and the audit log, and runs the background loops. PostgreSQL is the source of truth. NATS JetStream carries event notifications; nothing depends on it for correctness. The API also delivers outputs and reads the issue queues that feed agent teams. |
| **Trust boundary** | The API never calls a worker. A worker holds one scoped, revocable token and uses the worker API to claim runs, fetch what a run needs and report events. It has no database, event bus or master-key access. The API does call the launcher, to start and stop workers. |
| **Execution plane** | `orbit-launcher` starts workers on Docker or Kubernetes. `orbit-worker` runs the harness (OpenCode, Pi, or the mock for testing) in a workspace of its own per run. The harness calls the model provider itself. Connector tools go over MCP to the tool broker, which checks the allowlist, the autonomy level and any approval, then leases the credential for that one call. See [Execution Plane](execution-plane.md). |
| **Outside ORBIT** | Dashed boxes are systems ORBIT talks to but does not run: outputs, issue queues, model providers and connectors. |
| **Numbers 1 to 6** | The life of one run, repeated in the strip under the diagram. |

| Status | Components |
|---|---|
| **Implemented** | API, PostgreSQL schema, NATS/memory bus, mock harness, OpenCode v2.3.0, Pi v1.5.2, X read hydration and posting, SSE, audit, encrypted secrets, tool broker and tool policies, GitHub and GitLab connectors (approval-gated writes), read-only Kubernetes connector, approval UI, Agent Teams (queue, dispatcher, multi-repo, close-the-loop), continuous agent runtime, memory, launcher and K8s Jobs, OIDC, custom roles, access teams and sharing, webhook/Slack/Mattermost/Telegram outputs, offline licensing, custom CA, dark/light mode, env var mappings, scripts in skills, per-agent models, Work wizard |
| **Planned** | Inbound webhooks (webhook and event triggers are stored but never fire), Telegram and webhook connector invocation, large-scale simulation runs, Claude Code adapter |

## System context

```mermaid
flowchart LR
  subgraph Experience["Agent Experience"]
    Web["Web app (Next.js)"]
    Mobile["Future Expo app"]
  end

  subgraph Control["Control Plane"]
    API["orbit-api (Axum)"]
    DB[("PostgreSQL")]
    Bus{{"NATS JetStream"}}
  end

  subgraph Execution["Execution Plane"]
    Launcher["launcher<br/>(optional)"]
    Worker["orbit-worker<br/>mock"]
    OCWorker["orbit-worker-opencode"]
    PiWorker["orbit-worker-pi"]
    Harness["OpenCode / Pi CLI<br/>(isolated workspace)"]
    MCP["mcp-broker<br/>(stdio bridge)"]
    Broker["Policy-enforcing<br/>tool broker"]
  end

  subgraph External["External systems"]
    GitHub["GitHub API"]
    X["X API"]
    More["GitLab · Kubernetes API ·<br/>model providers"]
  end

  Web -->|"REST /api/v1 + SSE"| API
  Mobile -.->|"same API"| API
  API <--> DB
  API -.->|"publish events"| Bus
  API -->|"launch / stop"| Launcher
  Launcher -.->|"starts workers and Jobs"| Worker
  Launcher -.-> OCWorker
  Launcher -.-> PiWorker
  Worker -->|"worker API + token"| API
  OCWorker -->|"worker API + token"| API
  PiWorker -->|"worker API + token"| API
  OCWorker -->|"spawn per execution"| Harness
  Harness <-->|"MCP stdio"| MCP
  MCP <-->|"loopback + execution token"| Broker
  Broker -->|"HTTPS + leased credentials"| GitHub
  Broker -->|"HTTPS + leased credentials"| More
  Broker -->|"X posts.create"| X
  OCWorker -->|"X read hydration"| X
  Broker -.->|"approvals, events, audit (via API)"| API
```

## Execution sequence (manual run with a GitHub write)

```mermaid
sequenceDiagram
  autonumber
  actor User
  participant Web
  participant API as orbit-api
  participant DB as PostgreSQL
  participant Bus as NATS
  participant W as orbit-worker-opencode
  participant OC as OpenCode
  participant B as Tool broker
  participant GH as GitHub

  User->>Web: Run agent + task
  Web->>API: POST /agents/{id}/executions
  API->>DB: insert execution (queued, idempotency key)
  API->>Bus: publish orbit.execution.requested
  API-->>Web: 202 Accepted
  Web->>API: GET /executions/{id}/events/stream (SSE)
  W->>API: claim execution (queued → dispatched)
  W->>API: report running, append events
  W->>B: start execution-scoped broker (token, allowlist)
  W->>OC: opencode run --standalone --format json
  OC->>B: MCP tools/list
  B-->>OC: allowed GitHub tools only
  OC->>B: tools/call github_issues_comment
  B->>API: policy = require approval, create approval
  B->>API: append tool_call event
  Web->>API: GET /approvals (pending)
  User->>Web: Approve
  Web->>API: POST /approvals/{id}/approve
  API->>DB: decision + audit
  B->>API: wait for decision → approved
  B->>GH: POST comment (credential leased from the API for this call)
  GH-->>B: 201 Created
  B-->>OC: tool result
  OC-->>W: NDJSON text/tool/usage events
  W->>API: persist events, complete execution
  API->>Bus: publish orbit.execution.updated
  API-->>Web: SSE snapshot + events + done
```

## Tool broker decision path

```mermaid
flowchart TD
  Call["Harness calls tool via MCP"] --> Token{"Execution token valid?"}
  Token -- no --> Deny401["401 unauthorized + audit"]
  Token -- yes --> Allowed{"Capability in agent +<br/>connection allowlist?"}
  Allowed -- no --> Deny403["403 denied + event + audit"]
  Allowed -- yes --> Effect{"Capability effect"}
  Effect -- read --> Invoke["Lease credential, invoke connector"]
  Effect -- "write / side effect" --> Level{"Autonomy level"}
  Level -- "L4 + allowlisted" --> Invoke
  Level -- "L3 prepare" --> Approval["Create approval, block tool call"]
  Level -- "L1/L2 observe/recommend" --> Deny403
  Approval --> Human{"Human decision"}
  Human -- "rejected / expired" --> Denied["Tool error + audit"]
  Human -- approved --> Invoke
  Invoke --> Result["Tool result → harness + events + audit"]
```

## Domain model

```mermaid
erDiagram
  ORGANIZATION ||--o{ USER : has
  ORGANIZATION ||--o{ AGENT : owns
  ORGANIZATION ||--o{ PROVIDER : configures
  ORGANIZATION ||--o{ CONNECTION : authorizes
  ORGANIZATION ||--o{ TOOL_POLICY : enforces
  ORGANIZATION ||--o{ CUSTOM_ROLE : defines
  ORGANIZATION ||--o{ ACCESS_TEAM : groups
  ORGANIZATION ||--o{ OUTPUT : delivers_to
  ORGANIZATION ||--o{ ENV_MAPPING : maps

  AGENT ||--o{ AGENT_VERSION : versions
  AGENT ||--o| AGENT_RUNTIME : keeps_running
  AGENT ||--o{ TOOL_POLICY : narrows
  AGENT ||--o{ OUTPUT : reports_via
  AGENT ||--o{ TRIGGER : has
  AGENT ||--o{ EXECUTION : runs

  AGENT_VERSION ||--o{ EXECUTION : snapshot
  CONNECTOR ||--o{ CONNECTION : instantiates

  EXECUTION ||--o{ EXECUTION_EVENT : emits
  EXECUTION ||--o{ APPROVAL : may_require

  ORGANIZATION ||--o{ AGENT_TEAM : owns
  AGENT_TEAM ||--o{ TEAM_RUN : runs
  TEAM_RUN ||--o{ TEAM_STEP : has
  AGENT_TEAM ||--o{ TEAM_QUEUE_ITEM : queues
  TEAM_RUN ||--o{ TEAM_QUEUE_ITEM : fulfills
  TEAM_STEP }o--|| EXECUTION : materializes
  ORGANIZATION ||--o{ SKILL : owns
  ORGANIZATION ||--o{ WORKER_PROFILE : defines
  WORKER_PROFILE ||--o{ WORKER_LAUNCH : launches
  WORKER_PROFILE }o--o{ ENV_MAPPING : uses
  AGENT ||--o{ AGENT_MEMORY : remembers

  ORGANIZATION {
    uuid id
    text slug
    text mode
  }
  AGENT {
    uuid id
    text slug
    text name
    text status
    int current_version
  }
  AGENT_VERSION {
    uuid id
    int version
    jsonb spec
    text checksum
  }
  EXECUTION {
    uuid id
    text status
    text trigger_source
    text placement
    jsonb input
    jsonb output
    jsonb error
    jsonb usage
    timestamptz archived_at
  }
  APPROVAL {
    uuid id
    text status
    jsonb requested_action
    uuid decided_by
  }
  CONNECTION {
    uuid id
    text status
    text credential_ref
    text scopes
  }
  AGENT_TEAM {
    uuid id
    jsonb definition
    timestamptz last_poll_at
  }
  TEAM_RUN {
    uuid id
    uuid team_id
    text status
    timestamptz archived_at
  }
  TEAM_STEP {
    uuid id
    int step_index
    text status
    uuid execution_id
  }
  TEAM_QUEUE_ITEM {
    uuid id
    text external_key
    text status
    int attempts
  }
  SKILL {
    uuid id
    text slug
    text content
    jsonb files
    int version
  }
  WORKER_PROFILE {
    uuid id
    text harness_kind
    text target
    jsonb env_mappings
  }
  AGENT_RUNTIME {
    uuid id
    text status
    text task
    int interval_seconds
    timestamptz next_run_at
    int consecutive_failures
  }
  TOOL_POLICY {
    uuid id
    uuid agent_id
    text action
    jsonb matcher
  }
  CUSTOM_ROLE {
    uuid id
    text name
  }
  ACCESS_TEAM {
    uuid id
    text slug
  }
  OUTPUT {
    uuid id
    text kind
    text secret_ref
  }
  ENV_MAPPING {
    uuid id
    text name
    jsonb mappings
  }
```

## Deployment topology

```mermaid
flowchart TB
  subgraph Local["Local / Docker Compose"]
    LWeb["web :3000"] --> LApi["api :8080"]
    LWorker["worker (mock)"]
    LLauncher["launcher<br/>(Docker socket)"]
    LOpencode["worker-opencode"]
    LDb[("postgres")]
    LNats{{"nats -js"}}
    LApi <--> LDb
    LApi <--> LNats
    LWorker -->|"worker API"| LApi
    LOpencode -->|"worker API"| LApi
    LApi --> LLauncher
    LLauncher -.-> LOpencode
  end

  subgraph K8s["Kubernetes (Helm)"]
    Ing["Ingress<br/>orbit.example.com"] --> SvcWeb["Service: web"]
    Ing -->|"/api, /docs, /healthz"| SvcApi["Service: api"]
    WebD["Deployment: web"] --> SvcWeb
    ApiD["Deployment: api"] --> SvcApi
    WorkerD["Deployment: worker<br/>ORBIT_WORKER_HARNESSES=mock"]
    OpencodeD["Deployment: worker-opencode<br/>ORBIT_WORKER_HARNESSES=opencode"]
    PiD["Deployment: worker-pi<br/>ORBIT_WORKER_HARNESSES=pi"]
    LauncherD["Deployment: launcher<br/>(namespaced workload RBAC)"]
    Pg[("PostgreSQL<br/>bundled or external")]
    Nats[("NATS<br/>bundled or external")]
    Secret["Secrets: DB URL, master key,<br/>OIDC, OpenCode auth export"]
    ApiD <--> Pg
    ApiD <--> Nats
    WorkerD -->|"worker API"| SvcApi
    OpencodeD -->|"worker API"| SvcApi
    PiD -->|"worker API"| SvcApi
    SvcApi --> LauncherD
    LauncherD -.->|"creates worker Deployments and Jobs"| OpencodeD
    OpencodeD -.-> Secret
  end
```

## Boundaries that keep the design honest

1. **The control plane never runs agent code.** Only workers and harness containers do.
2. **PostgreSQL is the source of truth.** The event bus is an accelerator, not a queue you can lose.
3. **Policy and approvals live outside the model.** Prompts are not a security boundary.
4. **Credentials are leased by the broker per action**, never embedded in prompts, CLI arguments or tool output.
5. **Agent versions are immutable.** Running executions keep the version they started with.
6. **Workers are harness-scoped.** The mock worker cannot claim OpenCode executions and vice versa.
7. **External content is untrusted.** Trusted instructions travel separately from retrieved data.
