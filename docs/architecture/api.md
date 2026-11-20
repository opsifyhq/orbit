# API Design (`/api/v1`)

OpenAPI document: `GET /api/v1/openapi.json`; interactive docs: `GET /docs`.
The TypeScript client in `packages/api-client` is generated from this document.

## Conventions

- JSON only (`application/json`), UTC RFC 3339 timestamps, UUIDv7 identifiers.
- Authentication: `Authorization: Bearer <access_token | api_key>`. Native clients use
  OIDC access tokens; automation uses `orbit_sk_…` API tokens, workers use `orbit_wk_…`
  worker tokens.
- Every response carries `X-Request-Id`; errors include it.
- Idempotent POSTs accept `Idempotency-Key` (executions).
- Keyset pagination: `?limit=25&cursor=<opaque>` → `{ "items": [...], "next_cursor": null }`.
- Filtering: `?status=running&agent_id=...&q=...` where documented.
- Optimistic concurrency: `PATCH` may include `If-Match: "<version>"`.

### Error envelope

```json
{
  "error": {
    "code": "validation_error",
    "message": "agent spec is invalid",
    "details": [{ "path": "spec.resources.timeoutSeconds", "message": "must be 1..=86400" }],
    "request_id": "01J…"
  }
}
```

Codes: `bad_request`, `validation_error`, `unauthorized`, `forbidden`, `not_found`,
`conflict`, `rate_limited`, `unsupported`, `unavailable`, `internal`. Worker launch
endpoints additionally return `launcher_unavailable`, `launcher_error` and
`launch_failed`.

The envelope is enforced centrally: a middleware in `orbit-api` rewrites any 4xx/5xx
response that is not already JSON (malformed path/query parameters, unknown routes,
`method_not_allowed`, `payload_too_large`, `unsupported_media_type`, ...) into the envelope,
keeping headers such as `Allow`. Successful responses, SSE streams, `HEAD` responses and the
Swagger UI (`/docs`) are never rewritten. Settings updates (`PUT /api/v1/settings/*`) are
validated with field-level `details` and recorded in the audit log.

## Resources

| Method & path | Purpose |
|---|---|
| `GET /healthz`, `GET /readyz` | liveness / readiness (database gates; event bus reported as degraded) |
| `GET /metrics` | Prometheus metrics |
| `GET /api/v1/meta` | platform version, auth mode, capabilities |
| `GET /api/v1/docs` | embedded documentation index (categories and pages); authenticated |
| `GET /api/v1/docs/{slug}` | one page: JSON with Markdown, `?format=markdown`, or HTML (`?format=html` / `Accept: text/html`); relative links are rewritten to `/api/v1/docs/...` |
| `GET /api/v1/docs/assets/{path}` | embedded images used by the docs |
| `GET /api/v1/me` | current user, organization, role, permissions |
| `GET/POST /api/v1/agents` | list (filter/search) / create agent + version 1 |
| `GET/PATCH/DELETE /api/v1/agents/{id}` | read / new version or metadata / archive |
| `POST /api/v1/agents/bulk/{archive,delete}` | bulk archive / permanent delete (1–500 deduped ids, `dryRun`, per-id failures) |
| `POST /api/v1/agents/{id}/activate` | draft/paused → active |
| `POST /api/v1/agents/{id}/pause` | active → paused |
| `GET /api/v1/agents/{id}/versions` | immutable version history |
| `GET /api/v1/agents/{id}/export` | YAML export (`orbit.io/v1`) |
| `POST /api/v1/agents/{id}/executions` | manual execution (202) |
| `GET /api/v1/agents/{id}/executions` | executions for agent |
| `GET /api/v1/executions` | all executions (status, agent, cursor) |
| `GET /api/v1/executions/{id}` | execution detail with output/error/usage |
| `GET /api/v1/executions/{id}/events` | stored events (`afterSeq`, `limit`, optional `kind`, `tool`, `q`) |
| `GET /api/v1/executions/{id}/events/stream` | SSE stream (replay + tail) |
| `POST /api/v1/executions/{id}/cancel` | request cancellation |
| `GET /api/v1/harnesses` | harness registry + capabilities + health |
| `GET /api/v1/workers` | live worker heartbeats |
| `GET/POST /api/v1/providers` | list / create model provider |
| `PATCH/DELETE /api/v1/providers/{id}` | update / delete (redacted secrets) |
| `POST /api/v1/providers/{id}/test` | connectivity probe (OpenAI-compatible) |
| `GET /api/v1/connectors` | connector registry/manifests |
| `GET/POST /api/v1/connections` | list / create connection |
| `PATCH/DELETE /api/v1/connections/{id}` | update / delete connection |
| `GET/POST /api/v1/triggers` | list / create trigger |
| `PATCH/DELETE /api/v1/triggers/{id}` | update / delete trigger |
| `GET/POST /api/v1/skills` | list / create skill |
| `GET/PATCH/DELETE /api/v1/skills/{id}` | read / update / delete skill |
| `POST /api/v1/skills/bulk/delete` | bulk permanent delete (1–500 deduped ids, `dryRun`, per-id failures); skills have no archive state (ADR 0013) |
| `GET/POST /api/v1/agent-teams` | list / create Agent Team |
| `GET/PATCH/DELETE /api/v1/agent-teams/{id}` | read / update / delete team |
| `POST /api/v1/agent-teams/{id}/runs` | start a team run |
| `GET/POST /api/v1/agent-teams/{id}/queue` | list / enqueue work items |
| `POST /api/v1/agent-teams/{id}/queue/{item_id}/{run,retry,requeue,cancel}` | queue item actions |
| `GET /api/v1/team-runs` | list team runs (`?archived=`) |
| `GET /api/v1/team-runs/{id}` | team run with steps |
| `GET/POST /api/v1/worker-profiles` | list / create worker profile |
| `GET/PATCH/DELETE /api/v1/worker-profiles/{id}` | read / update / delete profile |
| `POST /api/v1/worker-profiles/{id}/{enable,disable,token,launch}` | profile lifecycle |
| `GET /api/v1/worker-profiles/{id}/bootstrap` | generated Docker/Compose/Kubernetes |
| `GET /api/v1/worker-launches` | launcher-managed instances (`?archived=`) |
| `GET/DELETE /api/v1/worker-launches/{id}` | read / delete launch |
| `POST /api/v1/worker-launches/{id}/{stop,archive}` | stop / archive launch |
| `GET /api/v1/worker-launches/{id}/logs` | launcher logs |
| `GET/POST /api/v1/worker-tokens` | list / issue scoped worker tokens |
| `DELETE /api/v1/worker-tokens/{id}` | revoke worker token |
| `GET/POST /api/v1/api-tokens` | list / create user API tokens |
| `DELETE /api/v1/api-tokens/{id}` | revoke API token |
| `GET /api/v1/agent-instances` | long-running agent instances |
| `GET/PUT /api/v1/settings/oidc` | read / update OIDC configuration (PUT merges; absent fields are kept) |
| `GET /api/v1/organizations/current` | current organization |
| `GET /api/v1/users` | organization members |
| `PATCH /api/v1/users/{id}` | update a member role |
| `GET /api/v1/dashboard/summary` | dashboard counters + recent activity |
| `GET /api/v1/audit-logs` | audit trail |

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

### Not implemented yet

- Inbound webhooks (`POST /api/v1/webhooks/{trigger_id}`) — planned with HMAC verification.
- `GET /api/v1/harnesses/{kind}`.

## SSE stream

```
event: execution.event
id: 42
data: {"seq":42,"kind":"log","level":"info","message":"…","ts":"…"}
```

The endpoint sets `Last-Event-ID` support, sends a `snapshot` event first (execution
status), then replays stored events with `seq > last_event_id` and polls for new rows.
Terminal executions close the stream after a final `done` event.

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
