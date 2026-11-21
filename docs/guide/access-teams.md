# Access teams and sharing

Access teams are the multi-team ownership model for resources. They are **not** Agent
Teams (workflows); see [agent-teams.md](agent-teams.md).

## Teams

- **Access teams** in the Admin group of the sidebar lists access teams. Choose **New
  team**, then add members as `lead`, `member` or `viewer`. The creator becomes lead.
- The **Default** team is created with the organization and cannot be deleted. Existing
  resources belong to it.
- Org owners and admins see and manage every team's resources. Team leads manage the
  resources their team owns.
- Creating, renaming and deleting teams and changing their members need the
  `settings:write` permission (`POST/PATCH/DELETE /api/v1/teams`, `/teams/{id}/members`).
  The built-in Admin and Operator roles do not have it, and a team lead does not get it
  from the team role, so in practice owners (or a custom role with `settings:write`)
  manage membership. See [roles](rbac.md).
- Team management is an enterprise feature (`teams`). Community installs keep the single
  Default team and org-wide visibility.

## Visibility

Every shareable resource (agent, skill, connection, provider, worker profile, trigger,
tool policy, Agent Team, environment mapping) has `ownerUserId`, `ownerTeamId` and `visibility`:

| Visibility | Who can see it |
|---|---|
| `private` | owner and org owner/admin |
| `team` | members of the owning team, plus org owner/admin |
| `org` | all organization members |

A team worker only claims executions whose agent is `org`-visible or owned by its team.
Execution context, events, cancellation and approvals use the same rule.

## Sharing

Open the resource's edit form (Studio) and use its **Access control** section: choose
Organization, Team or Private, and the "Owning team" (or call
`GET/PATCH /api/v1/sharing/{resource}/{id}`). The section works only with both the `teams`
and `resource_permissions` license features. Changing visibility requires manage access,
the `settings:write` permission and the `resource_permissions` feature; changes are
audited. Existing data
stays readable if a license expires — only new team/share writes are blocked.

## Strict team isolation

Enabled by default (**Settings → Members and roles → Team isolation → Strict team
isolation**, API `GET/PUT /api/v1/settings/access`; changing it needs `settings:write`).
When enabled, non-admin users only see resources owned by their access teams (or their own
private resources). Organization visibility no longer exposes a team-owned resource to
other teams. Org owners/admins still see everything.

Existing users are migrated into the **Default** team, so legacy Default-owned resources
remain visible while non-default teams stay isolated.

## Team roles

| Role | View team resources | Use/run | Manage |
|---|---|---|---|
| `lead` | yes | yes | yes |
| `member` | yes | yes | no |
| `viewer` | yes | no | no |

Organization owner/admin bypass these checks. Use/run is enforced when starting an
execution with a team-owned agent or connection.
