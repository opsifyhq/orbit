# API Design (`/api/v1`)

OpenAPI document: `GET /api/v1/openapi.json`; interactive reference: `GET /docs`.

The document is built from the handler annotations in `crates/orbit-api`. It is also
committed as `packages/api-client/openapi.json`, and the TypeScript client
(`packages/api-client/src/schema.d.ts`) is generated from that file by `pnpm api:generate`,
which needs no running API or database. Three checks keep them in step: a Rust test
compares the router with the document, a second one compares the document with the
committed file, and CI runs `pnpm api:generate --check` for the TypeScript types. The
OpenAPI document is the complete and authoritative list of operations; the table below is
an overview.

## Conventions

- JSON only (`application/json`), camelCase field and query parameter names, UTC RFC 3339
  timestamps, UUIDv7 identifiers.
- Authentication: `Authorization: Bearer <access_token | api_key>`. Native clients use
  OIDC access tokens; automation uses `orbit_sk_…` API tokens, workers use `orbit_wk_…`
  worker tokens.
- Every response carries `X-Request-Id`; errors include it as `requestId`.
- Unauthenticated/public routes (health, metrics, meta, OIDC helpers, OpenAPI/Swagger)
  are limited per source IP to `ORBIT_ANON_RATE_LIMIT_PER_MINUTE` requests per 60-second
  window (default 120). Excess requests return `429 rate_limited` in the standard
  envelope with `Retry-After`. Authenticated API routes and worker-protocol routes are
  not affected; behind an L7 proxy the peer address is the proxy, so enforce client-IP
  limits there as well.
- Idempotent execution creation: send an `Idempotency-Key` header or an `idempotencyKey`
  body field. Keys are scoped to the organization; a repeated key returns the execution
  that was already created.
- Keyset pagination on agents, executions, execution events, agent instances and audit
  logs: `?limit=25&cursor=<opaque>` returns `{ "items": [...], "nextCursor": null }`
  (agents also return `total`). Other lists return a plain array.
- Filtering, where documented: `?status=…&agentId=…&q=…`. Executions accept several
  statuses separated by commas (`status=failed,timed_out`); agents accept
  `status=current` for everything that is not archived.
- Optimistic concurrency: `PATCH /api/v1/agents/{id}` accepts `expectedVersion`, the
  version the edit started from. When the agent has a newer version the request is
  refused with 409 instead of overwriting it.

### Error envelope

```json
{
  "error": {
    "code": "validation_error",
    "message": "agent spec is invalid",
    "details": [{ "path": "spec.resources.timeoutSeconds", "message": "must be 1..=86400" }],
    "requestId": "01J…"
  }
}
```

| Status | Code |
|---|---|
| 400 | `bad_request` |
| 401 | `unauthorized` |
| 402 | `license_required` (the license does not include the feature), `license_limit` (a license limit is reached) |
| 403 | `forbidden` |
| 404 | `not_found` |
| 405, 406, 408, 413, 414, 415 | `method_not_allowed`, `not_acceptable`, `request_timeout`, `payload_too_large`, `uri_too_long`, `unsupported_media_type` |
| 409 | `conflict` |
| 422 | `validation_error`, with one `details` entry per field |
| 429 | `rate_limited` |
| 501 | `unsupported` |
| 502, 503, 504 | `bad_gateway` or `launcher_error`, `unavailable` or `launcher_unavailable`, `gateway_timeout` |
| other 5xx | `internal` |

The envelope is enforced centrally: a middleware in `orbit-api` rewrites any 4xx/5xx
response that is not already JSON (malformed path/query parameters, unknown routes,
`method_not_allowed`, `payload_too_large`, `unsupported_media_type`, ...) into the envelope,
keeping headers such as `Allow`. Successful responses, SSE streams, `HEAD` responses and the
Swagger UI (`/docs`) are never rewritten. Settings updates (`PUT /api/v1/settings/*`) are
validated with field-level `details` and recorded in the audit log.

## Resources

| Method & path | Purpose |
|---|---|
| **System** | |
| `GET /healthz`, `GET /readyz` | liveness / readiness (database gates; event bus reported as degraded) |
| `GET /metrics` | Prometheus metrics |
| `GET /api/v1/meta` | platform version, auth mode, capabilities |
| `GET /api/v1/docs` | embedded documentation index (sections and pages); authenticated |
| `GET /api/v1/docs/{*path}` | one page by slug (for example `architecture/overview`): JSON with Markdown, `?format=markdown`, or HTML (`?format=html` / `Accept: text/html`); a path under `assets/` returns an embedded image |
| `GET /api/v1/auth/oidc/config`, `POST /api/v1/auth/oidc/token` | browser sign-in helpers (unauthenticated) |
| **Account** | |
| `GET /api/v1/me` | current user, organization, role, permissions |
| `GET /api/v1/organizations/current` | current organization |
| `GET /api/v1/users`, `PATCH /api/v1/users/{id}` | organization members / change a member's role |
| `GET/POST /api/v1/api-tokens`, `DELETE /api/v1/api-tokens/{id}` | list / create / revoke user API tokens |
| `GET /api/v1/dashboard/summary` | dashboard counters and recent activity |
| `GET /api/v1/audit-logs` | audit trail |
| **Agents** | |
| `GET/POST /api/v1/agents` | list (filter, search) / create agent and version 1 |
| `GET/PATCH/DELETE /api/v1/agents/{id}` | read / new version or metadata / archive |
| `POST /api/v1/agents/bulk/{archive,delete}` | bulk archive / permanent delete (1–500 deduped ids, `dryRun`, per-id failures) |
| `POST /api/v1/agents/{id}/activate` | draft, paused or archived → active (restores an archived agent) |
| `POST /api/v1/agents/{id}/pause` | active → paused |
| `GET /api/v1/agents/{id}/versions` | immutable version history |
| `GET /api/v1/agents/{id}/export` | YAML export (`orbit.io/v1`) |
| `GET/POST /api/v1/agents/{id}/runtime`, `POST …/runtime/stop` | read / start / stop the agent's continuous runtime |
| `GET /api/v1/agent-instances` | legacy instance list (nothing creates instances; see the domain model) |
| **Executions and approvals** | |
| `POST /api/v1/agents/{id}/executions` | start an execution (202); optional `placement`, `harness`, `idempotencyKey` |
| `GET /api/v1/agents/{id}/executions` | executions of one agent |
| `GET /api/v1/executions` | all executions (`status`, `agentId`, `archived`, cursor) |
| `GET /api/v1/executions/{id}` | execution detail with output, error and usage |
| `GET /api/v1/executions/{id}/events` | stored events (`afterSeq`, `limit`, optional `kind`, `tool`, `q`) |
| `GET /api/v1/executions/{id}/events/stream` | SSE stream (replay, then tail) |
| `POST /api/v1/executions/{id}/cancel` | request cancellation |
| `POST /api/v1/executions/{id}/rerun` | run again with the same input; refused with 409 for a step of a team run |
| `GET /api/v1/approvals` | approval requests (`status`, `executionId`) |
| `POST /api/v1/approvals/{id}/{approve,reject}` | decide an approval |
| **Agent teams** | |
| `GET/POST /api/v1/agent-teams` | list / create an agent team |
| `GET/PATCH/DELETE /api/v1/agent-teams/{id}` | read / update / delete a team |
| `POST /api/v1/agent-teams/{id}/runs` | start a team run |
| `GET/POST /api/v1/agent-teams/{id}/queue` | list / enqueue work items |
| `POST /api/v1/agent-teams/{id}/queue/{item_id}/{run,retry,requeue,cancel}` | queue item actions |
| `GET /api/v1/team-runs` | team runs (`archived`, `teamId`, `limit`) |
| `GET /api/v1/team-runs/{id}` | one team run with its steps |
| **Build** | |
| `GET/POST /api/v1/skills`, `GET/PATCH/DELETE /api/v1/skills/{id}` | skills |
| `POST /api/v1/skills/bulk/delete` | bulk permanent delete (1–500 deduped ids, `dryRun`, per-id failures); skills have no archive state (ADR 0013) |
| `GET/POST /api/v1/tool-policies`, `PATCH/DELETE /api/v1/tool-policies/{id}` | tool policies |
| `GET/POST /api/v1/triggers`, `PATCH/DELETE /api/v1/triggers/{id}` | triggers (only `manual` and `schedule` fire today) |
| `POST /api/v1/import`, `POST /api/v1/import/opencode` | import portable resources / an OpenCode configuration |
| `GET /api/v1/export/{resource}/{id}` | portable export of one resource |
| **Connect** | |
| `GET /api/v1/harnesses` | harness registry, capabilities and health |
| `GET/POST /api/v1/providers`, `PATCH/DELETE /api/v1/providers/{id}` | model providers (secrets are never returned) |
| `POST /api/v1/providers/{id}/test`, `POST /api/v1/providers/probe` | connectivity check of a saved provider / of a configuration before saving |
| `GET /api/v1/connectors` | connector registry and manifests |
| `GET/POST /api/v1/connections`, `PATCH/DELETE /api/v1/connections/{id}` | connections |
| `POST /api/v1/connections/{id}/test` | check a connection; `supported: false` when the connector has no probe |
| `GET/POST /api/v1/outputs`, `PATCH/DELETE /api/v1/outputs/{id}`, `POST …/test`, `GET …/deliveries` | outputs (webhook, Slack, Mattermost, Telegram) |
| `GET/POST /api/v1/env-mappings`, `PATCH/DELETE /api/v1/env-mappings/{id}` | environment mappings |
| **Infrastructure** | |
| `GET /api/v1/workers` | live worker heartbeats |
| `GET/POST /api/v1/worker-profiles`, `GET/PATCH/DELETE /api/v1/worker-profiles/{id}` | worker profiles |
| `POST /api/v1/worker-profiles/{id}/{enable,disable,token,launch}` | profile lifecycle |
| `GET /api/v1/worker-profiles/{id}/bootstrap` | generated Docker, Compose or Kubernetes manifests |
| `GET /api/v1/worker-profiles/{id}/env-mappings` | environment mappings attached to a profile |
| `GET /api/v1/worker-launches`, `GET/DELETE /api/v1/worker-launches/{id}` | launcher-managed workers (`archived`) |
| `POST /api/v1/worker-launches/{id}/{stop,archive}`, `GET …/logs` | stop / archive a launch, read its logs |
| `GET/POST /api/v1/worker-tokens`, `DELETE /api/v1/worker-tokens/{id}` | list / issue / revoke scoped worker tokens |
| **Admin** | |
| `GET/POST /api/v1/roles`, `PATCH/DELETE /api/v1/roles/{id}` | custom roles (licensed) |
| `GET/POST /api/v1/teams`, `PATCH/DELETE /api/v1/teams/{id}` | access teams (licensed) |
| `GET /api/v1/teams/{id}/resources` | team access overview: summary and a page of visible resources (licensed) |
| `GET/POST /api/v1/teams/{id}/members`, `DELETE …/members/{user_id}` | access team members |
| `GET/PATCH /api/v1/sharing/{resource}/{id}`, `PATCH /api/v1/sharing/{resource}/bulk` | owner and visibility of a resource (licensed) |
| `GET/PUT /api/v1/settings/{oidc,harness,approvals,access}` | organization settings (PUT merges; absent fields are kept) |
| `GET /api/v1/license` | installed license, features and limits |

Every execution and team-run list accepts `?archived=true|false|all`; the default view
hides archived terminal records.

Execution events can be narrowed in SQL before `limit` with comma-separated `kind`
(`status`, `log`, `tool_call`, `output`, `error`, `metric`) and literal case-insensitive
substrings `tool` (tool/capability/connector/connection identifiers) and `q` (message or
identifiers). These are read-only presentation filters: they never rewrite events or the
SSE stream. Unknown kinds and terms longer than 200 characters return 400.

### Worker API (token-authenticated, `/api/v1/worker/*`)

Used only by workers and the launcher-managed execution plane:
`session`, `heartbeat`, `claim`, `executions/{id}/{context,running,events,complete,cancel}`,
`approvals` + `approvals/{id}/wait`, `connections/{id}`, `audit`, `env`, and
`memory/{list,store}`.

These routes are a private protocol between the API and its workers. They are left out of
the OpenAPI document on purpose and may change between releases.

### Not implemented yet

- Inbound webhooks (`POST /api/v1/webhooks/{trigger_id}`) — planned with HMAC verification.
- `GET /api/v1/harnesses/{kind}`.

## SSE stream

```
event: execution.event
id: 42
data: {"seq":42,"kind":"log","level":"info","message":"…","ts":"…"}
```

| Event | Meaning |
|---|---|
| `snapshot` | sent first: the execution's current status |
| `execution.event` | one stored event; `id` is its sequence number |
| `done` | the execution reached a terminal state; the stream then closes |
| `error` | the stream could not read the execution; the client should reconnect |
| `shutdown` | the API is stopping; the client should reconnect |

The stream replays stored events with `seq` greater than `Last-Event-ID` (or the
`afterSeq` query parameter), then polls for new rows every 700 ms and sends a keep-alive
comment every 15 seconds.

## Auth model

- Dev mode (`ORBIT_AUTH_MODE=dev`): a seeded developer user/organization. Requests with
  no token are accepted only when `ORBIT_DEV_AUTH_ALLOW_ANONYMOUS=true`; the web
  container injects the dev token server-side. Dev mode is rejected outright when
  `ORBIT_ENV=production`.
- OIDC mode (`ORBIT_AUTH_MODE=oidc`): the web app runs Authorization Code + PKCE against
  the configured provider (`GET /api/v1/auth/oidc/config`, `POST /api/v1/auth/oidc/token`);
  ORBIT validates bearer JWTs against the provider JWKS and maps `(iss, sub)` to a user,
  optionally auto-provisioning.
- API tokens (`orbit_sk_…`): hashed with SHA-256 in `api_tokens`, scoped and revocable.
  Worker tokens (`orbit_wk_…`) are separate: scoped to harnesses/agents and never grant
  control-plane permissions.
- All authorization checks live in `orbit-security` and the handlers, never in prompts.
