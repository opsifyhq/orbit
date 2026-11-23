# Threat Analysis — ORBIT

## Scope

This document covers the threat model for ORBIT as a self-hosted control plane for AI
agents. It identifies assets, threat actors, attack vectors, the mitigations that exist in
the code today and the risks that remain. Each mitigation names where it is implemented;
anything the platform does not do is listed under "Residual risks".

## Assets

| Asset | Location | Sensitivity |
|---|---|---|
| Provider credentials (API keys) | PostgreSQL (encrypted at rest) | Critical |
| Connection credentials (GitHub/GitLab tokens) | PostgreSQL (encrypted at rest) | Critical |
| Worker profile environment values | PostgreSQL | High |
| Worker tokens | PostgreSQL (hashed) | High |
| Agent definitions | PostgreSQL | Medium |
| Execution history (input/output) | PostgreSQL | Medium |
| Audit logs | PostgreSQL | Medium |
| Master key (`ORBIT_MASTER_KEY`) | Environment variable | Critical |
| Worker tokens (plaintext) | Worker process environment, per-launch Secret | High |

## Threat actors

1. **External attacker**: no credentials, network access only.
2. **Malicious agent**: compromised agent definition or prompt injection.
3. **Insider threat**: authenticated user with excessive permissions.
4. **Supply chain**: compromised dependency or container image.

## Attack vectors and mitigations

### 1. Credential theft

**Vector:** an attacker extracts provider or connection credentials from the database,
the API or the cluster.

**Mitigations:**

- Credentials are encrypted at rest (AES-256-GCM through `SecretCipher`).
- The master key is required to decrypt and lives in the environment, not in the
  database. In production the API refuses to start with the published development key.
- The API never returns plaintext credentials (`SecretLease` refuses serialization).
- Worker profile `env` is write-only: responses list variable names with `hasValue`,
  never the values. On update a string sets a value, `null` keeps the stored one and an
  omitted name is removed.
- Worker tokens are hashed; only the hash is stored.
- A launched worker receives its token and profile environment through a per-launch
  Kubernetes Secret (`secretKeyRef`), or a 0600 `--env-file` for Docker, never in a
  Pod spec or command line. The Secret is owned by the workload and deleted with it.
- A profile's `authSecretName` must match the installer's allowlist
  (`ORBIT_WORKER_AUTH_SECRETS`, Helm `launcher.authSecrets`), checked by the API on
  save and again by the launcher, so a profile editor cannot mount ORBIT's own Secrets.
  Binding a new auth Secret to a profile takes an Owner or Admin; other editors may only
  keep, clear or reuse a Secret already bound to a profile they manage, so one team
  cannot mount another team's harness credentials.
- The launcher's Role cannot be scoped by name, so admission policies
  (`launcher.admissionPolicy.enabled`, Kubernetes 1.30+) limit its ServiceAccount to
  `orbit-worker-<launch>` workloads and Opaque `-env` Secrets, and refuse pods that
  reference platform Secrets, use projected tokens, hostPath, host namespaces or
  privileged containers. A leaked launcher token is therefore not the master key.
- Docker-launched and dispatched containers drop all capabilities, set
  `no-new-privileges`, a PID limit and the non-root `orbit` user, like Kubernetes worker
  pods, and get `host.docker.internal` only when their API URL needs it.
- `/readyz` logs dependency errors instead of returning them.

### 2. Prompt injection

**Vector:** malicious content in issues or emails makes the agent perform unintended
actions.

**Mitigations:**

- Autonomy is platform-enforced (`evaluate_capability`), not prompt-based.
- Capability allowlists are enforced by the tool broker, not the harness.
- Trusted instructions travel separately from run input and external content, and the
  default agent prompts tell the model to treat retrieved content as untrusted data
  (this is guidance to the model, not an enforcement point).
- Write actions require approval at autonomy L3 and are auto-approved only at L4. A team
  run's `autoApprove` never raises the level and needs `approvals:decide`.
- Tool policy layer: rules are additive restrictions. A rule `allow` cannot weaken a
  platform `require_approval` or `deny`, so a prompt-injected agent cannot grant itself a
  capability. Paths are lexically normalised before matching so `../` cannot bypass a
  `secrets/**` rule, and command rules use regexes validated at write time. Native tools
  with no matching rule follow the platform decision (L1/L2 deny writes, L3 requires
  approval, L4 allows allowlisted actions). Rule management requires `settings:write`
  and is audited.
- Policy loading fails closed: a tool policy that cannot be loaded stops the run, an
  unparseable rule denies instead of being skipped, `deny` and `require_approval` rules
  match even when no path can be extracted, and an unknown capability gets no grant.

### 3. Agent privilege escalation

**Vector:** an agent tries to use capabilities it was not granted.

**Mitigations:**

- The tool broker enforces capability allowlists per execution.
- `evaluate_capability()` checks the autonomy level, approval mode and grants.
- Agent definitions can only be changed through the user API, which needs
  `agents:write` and object-level access. A worker token reaches only the
  `/api/v1/worker/*` routes.
- Worker tokens are scoped to harness kinds and, optionally, to a worker profile.
- Approval decisions are bound to the execution, expire with it and default to deny on
  timeout.

### 4. Data exfiltration via agent

**Vector:** an agent reads sensitive data and sends it to an external service.

**Mitigations:**

- Connectors are allowlisted per agent binding.
- External side effects require approval at L3.
- The broker records every brokered tool call: the audit log keeps the capability, the
  decision and the approval id, and the execution event stream keeps the arguments
  (truncated to 4,000 characters) and the result (truncated to 8,000 characters).
- Execution output is stored but not automatically shared.

### 5. Container escape and workspace tampering (worker)

**Vector:** a compromised agent escapes the worker container or writes outside its
workspace.

**Mitigations:**

- Workers run as non-root (uid 10001) with all capabilities dropped and privilege
  escalation disabled.
- Per-execution isolation: Kubernetes Jobs, or a Docker container per execution.
- Workers have no database access (token-only API) and launched pods do not mount a
  Kubernetes service account token.
- Workspace paths (`workspaceSubdir`, `sessionDir`) are relative and confined to the
  execution workspace. Files the platform writes into a workspace (skills, subagent
  files) are never written through a symlink that the cloned repository shipped.
- Cancel and timeout stop the harness's whole process group (SIGTERM, then SIGKILL after
  a grace period), so nothing outlives a run.
- Worker-side git (`repo.checkout`, `commit`, `push`) runs through one builder that
  clears the environment to an allowlist (no `ORBIT_WORKER_TOKEN` or `ORBIT_API_URL`),
  disables system and global git config, overrides hooks, fsmonitor, ssh command and
  external diff, uses `--no-verify`, and refuses to run when the workspace's git config
  defines filter, textconv, merge-driver, credential, include or other program-executing
  entries (#195).
- CPU and memory limits are applied from the worker profile.

### 6. Server-side request forgery (outbound URLs)

**Vector:** a user with write access to providers, connections, outputs or work sources
points ORBIT at an internal address (cloud metadata, loopback, a management interface),
or at a host they control to collect the stored credential.

**Mitigations:**

`crates/orbit-security/src/http.rs` provides `check_outbound_url` (literal hosts and
resolved addresses) and a DNS-guarded HTTP client that applies the same policy to every
lookup and refuses cross-host redirects. Every call site that uses a URL from stored or
user-supplied config is covered:

| Call site | URL source | Checked? |
|---|---|---|
| `probe_endpoint` (`crates/orbit-api/src/handlers/catalog.rs`) | provider `baseUrl` and its models endpoint | Yes, on each probe |
| `probe_connection` / `probe_supported_connection` (`crates/orbit-api/src/handlers/catalog.rs`) | connection `baseUrl` or `url` | Yes, on each test; URL fields are also checked when saved |
| Webhook delivery (`crates/orbit-api/src/handlers/outputs.rs`) | write-only webhook URL | Yes, on save and on each delivery |
| `send_telegram` (`crates/orbit-api/src/handlers/outputs.rs`) | `api.telegram.org`, or `ORBIT_TELEGRAM_API_BASE_URL` set by an operator | Yes, on each send; the token is confined to the URL path |
| Work-source dispatcher (`crates/orbit-api/src/dispatcher.rs`, `crates/orbit-api/src/work_sources.rs`) | connection `baseUrl` | Yes, before each dispatch |
| Team outcome delivery (`crates/orbit-api/src/team_outcomes.rs`) | connection `baseUrl` | Yes, on each delivery |
| Kubernetes client (`crates/orbit-kubernetes/src/lib.rs`) | API server / kubeconfig `server` | Yes, before each request; only Kubernetes `Status` messages are forwarded, never other response bodies |
| X context hydration (`services/worker/src/connectors.rs`) | connection `baseUrl` | Yes, on each hydration |
| GitHub, GitLab and X broker clients (`services/worker/src/{github,gitlab,x}.rs`, using the client from `services/worker/src/broker.rs`) | connection `baseUrl` | Yes: the guarded client checks DNS on every request; the base URL is also checked when the connection is saved |
| Worker git clone/push (`services/worker/src/git.rs`) | connection `cloneBaseUrl` | Checked when the connection is saved, not again at use time (the request is made by the `git` binary) |

The remaining HTTP callers use operator-supplied internal URLs, not agent-editable
config, and are deliberately outside the outbound policy:

- OIDC discovery (`crates/orbit-api/src/handlers/auth_oidc.rs`) and JWKS fetches
  (`crates/orbit-security/src/auth.rs`) use `client_with_custom_ca` with the issuer/JWKS
  URL from `ORBIT_OIDC_ISSUER` / `ORBIT_OIDC_JWKS_URL`; both are cached and rate-bounded.
- Launcher calls (`crates/orbit-api/src/handlers/worker_launches.rs`) use
  `ORBIT_LAUNCHER_URL`; worker → API calls (`services/worker/src/client.rs`) use
  `ORBIT_API_URL`.
- The MCP bridge (`services/worker/src/mcp_broker.rs`, `services/mcp-broker/src/main.rs`)
  calls `ORBIT_BROKER_URL`, which the worker sets to its loopback tool broker.

A stored credential never follows an edited endpoint: changing a provider's base URL or
one of the connection URL fields (`baseUrl`, `cloneBaseUrl`, `url`, `apiServer`) requires
the credential in the same request. Loopback, link-local, cloud-metadata, unspecified,
multicast and reserved addresses are blocked; metadata addresses can never be allowlisted.

Private ranges (RFC 1918, CGNAT, IPv6 ULA) are allowed by default so self-hosted model
  servers work. `ORBIT_OUTBOUND_ALLOW_PRIVATE=false` blocks them, and
  `ORBIT_OUTBOUND_ALLOWED_HOSTS` lists exceptions (Helm: `outbound.*`).

### 7. Supply chain compromise

**Vector:** a compromised dependency or container image.

**Mitigations:**

- Rust dependencies are audited with `cargo audit` and Node dependencies with
  `pnpm audit --audit-level=high` in CI; both fail the build.
- A TruffleHog scan fails the build on verified secrets.
- `Cargo.lock` and `pnpm-lock.yaml` pin versions, and third-party GitHub Actions are
  pinned to commit SHAs.
- Multi-stage image builds keep build tools out of the runtime images.
- Release images are published to `ghcr.io/opsifyhq/orbit`.

Container images are **not** scanned for vulnerabilities in CI; see "Residual risks".

### 8. Unauthorized access to the control plane

**Vector:** an attacker accesses the API without authentication.

**Mitigations:**

- Every API route requires authentication (dev token, API token or OIDC bearer token)
  except `/healthz`, `/readyz`, `/metrics`, `/api/v1/meta`, the two OIDC login endpoints,
  `/docs` and the OpenAPI document. Worker routes use worker tokens. Keep
  `/api/v1/meta` (which reports the licence tier and customer) off untrusted networks if
  that matters to you.
- `/metrics` is unauthenticated by default and reveals operational detail: the version,
  the API's route templates and request rates, execution and worker counts, the licence
  phase, usage and limits. It carries no organization, user, agent or execution
  identifiers, no names and no secrets. The Helm chart's ingress does not route it. To
  restrict it further, set `ORBIT_METRICS_TOKEN` (bearer token, compared as SHA-256
  digests; 401 without it) and/or `ORBIT_METRICS_ADDR` (dedicated listener that serves only
  `/metrics` and `/healthz`; the API port then returns 404 for `/metrics`), and limit the
  port with `networkPolicy.enabled` and `monitoring.allowFrom`. The API logs a warning at
  start when it runs in production with neither set. The dedicated listener has no rate
  limit, so do not publish it. See
  [Observability](../development/observability.md).
- Dev auth mode is refused when `ORBIT_ENV=production`, and the Helm chart refuses it
  when `platform.environment=production`.
- OIDC mode validates JWTs against the provider's JWKS. Discovery and JWKS fetches are
  cached and refreshed at a bounded rate, so unauthenticated requests cannot be turned
  into a stream of calls to the identity provider.
- Unauthenticated routes are rate limited per client. Behind the web proxy and the
  ingress every request has the same TCP peer, so the limit keys on `X-Forwarded-For`
  only for peers in `ORBIT_TRUSTED_PROXIES` (the chart trusts the private ranges by
  default). A client inside a trusted range can choose its own key; narrow the setting
  to the pod CIDR where clients share those ranges. Buckets are in-memory per API
  process, not shared between replicas: if the web proxy's peer is not trusted, every
  anonymous caller shares one bucket behind it, and the budget is per replica.
- API tokens are hashed and always bounded by their owner's current role. Role edits
  follow a grant ceiling (see [rbac.md](../development/rbac.md)).
- `ORBIT_ALLOW_API_TOKENS=false` disables API tokens entirely.

### 9. Worker impersonation

**Vector:** an attacker uses a stolen worker token to impersonate a worker.

**Mitigations:**

- Worker tokens are scoped to harness kinds and optionally to a worker profile, and can
  be revoked. Tokens issued by the launcher are revoked when the launch stops or fails,
  and a single-execution token is revoked when its execution ends.
- Worker heartbeats are monitored and stale heartbeat rows are pruned.
- Execution-scoped worker endpoints are bound to the token that claimed the execution
  (`executions.worker_token_id`); other tokens get `404`. Worker heartbeat records are
  owned by the first token that reports them and are scoped to the organization (#139).
- Terminal transitions are enforced in SQL from the domain transition table.
- Agent memory is reachable only through a bound execution and uses that execution's
  agent; entries are capped at 64 KiB.

### 10. Cross-team object access (IDOR)

**Vector:** an authenticated member guesses an object ID and reads or changes another
team's agent, connection, provider, worker profile, trigger, tool policy, output, agent
team, execution or approval.

**Mitigations:**

- Every shareable row carries `owner_user_id`, `owner_team_id` and `visibility`
  (`private | team | org`); legacy rows are `org` in the Default team.
- `AccessScope` is loaded from the caller's role and team memberships. List endpoints
  filter rows; get, update, delete and export use `require_view` or `require_manage`,
  returning `404` for invisible rows so existence is not confirmed.
- Executions, events, SSE streams, cancellation and approvals are checked against the
  parent agent's visibility, not only organization membership.
- Worker claims are constrained to the worker profile's owning team.
- Sharing changes require manage access plus the `resource_permissions` licence feature;
  team writes require `teams`. Both are audited.
- Integration tests create two operator users with API tokens and assert list, get and
  delete isolation.

The full matrix is in [idor-matrix.md](idor-matrix.md).

## Residual risks

These are known gaps. They are not mitigated by ORBIT today:

- **No vulnerability scan of container images.** CI audits dependencies and scans for
  secrets, but there is no image scanner (Trivy or similar). Scan the published images in
  your own registry pipeline.
- **No per-agent network egress control.** An agent's `spec.resources.network` setting
  is recorded but not enforced. The chart's NetworkPolicies (`networkPolicy.enabled=true`,
  on in the production example) keep workers away from the database, NATS, the launcher
  and private ranges, but every agent on a worker gets the same public egress. They are
  off in the default values and need a CNI that enforces them.
- **Launched workers have a writable root filesystem.** Chart workloads set
  `readOnlyRootFilesystem: true`, but pods created by the launcher set it to `false`
  (`services/launcher/src/main.rs`). Isolation relies on non-root, dropped capabilities
  and the per-launch pod.
- **Audit log entries for tool calls carry no arguments.** Arguments and results are in
  the execution events, truncated, and subject to the execution's retention.
- **Owners and admins see every resource.** This is intentional (ADR 0011). Team-run
  objects follow their agent team and are not independently shareable (issue #37).
- **Secrets are only as private as the cluster's etcd and the Docker host.** Enable
  encryption at rest for Secrets, and protect access to the Docker socket
  (`docker inspect` shows container environment).
- **The in-memory event bus is not durable.** It is refused in production, where the
  NATS bus is required; PostgreSQL remains the source of truth.

## Recommendations for production

1. Use OIDC authentication, not dev mode, and set `platform.environment=production`.
2. Enable `networkPolicy.enabled` on a CNI that enforces it, so only the API reaches the
   launcher, database and NATS and workers reach only DNS, the API and public addresses;
   keep `launcher.admissionPolicy.enabled` on (Kubernetes 1.30+).
3. Use per-execution isolation (Kubernetes Jobs) for untrusted work.
4. Monitor the audit log, and set `platform.auditRetentionDays` to match your policy.
5. Rotate worker tokens periodically; launcher-issued tokens are already per launch.
6. Use a managed database with encryption at rest, and enable TLS for the ingress and
   for the database connection.
7. Set `ORBIT_MASTER_KEY` to a strong, unique value (`openssl rand -base64 32`).
8. Keep `launcher.authSecrets` narrow and `outbound.allowPrivate=false` unless you run
   private model servers.
9. Scan the container images you deploy.
