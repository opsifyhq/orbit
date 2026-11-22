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

**Vector:** a user with write access to providers, outputs or work sources points ORBIT
at an internal address (cloud metadata, loopback, a management interface).

**Mitigations:**

- Outbound URLs for providers, outputs and work sources are checked after DNS
  resolution (`crates/orbit-security/src/http.rs`). Loopback, link-local, cloud-metadata,
  unspecified, multicast and reserved addresses are blocked; metadata addresses can never
  be allowlisted.
- Private ranges (RFC 1918, CGNAT, IPv6 ULA) are allowed by default so self-hosted model
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
  `/docs` and the OpenAPI document. Worker routes use worker tokens. Keep `/metrics` and
  `/api/v1/meta` (which reports the licence tier and customer) off untrusted networks if
  that matters to you.
- Dev auth mode is refused when `ORBIT_ENV=production`, and the Helm chart refuses it
  when `platform.environment=production`.
- OIDC mode validates JWTs against the provider's JWKS. Discovery and JWKS fetches are
  cached and refreshed at a bounded rate, so unauthenticated requests cannot be turned
  into a stream of calls to the identity provider.
- Unauthenticated routes are rate limited per client. Behind the web proxy and the
  ingress every request has the same TCP peer, so the limit keys on `X-Forwarded-For`
  only for peers in `ORBIT_TRUSTED_PROXIES` (the chart trusts the private ranges by
  default). A client inside a trusted range can choose its own key; narrow the setting
  to the pod CIDR where clients share those ranges.
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
- **No network egress control.** An agent's `spec.resources.network` setting is
  recorded but not enforced, and the Helm chart ships no egress NetworkPolicy. The only
  policy it can create (`networkPolicy.enabled=true`) limits ingress to the launcher.
  Restrict worker egress with your own NetworkPolicy.
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
2. Apply your own NetworkPolicies to restrict worker egress, and enable
   `networkPolicy.enabled` so only the API reaches the launcher.
3. Use per-execution isolation (Kubernetes Jobs) for untrusted work.
4. Monitor the audit log, and set `platform.auditRetentionDays` to match your policy.
5. Rotate worker tokens periodically; launcher-issued tokens are already per launch.
6. Use a managed database with encryption at rest, and enable TLS for the ingress and
   for the database connection.
7. Set `ORBIT_MASTER_KEY` to a strong, unique value (`openssl rand -base64 32`).
8. Keep `launcher.authSecrets` narrow and `outbound.allowPrivate=false` unless you run
   private model servers.
9. Scan the container images you deploy.
