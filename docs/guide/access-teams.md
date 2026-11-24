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
- Creating, renaming and deleting teams need the `settings:write` permission
  (`POST/PATCH/DELETE /api/v1/teams`). Changing members needs `members:read` plus team lead
  or organization owner/admin (`/teams/{id}/members`). The built-in Admin role has the
  owner/admin bypass; a team lead gets membership management from the team role without
  `settings:write`. See [roles](rbac.md).
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

Open the resource's create or edit form (Studio) and use its **Access control** section:
choose Organization, Team or Private, and the "Owning team" (or call
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

## Team access overview

Open a team from **Teams** (or visit `/teams/{id}`) to see its access overview. The page uses
`GET /api/v1/teams/{team_id}/resources?type=...&cursor=...&limit=...`, one read endpoint that
returns the summary and a page of rows together. Do not assemble this view from the resource
list endpoints: the server applies `AccessScope` to every row.

- **Overview**: member counts by role (lead/member/viewer) and visible resource counts by type.
  A warning counts resources owned by this team but marked `org` visibility — wider than the
  team intended.
- **Resources**: rows grouped by agents, skills, connections, providers, worker profiles,
  Agent Teams, triggers, tool policies and environment mappings. Each row shows the owner user,
  owning team, visibility, whether this team owns it, its last update, and the caller's effective
  `view`/`use`/`manage` level. Leads and org owners/admins can open the same access editor used on
  the resource page from the row's **Access** action.
- **Members**: existing membership management plus an explanation of what each role implies.
  Adding or removing members requires team lead or org owner/admin.
- **How access works**: how `private`/`team`/`org` visibility and team role combine, and how strict
  team isolation narrows organization visibility.

The endpoint returns `404` to non-members and to callers from another organization; team viewers
can read it. It never returns a resource whose owning team is different from the requested team,
and it applies the same visibility predicate as resource lists, so strict team isolation and
invisible resources are respected. Org owners/admins see the same team-scoped set, not a global
matrix.

Outputs are intentionally absent: they are delivery records attached to an agent or Agent Team,
with no `owner_team_id` or `visibility` column, so they are not shareable resources. The UI states
this rather than showing invented ownership or access data.
