# Provider & Connector Contracts

## Part A — Model providers

Source: `crates/orbit-contracts/src/provider.rs`. Providers are stored separately from
harnesses because a harness may manage its own models, or consume an ORBIT-configured
endpoint.

### Kinds

| Kind | Use |
|---|---|
| `openai_compatible` | vLLM, LiteLLM, APISIX/AI gateway, OpenRouter-style, any `/v1` endpoint |
| `anthropic` | Anthropic's native Messages API (`ModelProtocol::AnthropicMessages`) |
| `harness_managed` | harness owns auth/inventory (OpenCode Go, Claude Code subscription, Pi) |

The database check `providers_kind_check` must list every kind; migration 0054 added
`anthropic`, and an API test saves one provider of each kind.

OpenCode Go is **not** a harness and not a generic provider: it is represented as a
`harness_managed` provider whose credential is stored in `secret_ref` and handed to the
OpenCode adapter. Provider names and model inventories are never hardcoded in the UI or
domain; both are discovered from the provider registry.

### `openai_compatible` configuration

Config documents use camelCase keys and reject unknown fields (`deny_unknown_fields`).

```jsonc
{
  "baseUrl": "https://gateway.example.com/v1",   // http(s), no trailing slash
  "model": "qwen3-coder",
  "secretRef": "provider/9f0c…",                 // never returned by the API
  "headers": { "X-Tenant": "orbit" },            // optional, non-secret custom headers
  "requestParams": { "temperature": 0.2 },       // optional, adapter passthrough
  "contextWindow": 131072,
  "maxOutputTokens": 16384,
  "compat": {
    "chatCompletions": true,
    "responsesApi": false,
    "tools": true,
    "streaming": true,
    "jsonMode": false,
    "vision": false,
    "embeddings": false,
    "modelsEndpoint": true
  },
  "settings": { "models": ["qwen3-coder"] }       // optional, ORBIT metadata (see below)
}
```

`Authorization` and `X-Api-Key` headers are rejected in `headers`; configure them as a
credential instead.

### `anthropic` configuration

```jsonc
{
  "baseUrl": "https://api.anthropic.com/v1",   // optional, this is the default
  "model": "claude-…",
  "secretRef": "provider/9f0c…",
  "contextWindow": 200000,
  "maxOutputTokens": 8192,
  "settings": { "models": ["claude-…"] }   // optional, ORBIT metadata (see below)
}
```

### `harness_managed` configuration

```jsonc
{
  "harness": "opencode",         // required: the harness that owns the provider
  "provider": "opencode-go",     // provider name as the harness knows it, not validated
  "model": "…",
  "secretRef": "provider/9f0c…",
  "settings": {}
}
```

`settings` is ORBIT metadata that is never sent to the endpoint or the harness. Every
kind accepts it; `settings.models` is the model catalog that agent and worker model pickers
offer. **Test** on a saved provider writes the discovered models there, and the provider
form edits it.

Capabilities are **explicit**: adapters must not assume every OpenAI-compatible endpoint
supports tools, streaming, JSON mode or `/models`. Unsupported features degrade
predictably and are visible in the UI.

### Resolution

The worker resolves a provider into `ResolvedModel` for the adapter, which includes the
endpoint, model id, custom headers, params and a short-lived `SecretLease`. Credentials
are decrypted only in memory and injected into harness processes through env vars or
tmpfs files; they are never written to logs, execution events or the frontend.

## Part B — Connectors

Source: `crates/orbit-contracts/src/connector.rs` (trait + DTOs), manifests in
`crates/orbit-api/src/connectors.rs`, execution-plane clients in
`services/worker/src/{github,gitlab,x}.rs`.

### Concepts

- **Connector**: versioned integration manifest with auth methods and capability catalog.
- **Connection**: authorized account in an organization (`credential_ref`, scopes,
  status, expiry).
- **Capability**: `resource.action` with an effect:
  - `read` — safe to call at autonomy level 1+ (Observe)
  - `write` — mutating, requires level 3 (prepare/approval) or 4 with allowlist
  - `external_side_effect` — sends messages, creates MRs, spends money; always
    auditable and approval-gated below level 4

### Manifest

```jsonc
{
  "slug": "gitlab",
  "name": "GitLab",
  "version": "0.1.0",
  "auth": ["oauth2", "api_token"],
  "webhooks": true,
  "capabilities": [
    { "id": "issues.list", "effect": "read" },
    { "id": "issues.create", "effect": "write" },
    { "id": "pipelines.status", "effect": "read" }
  ]
}
```

### Invocation contract

```rust
#[async_trait]
pub trait Connector: Send + Sync {
    fn manifest(&self) -> &ConnectorManifest;
    async fn validate_connection(&self, context: &ConnectorContext) -> Result<(), ConnectorError>;
    async fn invoke(&self, context: ConnectorContext, call: CapabilityCall)
        -> Result<CapabilityResult, ConnectorError>;
    async fn verify_webhook(&self, headers: &http::HeaderMap, body: &[u8],
        secret: Option<&SecretLease>) -> Result<Option<ConnectorEvent>, ConnectorError>;
}
```

**Current implementation note:** the built-in GitHub, GitLab and X connectors are invoked
by the worker's policy-enforcing tool broker (`services/worker/src/broker.rs`) through the
`github.rs` / `gitlab.rs` / `x.rs` clients rather than as `Connector` trait objects. The
trait remains the target contract for third-party connectors. X `posts.search` read
hydration is a separate pre-execution step; X `posts.create` is a broker tool.

`ConnectorContext` contains only platform-approved grants; the connector SDK enforces
that `call.capability` is in the grant list before executing. MCP servers are one
transport option (`mcp` capability provider), not the only connector mechanism.

### Implemented scope

- **Manifests** seeded for `mock`, `telegram`, `github`, `gitlab`, `kubernetes`, `x`,
  `webhook` (see the table below).
- **Connections** stored with encrypted credentials; create / list / update / delete.
  The API never returns the credential (`hasCredential` only).
- **GitHub** through the tool broker (reads implemented; writes are approval-gated and
  still need live end-to-end verification).
- **GitLab** through the tool broker.
- **Kubernetes** through the tool broker: read-only inspection using a ServiceAccount
  token or a static kubeconfig (client in `crates/orbit-kubernetes`; see
  [the Kubernetes connector guide](../guide/kubernetes-connector.md)).
- **X read hydration**: before an execution, the worker resolves `x` bindings with the
  `posts.search` capability, loads the connection's bearer token, calls
  `GET /2/tweets/search/recent`, and injects up to 10 posts into the harness input as
  `externalContext` with `provenance.trusted=false`. The query comes from
  `payload.query`, then `connection.config.query`, then the task text. App-only bearer
  tokens are sufficient for this read.
- **X posting**: `posts.create` is the tool `x_posts_create`, served only for `x` bindings.
  It calls `POST /2/tweets` with the connection credential as a user-context bearer token,
  accepts `text` plus `replyTo` / `reply.in_reply_to_tweet_id`, and has the
  `external_side_effect` effect so autonomy and approvals apply. 401/403 returns an
  actionable `user-context`/`tweet.write` error. See [the X connector guide](../guide/x-connector.md).
- **Repository tools** (`repo.checkout`, `repo.commit`, `repo.push`) run in the worker
  over a per-execution workspace; git credentials are injected through `GIT_ASKPASS` in
  the child process only, and only for `repo.checkout` and `repo.push` (a commit is
  local and gets no token). The workspace's `.git/config` is agent-writable, so
  `repo.push` never uses its `remote.*` settings: it pushes to the URL built from the
  connection (`cloneBaseUrl`) and the checked-out repository, re-checked against
  `allowedRepositories` / `allowedProjects`. Git refuses to run when the repository config
  sets `url.*` (rewrites) or `http.*` (proxy, TLS, extra headers) keys.
  the child process only.
- **Several connectors**: every supported binding of the agent (and of its team)
  contributes its tools, one binding per connector type; a later binding of the same type
  is ignored with a warning event. A binding without agent tools (X `posts.search`, which
  hydrates the prompt instead) does not count.
- **Capability enforcement**: the broker evaluates `evaluate_capability` (autonomy ×
  effect × grant) before every call; writes below L4 create approvals.

### Connector status and capabilities

Capability lists are taken from `builtin_manifests()` in
`crates/orbit-api/src/connectors.rs`. The tool broker serves `github`, `gitlab`,
`kubernetes` and `x` bindings (`services/worker/src/broker.rs`); X only exposes
`posts.create` as a tool because `posts.search` stays pre-execution hydration. A
manifest-only connector can be selected and given a connection, but no code invokes its
capabilities.

| Connector | Status | Auth | Capabilities (effect) |
|---|---|---|---|
| `github` | invokable through the broker | `api_token` | read: `issues.list`, `issues.get`, `pulls.list`, `pulls.get`, `checks.list`, `repo.checkout`; write: `issues.comment`, `issues.create`, `issues.update`, `pulls.create`, `repo.commit`; external side effect: `repo.push` |
| `gitlab` | invokable through the broker | `oauth2`, `api_token` (API token implemented) | read: `projects.list`, `group_issues.list`, `boards.list`, `issues.list`, `issues.get`, `merge_requests.list`, `merge_requests.get`, `pipelines.status`, `repo.checkout`; write: `issues.create`, `issues.update`, `issues.comment`, `merge_requests.comment`, `repo.commit`; external side effect: `merge_requests.create`, `merge_requests.approve`, `merge_requests.merge`, `repo.push` |
| `kubernetes` | invokable through the broker | `api_token` | read: `namespaces.list`, `pods.list`, `pods.get`, `pods.logs`, `deployments.list`, `deployments.get`, `events.list` |
| `x` | read hydration + broker posting | `oauth2`, `api_token` (user-context token required for writes) | read: `posts.search`; external side effect: `posts.create` |
| `mock` | manifest only | none | read: `notes.list`; write: `notes.create` |
| `telegram` | manifest only | `api_token` | read: `messages.list`; external side effect: `messages.send` |
| `webhook` | manifest only | `hmac_webhook`, `none` | external side effect: `request.send` |

Connection allowlists are enforced by the worker clients before any request:

| Connector | Allowlist | Empty list |
|---|---|---|
| `github` | `allowedRepositories` (owner/repo); `checks.list` `ref` must be a valid git ref (no `..`, `?`, `#`, `%`, leading or trailing `/`), and list `state` must be `open`, `closed` or `all` | any repository the token can reach |
| `gitlab` | `allowedProjects` for project tools and `projects.list` results; group tools accept only `groupId` and `allowedGroups` | `allowedProjects`: any project; groups: refused unless `groupId` is set |
| `kubernetes` | `namespaces`, `allowedResources` (`*` for all) | `namespaces`: only `defaultNamespace`; `allowedResources`: every tool refused |

The Kubernetes client replaces container `env[].value` and the
`kubectl.kubernetes.io/last-applied-configuration` annotation with `[redacted]` in pod and
deployment results; `valueFrom` references are kept.

`repo.checkout` is performed by the worker before the harness starts and is not exposed
as a model tool. Outbound result delivery uses Outputs (`/api/v1/outputs`), not the
`webhook` connector.

Not yet implemented: Telegram `messages.send` invocation, OAuth flows, webhook
verification, and a third-party connector SDK. X posting is implemented and covered by
mock-server tests; the live X check is opt-in because it publishes a real post.
