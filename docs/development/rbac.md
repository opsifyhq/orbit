# Roles and permissions

ORBIT checks a fixed catalogue of 16 permissions. A role is a set of them. This page is
the reference for the built-in roles, custom roles, API tokens and the rules that stop
people from granting more than they hold. For object-level access (which agent, which
team) see the [access matrix](../security/idor-matrix.md).

## Permission catalogue

The names come from `crates/orbit-security/src/permissions.rs`. A cell marked yes means
the built-in role holds the permission.

| Permission | Viewer | Operator | Admin | Owner |
|---|---|---|---|---|
| `agents:read` | yes | yes | yes | yes |
| `agents:write` | | yes | yes | yes |
| `executions:read` | yes | yes | yes | yes |
| `executions:create` | | yes | yes | yes |
| `executions:cancel` | | yes | yes | yes |
| `approvals:decide` | | | yes | yes |
| `harnesses:read` | yes | yes | yes | yes |
| `providers:read` | yes | yes | yes | yes |
| `providers:write` | | | yes | yes |
| `connections:read` | yes | yes | yes | yes |
| `connections:write` | | yes | yes | yes |
| `triggers:write` | | yes | yes | yes |
| `settings:read` | yes | yes | yes | yes |
| `settings:write` | | | | yes |
| `members:read` | yes | yes | yes | yes |
| `audit:read` | | | yes | yes |

In short: viewers read, operators build and run agents, admins also decide approvals,
manage providers and read the audit log, and only owners hold `settings:write`.

`settings:write` guards organization settings, roles, users, API tokens, worker tokens,
worker profiles and launches, outputs, tool policies, environment mappings, sharing, agent teams
and access teams. Because only owners hold it by default, those actions
are owner-only unless you create a custom role that includes it.

Built-in roles are virtual; the check is `orbit_security::Permission::allows`. Owners and
admins also bypass team isolation, so they see every resource (ADR 0011).

## Development defaults

In `ORBIT_AUTH_MODE=dev` the seeded user is `dev@orbit.local` with the role from
`ORBIT_DEV_USER_ROLE` (default `owner`; `owner`, `admin`, `operator` or `viewer`). Setting
`ORBIT_DEV_USER_ROLE=viewer` is a quick way to see the UI as a read-only user.

## Custom roles

Owners can define organization roles with an explicit permission set. Custom roles need
the `custom_roles` licence feature (see [licensing.md](licensing.md)).

```bash
curl -X POST "$API/api/v1/roles" \
  -H "Authorization: Bearer $ORBIT_TOKEN" \
  -H 'content-type: application/json' \
  -d '{
    "name": "Release manager",
    "permissions": ["agents:read", "executions:read", "executions:create", "approvals:decide"]
  }'
```

- `GET /api/v1/roles`, `POST /api/v1/roles`, `PATCH/DELETE /api/v1/roles/{id}`
- Listing requires `settings:read`; changes require `settings:write` (owner by default).
- Names of built-in roles are reserved.
- Permissions are validated against the catalogue above and every change is audited.
- Assign the role to a user by name: `PATCH /api/v1/users/{id} {"role":"Release manager"}`.
  The Settings page has the same controls under **Members and roles**.

At authentication time ORBIT resolves the stored role name:

- a built-in role gives the role's permission set;
- a custom role gives its stored permission array (carried as the auth scopes);
- a deleted or unknown custom role is rejected with `403` until the user is reassigned.
  Built-in roles are never affected.

Custom roles apply to organization-level actions. For resource scoping, a user with a
custom role is treated like a viewer: they see what their teams and the resource
visibility allow. The effective `permissions` list is returned by `GET /api/v1/me`.

## Grant ceiling

Nobody can give out more than they hold:

- A role can only include permissions its editor holds. An attempt returns `403` and
  names the permissions.
- You cannot edit or delete the role you hold yourself, or a role that grants permissions
  you do not hold.
- You cannot change your own role.
- Only an owner can assign the Owner role or change a user who is an owner.
- Assigning a role (or replacing one) is refused when either role contains permissions
  the caller does not hold.

## API tokens

API tokens (`orbit_sk_…`) are always bounded by their owner's **current** role:

- Effective permissions are the token's scopes intersected with the owner's current
  permissions (the `*` wildcard is honoured on either side). Demoting a user, or editing
  their custom role, takes effect on their existing tokens immediately; no revocation is
  needed.
- A token whose scopes no longer overlap with the owner's role is rejected with `403`.
  An empty stored scope list means "the owner's role permissions". Tokens of disabled
  users are rejected.
- `POST /api/v1/api-tokens` requires `settings:write`. It rejects (`422`, offending
  scopes in `error.details`) any explicit scope the caller does not currently hold; `*`
  is allowed and resolves to the caller's current permissions. Omitting `scopes` defaults
  to the caller's own permissions.
- Owners and admins list every token; other users with `settings:read` list their own.
  Revoking (`DELETE /api/v1/api-tokens/{id}`) requires `settings:write`: an owner can
  revoke any token, anyone else only their own.
- `ORBIT_ALLOW_API_TOKENS=false` makes the API reject API tokens altogether, which leaves
  OIDC (or the dev token) as the only way in. The default is `true`.

Worker tokens (`orbit_wk_…`) are a separate kind: they authenticate workers on the
`/api/v1/worker/*` routes only and are scoped to harness kinds and, optionally, a worker
profile.

## Not implemented yet

- Shared connections with scoped access (the `shared_connections` licence feature is
  defined but not enforced).
- Quotas per organization or user (`quotas`, not enforced).
- A marketplace (`marketplace`, not enforced).
- Group-to-custom-role mapping from OIDC (see [oidc.md](oidc.md)).
