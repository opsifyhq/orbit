# IDOR / access matrix

Enforced by `crates/orbit-api/src/access.rs` (`AccessScope`). Invisible resources return
`404`; insufficient write permission returns `403`.

| Resource | List filter | Get | Write | Execution/approval |
|---|---|---|---|---|
| Agents | yes | yes | yes | parent visibility |
| Agent versions (`/agents/{id}/versions`) | — | parent visibility (404) | — | — |
| Agent export / agent executions list | — | parent visibility (404) | — | — |
| Agent runtime / activate / pause | — | manage (404 if invisible) | manage | — |
| Skills | yes | yes | yes | — |
| Connections | yes | yes | yes | **use** required on every run path (see preflight) |
| Providers | yes | yes | yes | **use** required when an agent binds it, in both platform- and harness-managed models (incl. team subagents); the worker context re-checks the run principal before returning the credential |
| Worker profiles | yes | yes | yes | claims scoped by owning team |
| Worker launches (`/worker-launches`) | visibility of the launch's worker profile | same (404 when invisible; owner/admin only if the profile was deleted) | launch, stop and archive need manage on the profile | — |
| Agent teams (`/agent-teams`) | yes | view (404) | manage; create, change and delete also need `settings:write` | running needs **use**; the agents, worker profiles and environment mappings a team names must be visible to the caller |
| Team runs (`/team-runs`, `/agent-teams/{id}/runs`) | via agent team | via agent team | — | follow their agent team |
| Environment mappings (`/env-mappings`) | yes | view | manage (`settings:write`) | **use** required to attach one to a worker profile |
| Outputs (`/outputs`) | visibility of the target agent or team | same (404 when the target is invisible) | manage the target agent or team; an output with no target (organization-wide) is owner/admin only | — |
| Triggers | yes | yes | yes | create needs **use** on the target agent and its spec references; fire re-checks the owner |
| Tool policies | yes | yes | yes | — |
| Executions | via agent | via agent | cancel via agent | events/SSE via agent |
| Approvals | via execution → agent | — | decision inherits execution | — |
| Dashboard summary | agent/execution/approval/connection/trigger counts, recent executions and usage filtered in SQL (`ResourceAccess`, joined via agent) | — | — | pending approvals via execution → agent |
| Access teams | member list | member | lead/owner/admin | — |
| Team access overview (`GET /api/v1/teams/{team_id}/resources`) | caller must be a member/viewer of that team or org owner/admin; rows filtered by the same `AccessScope` predicate as resource lists, then restricted to resources owned by that team or with no owning team and `org` visibility | `404` for non-members and other orgs | membership changes remain lead/owner/admin; the overview only reports the caller's effective level | `view`/`use`/`manage` derived from `can_view`/`can_use`/`can_manage` |
| Agent Team queue | team **view** | team **view** | team **use** (enqueue/run/retry/requeue/cancel) | — |
| Sharing | — | view | manage + license | — |
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
