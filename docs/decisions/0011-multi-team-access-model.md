# 0011 — Multi-team access model

Status: **Accepted** and implemented (access teams at `/api/v1/teams`, sharing at
`/api/v1/sharing/*`, ownership and visibility columns, license gating). Epic: #32.

## Decision

ORBIT keeps the organization as the tenant boundary and adds **Access Teams** inside an
organization. Access Teams are separate from **Agent Teams** (workflow/pipeline grouping).
The UI uses **Teams** for access teams and **Workflows** for Agent Teams; APIs remain
`/api/v1/teams` (access) and `/api/v1/agent-teams` (workflows).

## Ownership and visibility

Every shareable resource carries:

- `owner_user_id` (nullable)
- `owner_team_id` (nullable)
- `visibility`: `private | team | org` (default `org` for migration)

Shareable resources: agents, skills, connections, providers, worker profiles, triggers,
tool policies. Executions, team runs, approvals, events and logs inherit visibility from
their agent/team and are visible to the owning team plus org owners/admins.

Rules:

- `private`: owner user, team leads, org owners/admins
- `team`: members of the owning team, plus explicit grants
- `org`: all organization members (current behaviour)
- Org owners/admins may govern all resources but actions remain audited
- Team roles: `lead` (manage team + resources), `member` (use), `viewer` (read)
- Custom roles remain org-level permission sets; resource ownership is an additional check

Explicit grants: a resource can be shared read/use/manage with another team or user.
Default is no grant.

## Enforcement

Enforcement is server-side in repositories and handlers, never UI-only:

- List/get/update/delete/execute/export apply an access predicate
- Execution creation checks access to the agent and every referenced connection
- Worker profiles may only reference visible agents/skills/connections
- Workers claim only executions visible to their profile's team; no cross-team claims
- SSE/logs/approvals use the same visibility
- IDOR tests prove inaccessible IDs return 404/403 (no existence leak)
- Audit every share/unshare and denied access

## Migration

- Create one `Default` access team per organization
- Existing resources: `visibility = org`, `owner_team_id = Default`
- Backfill `owner_user_id` from `created_by` where useful
- No resource disappears after upgrade; no destructive migration
- Append-only migrations

## Licensing

Multi-team and resource visibility are enterprise features (`teams` or
`resource_permissions`). Community installations keep one implicit Default team and
org-wide visibility. Expired licenses do not hide existing data; new team/share writes are
blocked.

## Non-goals

- Cross-organization sharing
- Per-field ACLs
- Replacing Agent Teams / workflows
- Sharing databases or worker credentials across organizations

## Amendment (2026-11-21)
The decision above is what the code does. Resource tables carry `owner_user_id`,
`owner_team_id` and `visibility`, and the access predicate is applied in the store and
handlers. This note only records that the "implementation in progress" status is
obsolete.
