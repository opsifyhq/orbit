# IDOR / access matrix

Enforced by `crates/orbit-api/src/access.rs` (`AccessScope`). Invisible resources return
`404`; insufficient write permission returns `403`.

| Resource | List filter | Get | Write | Execution/approval |
|---|---|---|---|---|
| Agents | yes | yes | yes | parent visibility |
| Agent versions (`/agents/{id}/versions`) | — | parent visibility (404) | — | — |
| Agent export / agent executions list | — | parent visibility (404) | — | — |
| Agent runtime / activate / pause | — | manage (404 if invisible) | manage | — |
| Skills | yes | yes | yes | the run principal needs **use** of every skill the agent or the run names (the worker profile's own skills excepted); a skill that agents or worker profiles reference cannot be deleted |
| Connections | yes | yes | yes | **use** required on every run path (see preflight) |
| Connection test (`POST /api/v1/connections/{connection_id}/test`) | — | — | `connections:write` + `require_use` on the connection (404 if invisible); the stored credential is sent upstream and the result is recorded | — |
| Providers | yes | yes | yes | **use** required when an agent binds it, in both platform- and harness-managed models (incl. team subagents); the worker context re-checks the run principal before returning the credential |
| Provider test (`POST /api/v1/providers/{provider_id}/test`) | — | — | `providers:write` + `require_manage` on the provider; the stored credential is sent upstream and probed models are persisted | — |
| Provider probe (`POST /api/v1/providers/probe`) | — | — | `providers:write`; no object (unsaved config), no credential or models persisted | — |
| Worker profiles | yes | yes | yes | claims scoped by owning team |
| Worker launches (`/worker-launches`) | visibility of the launch's worker profile | same (404 when invisible; owner/admin only if the profile was deleted) | launch, stop and archive need manage on the profile | — |
| Agent teams (`/agent-teams`) | yes | view (404) | manage; create, change and delete also need `settings:write` | running needs **use**; the agents, worker profiles and environment mappings a team names must be visible to the caller; the work-source connection needs **use** at save time, and the dispatcher and outcome writer use it only while the team's owner can |
| Team runs (`/team-runs`, `/agent-teams/{id}/runs`) | via agent team | via agent team | — | follow their agent team |
| Environment mappings (`/env-mappings`) | yes | view | manage (`settings:write`) | **use** required to attach one to a worker profile; mappings a team run names are checked again for the run principal when the worker fetches its context |
| Outputs (`/outputs`) | visibility of the target agent or team | same (404 when the target is invisible) | manage the target agent or team; an output with no target (organization-wide) is owner/admin only | — |
| Triggers | yes | yes | yes | create needs **use** on the target agent and its spec references; fire re-checks the owner |
| Tool policies | yes | yes | yes | — |
| Executions | via agent | via agent | cancel via agent | events/SSE via agent; an idempotent replay only returns an execution of the target agent (409 otherwise), and keys starting with `team:`, `trigger:`, `runtime:` or `rerun:` are reserved |
| Approvals | via execution → agent | — | decision inherits execution | — |
| Dashboard summary | agent/execution/approval/connection/trigger counts, recent executions and usage filtered in SQL (`ResourceAccess`, joined via agent) | — | — | pending approvals via execution → agent |
| Access teams | member list | member | lead/owner/admin | — |
| Team access overview (`GET /api/v1/teams/{team_id}/resources`) | caller must be a member/viewer of that team or org owner/admin; rows filtered by the same `AccessScope` predicate as resource lists, then restricted to resources owned by that team or with no owning team and `org` visibility | `404` for non-members and other orgs | membership changes remain lead/owner/admin; the overview only reports the caller's effective level | `view`/`use`/`manage` derived from `can_view`/`can_use`/`can_manage` |
| Agent Team queue | team **view** | team **view** | team **use** (enqueue/run/retry/requeue/cancel) | — |
| Bulk sharing (`PATCH /api/v1/sharing/{resource}/bulk`) | — | — | `settings:write` + `resource_permissions` licence + `require_manage` per id; the target team must be one the caller belongs to (owner/admin may pick any); 1–500 ids; failures are reported per id | — |
| Sharing | — | view | manage + license | — |
| Portable import (`POST /api/v1/import`) | — | — | write permission per document kind (`agents:write` for agents/skills, `connections:write`, `providers:write`, `triggers:write`, `settings:write` for worker profiles/teams/tool policies); update/upsert needs `require_manage` on the existing row; references resolve only to rows the caller can view | — |
| OpenCode Markdown import (`POST /api/v1/import/opencode`) | — | — | `agents:write`; creates new agents/skills (no target row to check) and requires **use** of the skills, connections and provider named by an imported agent spec | — |
| Bulk agent archive/delete (`POST /api/v1/agents/bulk/archive`, `POST /api/v1/agents/bulk/delete`) | — | — | `agents:write` + `require_manage` per agent; archive transitions and delete FK checks run per id; 1–500 ids, failures reported per id | — |
| Bulk skill delete (`POST /api/v1/skills/bulk/delete`) | — | — | `agents:write` + `require_manage` per skill; delete FK checks run per id; 1–500 ids, failures reported per id | — |
| Export | per-kind read permission + view | — | — | — |

Worker-surface rows (#139): a worker token acts only on executions it claimed. The claim
stores the token id; legacy rows claimed before binding (NULL token, worker id set) stay
reachable by org/profile scope until they finish. Tokens shared by several workers (e.g.
one profile token for a pool, or Docker-driver child containers) are one principal.

Owner/admin bypass is intentional and documented in ADR 0011.

Organization-level objects have no owner or visibility and are checked by permission
only (see [rbac.md](../development/rbac.md)): settings, roles, users, API tokens, the
licence and the audit log. Lists of API tokens show every token to owners and admins and
only the caller's own tokens to everyone else. Granting rules (who may hand out which
role or permission) are in the same page.

The team overview deliberately excludes outputs: output rows have no `owner_team_id` or
`visibility` column and are not in `SHAREABLE_RESOURCES`, so they are delivery records rather
than resources that can be owned by or shared with an access team. The web UI says so instead
of inventing access data. Team membership reads now require the same team visibility check as
the overview; writes require team lead or org owner/admin via `AccessScope`, not just a global
settings permission.

## Approvals and secret references (#144)

- `POST /approvals/{id}/approve|reject` requires `approvals:decide` **and** use access to
  the agent (`AccessScope::require_use`). Owners/admins, the agent owner and team
  leads/members pass; team viewers receive `403`, users who cannot see the agent `404`.
- Four-eyes: `PUT /api/v1/settings/approvals {"approvalsRequireDifferentApprover": true}`
  (default `false`, API only) makes the user in `executions.requested_by` ineligible to
  approve or reject that execution's approvals (`403`).
- An approval past `expires_at` cannot be decided: the call returns `409 approval expired`
  and the row is marked `expired`.
- Worker `GET /worker/approvals/{id}/wait` returns `404` unless the approval belongs to the
  `executionId` in the query. `POST /worker/approvals` clamps `waitSeconds` to 1 s..24 h and
  allows at most 50 pending approvals per execution (`429`).
- Env-mapping secret refs are validated on create/update (`422`, same message for unknown
  and foreign refs, never including values) and resolved with
  `Store::get_secret_value_for_org`. The unscoped `get_secret_value` remains only for
  references read from org-checked rows and the org-less `SecretStore` trait.
