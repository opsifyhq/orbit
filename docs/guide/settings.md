# Settings

Settings is where you manage your own account and API access, sign-in, who can do what, and
how this installation behaves. Open **Settings** in the Admin group of the sidebar. The
page has tabs; the selected tab is kept in the address (`#tokens`, `#platform`) so you can
link to it. **Sign out** is at the top right. What you can change depends on your role:
changing anything in Settings needs the `settings:write` permission, which only owners and
custom roles that include it have ([roles](rbac.md)).

| Tab | To see it | To change it |
|---|---|---|
| Account | any signed-in user | not editable |
| API tokens | `settings:read` (you see your own tokens; owners and admins see all) | `settings:write` |
| Single sign-on | `settings:read` | `settings:write` |
| Members and roles | shown only with `settings:write` | `settings:write` |
| Platform | `settings:read` | `settings:write` for the default harness |
| Audit log | `audit:read` | not editable |

## Account

Shows your name, email, role and how you signed in, the organization's name, slug and mode,
and "What <role> can do": the permissions your role holds.

## API tokens

API tokens let CI, scripts and other clients call the ORBIT API. Send one as a bearer token
in the `Authorization` header.

To create a token, choose **New token**, enter a **Name** that says what will use it (for
example `ci-deploy`) and choose **Create token**. The page then shows "Copy your new token
now": the value is shown once and cannot be read again. A token can do whatever your role
can do; its permissions never exceed yours, and demoting you narrows your tokens
immediately ([roles](rbac.md#api-tokens)). To restrict scopes or set an expiry
(`expiresInDays`), create the token through the API.

To remove a token, choose **Revoke** and confirm "Revoke token". Revoked tokens stay in the
list marked "Revoked". Creating and revoking tokens needs `settings:write`.

## Single sign-on

**Single sign-on** configures OpenID Connect so people sign in with your identity
provider (Keycloak, Dex, Okta and others). The form has Issuer URL, Client ID, Redirect
URI, Default role (given on first sign-in), "Create users on first sign-in", and under
"Advanced settings" Scopes, Audience, JWKS URL and Identity claim. Choose **Save sign-in
settings**. The settings are stored by ORBIT, merged over the environment values, and
applied when the API starts, so restart the API after saving. See
[Single sign-on](../development/oidc.md) and
[group to team sync](../development/oidc-team-sync.md). OIDC role mappings accept only the
built-in roles.

## Members and roles

This tab appears only with `settings:write`. It has three sections:

- **Team isolation**: the "Strict team isolation" switch, on by default. When on,
  people who are not admins see only private resources and resources owned by their access
  teams. It applies as soon as you change it ([access teams](access-teams.md)).
- **People**: everyone in the organization, with a role menu per person. A role change
  applies as soon as you pick it. You cannot change your own role, only an owner can assign
  or change the owner role, and you cannot hand out a role with permissions you do not hold.
- **Custom roles**: add roles on top of owner, admin, operator and viewer by choosing a
  name and permissions. This needs the `custom_roles` license feature; without it the page
  says custom roles are a licensed feature.

## Platform

- **This installation**: version, API version, environment, sign-in mode, event transport
  and which features are enabled.
- **Default harness**: the harness used for runs that do not name one, such as scheduled
  runs, until a harness can be stored on a trigger. The menu offers "No default (require an
  explicit choice)" and the available harnesses, and saves as soon as you choose. It is
  shown only with `settings:write`. The setting is `GET/PUT /api/v1/settings/harness`.
- **License**: tier, customer, expiry and grace period, licensed features, usage against
  limits and warnings. Without a valid license the tab shows "Community edition". See
  [Licensing](licensing.md).

The "require a different approver" approvals policy is not in the web app yet; set it with
`PUT /api/v1/settings/approvals` ([Approvals](approvals.md#require-a-different-approver)).

## Audit log

**Recent activity** lists who changed what in this organization, newest first: changes to
agents, connections, roles, tokens and more. It needs `audit:read`, which Owner and Admin
have. Each row names the changed item (the name recorded with the change, or the start of
its id); **Details** shows the action, the full target id, the request id and the recorded
metadata. **Show** limits the list to one kind of activity, and **Load older activity**
pages back 25 entries at a time. Entries are kept indefinitely unless `ORBIT_AUDIT_RETENTION_DAYS` is set to a
number of days (Helm `platform.auditRetentionDays`); then older entries are deleted.

## API

- `GET /api/v1/me`, `GET /api/v1/organizations/current`, `GET /api/v1/users`
- `GET/POST /api/v1/api-tokens`, `DELETE /api/v1/api-tokens/{id}`
- `GET/PUT /api/v1/settings/oidc`, `/settings/access`, `/settings/harness`,
  `/settings/approvals`
- `PATCH /api/v1/users/{id}`, `GET/POST /api/v1/roles`, `PATCH/DELETE /api/v1/roles/{id}`
- `GET /api/v1/license`, `GET /api/v1/meta`
- `GET /api/v1/audit-logs`: query `limit` (25 by default, at most 100), `before_id` (the
  previous page's `nextCursor`), `action` (exact, such as `agent.deleted`) and `area` (the
  action family before the first dot, such as `agent` or `team_run`)
