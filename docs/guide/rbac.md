# RBAC

## Built-in roles

| Role | Permissions |
|---|---|
| `owner` | all permissions |
| `admin` | all except `settings:write` |
| `operator` | `agents:read/write`, `executions:read/create/cancel`, `harnesses:read`, `providers:read`, `connections:read/write`, `triggers:write`, `settings:read`, `members:read` |
| `viewer` | `agents:read`, `executions:read`, `harnesses:read`, `providers:read`, `connections:read`, `settings:read`, `members:read` |

Operators and viewers cannot decide approvals (`approvals:decide`), read the audit log
(`audit:read`) or manage providers (`providers:write`); admins can. Skills are managed
with `agents:write`.

`settings:write` is not limited to "platform settings". It also gates worker profiles,
launches and worker tokens, agent team create/update/delete, outputs, tool policies,
environment mappings, access teams and their members, changing a resource's visibility
(`/sharing`), API tokens (create and revoke), user role changes, custom roles, and the
settings under **Settings** (access policy, SSO, default harness, approvals policy). The
built-in Admin and Operator roles cannot do these things; owners can, and so can a custom
role that includes `settings:write`. Per-resource manage checks (owner, team lead,
owner/admin) apply on top.

Permissions are checked in handlers with `auth.require(Permission::…)`. The platform
evaluates them from the stored role; custom roles carry an explicit permission array.

## Custom roles (enterprise)

`GET/POST /api/v1/roles`, `PATCH/DELETE /api/v1/roles/{id}`; mutations require
`settings:write` and the `custom_roles` license feature. Built-in role names are reserved;
permissions are validated against the catalog and audited. Assign with
`PATCH /api/v1/users/{id} {"role":"Release manager"}`.

The role name is stored on the user; at authentication time ORBIT resolves built-in
permissions or the custom permission set. Deleting a role locks assigned users out until
they are reassigned (403), rather than silently downgrading them.

## API tokens

Create and revoke tokens under **Settings → API tokens** ([settings](settings.md)).

API tokens never exceed their owner's current role: effective permissions are the token's
scopes intersected with the owner's present permissions, so demoting a user immediately
narrows their existing tokens. Minting a token with a scope you do not hold is rejected
(422); `*` means "whatever my role grants". See [docs/development/rbac.md](../development/rbac.md).

## OIDC group mapping

`roleClaim` + `groupRoles` map IdP groups to built-in roles; `syncRoles` re-evaluates on
login. See [docs/development/oidc.md](../development/oidc.md).

## Access teams and resource sharing (enterprise)

Access teams (`/api/v1/teams`, sidebar **Access teams**) are available with the `teams` license
feature. Members have `lead`, `member` or `viewer` roles; the creator becomes lead. The
**Default** team cannot be deleted. These are distinct from **Agent Teams** (workflows).

Shareable resources (agents, skills, connections, providers, worker profiles, triggers,
tool policies, agent teams and environment mappings) carry `ownerUserId`, `ownerTeamId`
and `visibility`:

| Visibility | Visible to |
|---|---|
| `private` | owner and org owner/admin |
| `team` | members of the owning access team, plus owner/admin |
| `org` | every member of the organization |

`GET/PATCH /api/v1/sharing/{resource}/{id}` reads and changes sharing; mutations require
the `resource_permissions` feature and manage access, and are audited. Worker profiles
with an owning team only claim executions whose agent is `org`-visible or owned by that
team.

## Not implemented yet

Per-resource grants to individual users, shared connections with scoped access, and
marketplace entitlements. See #15.
