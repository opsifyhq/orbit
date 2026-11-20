# Changelog

All notable changes to ORBIT are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the
project adheres to [Semantic Versioning](https://semver.org/).

Release process: [docs/development/releasing.md](docs/development/releasing.md).

## [0.25.0] - 2026-11-20

### Added

- Web edit forms preserve fields they do not render, so saving teams, providers,
  connections, skills and worker profiles no longer drops hidden configuration (#205).
- Web test suite via `node --test` for HTTP classification, proxy path validation and
  login redirects.
- Bulk lifecycle actions for agents and skills: agents can be archived or permanently
  deleted, skills can be deleted (archive is out of scope — see ADR 0013). A dry-run
  preview explains mixed selections before confirmation, partial failures keep their ids
  selected, and one audit event records each bulk operation (#249).

### Changed

- API proxy validates and re-encodes path segments, adds a nonce CSP plus frame/referrer/
  no-sniff headers, preserves `next` query strings and serves docs assets/SSE same-origin
  (#219).
- 403 is treated as no-access instead of session expiry, `/no-access` is reachable, failed
  worker starts surface an error instead of a false success, and `/meta` failures show the
  retry state (#206).
- Harness I/O is bounded: oversized OpenCode/Pi lines are skipped, prompts over 100 KiB are
  rejected, tool-event data is truncated, Pi deltas are batched and CPU usage is unset when
  not measured (#118).
- Image builds verify downloaded Docker/kubectl/gh/glab/OpenCode binaries with checksums,
  keep secrets out of `deploy.sh` output, and limit the web build context (#220).
- Helm quick-start docs acknowledge the dev-auth ingress guard where required (#214).
- Schedules no longer need cron: one schedule picker in Start work and Agent Studio offers
  minutes, hourly, daily, weekly and monthly patterns, a time zone (yours by default, daylight
  saving included), a plain-English summary and the next three runs; Custom keeps raw cron with
  clear errors. Schedule summaries now name their time zone.
- Start work uses the Studio layout of Agent and Team Studio: a section rail that doubles as the
  summary, one section at a time (Who, What, When, Where, Then) and the action always in view.

## [0.24.0] - 2026-11-20

### Added

- Provider quick-start presets for OpenRouter, DeepSeek, Google Gemini and Azure OpenAI,
  plus native Anthropic provider support (Messages protocol for OpenCode/Pi and model
  discovery) (#232).
- Bulk **Access control** for agents and skills: multi-select, filtered Select all, one
  dialog for visibility/owning team, and a server-side bulk sharing endpoint with
  per-resource failures (#238).
- Kubernetes connector Test connection, shared client and client-certificate kubeconfig
  auth, with live kind tests for namespaces, pods, events and logs (#45).
- Embedded docs now follow the app navigation and render tables correctly (#234).

### Changed

- Agent Team screens follow the rest of the app: list, team page, Team Studio, run page,
  queue and run dialog (#241).
- Skill picker has Select all / Select none; resource forms flush access edits before the
  resource save, and Sharing is labelled Access control (RBAC) (#236, #237).
- Worker profiles show team-scoped claims and explain that profile visibility is not claim
  scope (#245).

### Fixed

- OIDC logins can no longer bind an email to a user already linked to another identity
  (#192), and saving sign-in settings preserves mappings and surfaces failures (#197,
  #230).
- Team-run autoApprove is capped at the agent's own autonomy, so Observe/Recommend agents
  cannot be raised to level 4 (#194, #227).
- Harness-managed models now require use of the provider, closing a cross-team credential
  path (#193, #226).
- Approval waits are counted in Job/container deadlines, so runs waiting for a human are
  not killed (#196, #229).
- Native OpenCode `task`/`subagent` dispatch is an internal action; subagents start at
  lower autonomy while their own tool calls remain individually policy-checked (#246).
- Stale worker heartbeat rows are pruned after 24 hours, and production/example values no
  longer pin stale image tags (#198, #239).
- Environment mappings now have ownership/visibility, filtered lists and manage/use checks
  (#240).
- Restored 41 regression tests lost while integrating the earlier PR backlog (#191, #224).

### Security

- Worker-side git ignores repository hooks, filters, textconv and global/system config and
  runs with a cleared allowlisted environment (#195, #228).

## [0.23.0] - 2026-11-20

### Changed

- Execution detail pages can approve or reject a pending tool call inline, with the exact
  action details, instead of navigating to Approvals first.
- Approvals are one click: the card's Approve/Reject buttons decide immediately and show a
  per-button loading state. The redundant confirmation dialog was removed.

### Fixed

- Native harness tools (bash/edit/write/read) now follow the agent's autonomy and approval
  mode when no tool-policy rule matches. L4/Auto no longer asks for every call; L3/Auto
  still requires approval and L1/L2 still deny writes. Explicit rules can only tighten
  this decision.

### Security

- Creating a trigger now requires **use** on the target agent and use of every connection
  and provider its current spec references, closing an IDOR that let an Operator attach a
  trigger to a private agent (#135).
- Agent Team queue endpoints now enforce team visibility: listing needs view and
  enqueue/run/retry/requeue/cancel need use, so non-members get `404` and team viewers
  cannot start the team's agents (#132).
- Production refuses to start with either published development master key; the Helm
  chart rejects the dev key when `platform.environment=production` (#107).

### Added

- Kubernetes connector: the connection Test button now parses ServiceAccount/kubeconfig
  credentials, reports visible namespaces and RBAC errors, and kubeconfig
  client-certificate auth is supported. The client moved to `orbit-kubernetes` so the API
  and worker share the same parser and HTTP client (#45).

## [0.22.1] - 2026-11-20

### Fixed

- OIDC/SSO redirect loop: the app shell no longer queries `/me` on public pages
  (login/callback), which cached a 401 and cleared the fresh token after sign-in.
- Team-output delivery enqueue no longer fails with "no unique or exclusion constraint
  matching the ON CONFLICT specification": the conflict target now names the partial
  unique index predicate.
- The signed-in navigation no longer flashes before OIDC redirects to the IdP; protected
  pages wait for auth metadata and `/me` instead of rendering the shell first.

## [0.22.0] - 2026-11-20

### Added

- Launcher workload names and worker ids are unique per launch (#106).
- API token scopes are bounded by the owner's current role, including custom-role changes
  and demotions (#136).
- OpenCode enforces autonomy for native tools (#110).
- Agent Studio preserves `harnessConfig` when saving (#109).
- Dashboard summary only counts agents the caller can see (#133).
- Agent version history requires agent visibility (#134).
- Schedule triggers claim atomically, support timezones, reconcile managed triggers and
  repair stuck `next_run_at` values (#111).
- CI runs `cargo audit` directly so the result fails only on real advisories, not on a
  check-run permissions error (#127).
- Embedded in-app documentation with working links (#44).
- Portable YAML/Markdown export and import for agents, skills, triggers, connections,
  providers, worker profiles, agent teams and tool policies (#80).
- Execution preflight shared by every run path: agent use, referenced connections and
  providers, harness availability and hourly quota (#140).
- Opt-in audit retention and output delivery claims (#130, #112).
- Approvals require use access, optional four-eyes, expiry handling and org-scoped secret
  references (#144).
- Worker-profile component split and calmer editor (#79).
- Worker launches: exact live lookup, no phantom launches and guarded updates (#121).

### Changed

- Worker tokens are bound to their claimed executions; memory, launches and harness paths
  are scoped to that claim (#139).
- Tool policy fails closed and derives native tool effects server-side (#141).
- Execution and approval lists apply visibility in SQL (#122).
- Deployment defaults bind infrastructure ports to loopback; Helm gains production
  guardrails (#128).
- Harness status, paths, descriptors and image URLs are documented accurately (#129).
- Continuous runtimes continue after a failed execution and Stop cannot be overwritten by
  an in-flight tick (#150, #115).
- Team runs and queue transitions are atomic and retryable (#113).
- API lifecycle uses DB-only readiness, confirmed publishes and prompt shutdown (#125).
- Worker always finishes claimed executions and reaps timed-out runs (#116).
- Launcher reports accurate launch status and workload lifecycle (#120).
- Web keeps sessions on transient errors and stops stream retry loops (#126).
- OIDC only syncs roles when a group mapping matches (#124).
- Rejections use the error envelope; settings are validated and audited (#123).
- Auth rejects wrong dev tokens and unsafe login redirects (#143).
- Credentials stay out of edited endpoints, argv and plain Kubernetes env (#142).
- Agent commits cannot expose runtime credentials; harness subprocess environments are
  isolated (#138, #137).
- Observe/Recommend deny writes; egress is labelled as not enforced (#131).
- Removed dead store code and unused dependencies (#130).

### Fixed

- Execution event sequence allocation is serialized (#114).
- Output deliveries are claimed, have no enqueue window and stop on permanent failures
  (#112).
- CI now runs real security and deploy gates and fixes the root build (#127).

### Security

- Cleared the RustSec advisories that the `cargo audit` gate (#127) reports:
  `utoipa` 5 → 6 and `utoipa-swagger-ui` 9 → 10 (drops `zip` 3.0.0, six advisories, a
  build-time dependency), and `sqlx` 0.8 → 0.9 (drops `rsa`, RUSTSEC-2023-0071, and moves
  the lockfile-only `libsqlite3-sys` to 0.40.0, RUSTSEC-2026-0459; neither was compiled).
  Generated OpenAPI is unchanged apart from ordering.

### Changed

- Minimum supported Rust version is now 1.94 (required by `sqlx` 0.9).
- Dynamic SQL is wrapped in `sqlx::AssertSqlSafe` with the reason it is safe, as `sqlx`
  0.9 requires.

## [0.21.0] - 2026-11-19

### Added

- Native tool policy hooks for Pi and OpenCode: native tools call the broker `/policy`
  before execution; deny blocks and approvals wait.
- Import OpenCode agent `.md` and `SKILL.md` definitions with frontmatter parsing,
  warnings for unsupported fields, and import buttons on Agents/Skills.
- Native Agent Team subagents can use their own agent model; OpenCode provider configs and
  subagent files are generated per model.
- Continuous monitor runtimes: start/get/stop, atomic scheduling ticks, interval,
  heartbeat, session continuity and agent-detail status. Persistent harness processes and
  the standalone code runtime remain open in #88.

### Changed

- **Agent Studio** shows one section at a time instead of one long page. A section nav
  shows each section's current value (it replaces the separate summary), marks sections
  with errors and keeps Save in view; each section ends with Back / Next, phones get a row
  of step pills, and the URL hash opens a section directly (`#model`). A failed save opens
  the first section with a problem.
- Connectors in Agent Studio are compact rows; allowed capabilities are grouped by what
  they can do (read only, change data, act outside ORBIT) instead of a warning badge on
  each.
- Skills are chosen with a searchable, fixed-height picker (Agent Studio and Agent teams)
  with an All / Selected filter. Agent team cards show skills as tags with "+N more"
  instead of a comma-joined list.

### Fixed

- Agent messages in the execution timeline render as Markdown (tables, lists, code)
  and fold long messages behind "Show all", instead of raw text cut off at 600 characters.
- Skill descriptions imported from SKILL.md no longer show YAML formatting (`>-`, `|`) or
  wrapping quotes, and are clamped to two lines in lists.

## [0.20.0] - 2026-11-19

### Fixed

- Agent YAML download uses the same-origin API proxy instead of opening/fetching the
  internal `NEXT_PUBLIC_ORBIT_API_URL` host, which browsers cannot reach.

### Added

- OIDC `requireMappedGroup` / `ORBIT_OIDC_REQUIRE_MAPPED_GROUP`: deny users whose IdP
  groups match no role or access-team mapping and show a dedicated no-access page.

### Changed

- Web UI redesigned on a documented design system (`docs/development/design-system.md`):
  theme-aware sidebar with regrouped, foldable navigation, bundled Archivo and JetBrains Mono fonts, flat
  bordered surfaces, an orbit status glyph whose shape encodes state, and shared
  primitives (`Section`, `Field`, `Callout`, `Segmented`, `ChoiceCard`, `DescriptionList`,
  `DropdownMenu`, `useConfirm`).
- Access controls (0.19.0 license gating) use the redesigned access field: without a license
  that includes `teams` and `resource_permissions`, edit forms show a one-line note instead of
  the controls and make no sharing requests. Agent access now sits at the end of the agent form,
  and agent teams gained access settings.
- The access field always shows the owning team (optional unless visibility is Team) and,
  when strict team isolation is on, explains that only that team's members can see the
  resource and how to share it with everyone.
- Navigation labels: Dashboard is now **Overview**, Work is the standing **Start work**
  action, Providers is **Model providers**, Teams is **Access teams**. Routes are unchanged.
- Overview, Agents, Executions, execution detail, agent detail, Approvals, Start work,
  Agent Studio, Agent teams, Workers, Settings (now tabbed) and the catalogue pages were
  rebuilt on those primitives. Execution filters live in the URL.
- **Environments**: mappings can now be edited and duplicated (the API already allowed it).
  Each mapping is a card showing where every variable comes from and which worker profiles
  use it; the editor picks one source per variable, suggests a name from the connection and
  flags invalid or duplicate names.

### Added

- Command palette (`⌘K` / `Ctrl+K`) for pages, agents and common actions.
- A schedule trigger can carry its own task (`spec.triggers[].task`), used as the task of
  every run it fires. Editable in Agent Studio; **Start work → On a schedule** creates one.
- Execution timeline reads as a story: what the agent said, the tools it used, which
  connector actions were allowed, blocked or sent for approval, and what a person decided.
  Agent answers render as Markdown. A run that ends without a result shows the agent's
  last message. Verified against real OpenCode runs.
- A run paused for approval shows "Needs approval" instead of "Running"; approvals show
  the repository and how long until they expire.
- Phone navigation reaches every page through a **More** sheet.
- Light / Dark / System theme choice in the account menu.
- **Activate** from the draft notice on an agent.
- Settings shows the organization default harness, team isolation, and members and roles
  to users with `settings:write`.

- The 0.18.0 additions carried onto the new design: **Export as JSON** sits in each
  row's overflow menu, access (visibility and owning team) is an "Access" block in edit
  forms, and **Run again** uses the rerun endpoint. The separate grid/list toggle for
  agents is gone: the agent list is always the compact, aligned list.

### Fixed

- Access: with strict team isolation on (the default), lists of agents, skills, worker
  profiles, connections, providers, triggers, tool policies and agent teams showed non-admins
  other teams' organization-visible resources, which then returned 404 when opened. List
  queries now apply the same rule as the detail check.
- Licenses: the `executions_per_day` limit, spelled as documented in ADR 0010, was ignored
  because the parser only read `executionsPerDay`. Both spellings are now accepted.
- **Workers → Edit** crashed on a fresh install: the seeded platform profile stored its
  inline environment mappings as `{}` instead of a list. The seed now writes `[]` and the
  editor treats anything but a list as empty.
- Workers older than the schedule `task` field could not claim runs of an agent with a
  scheduled task (their `TriggerSpec` refuses unknown fields), leaving the run stuck in
  `dispatched`. The worker context now leaves that field out; workers never read triggers.
- A run a live worker took but never started stayed `dispatched` forever, blocked the
  agent's concurrency, and could not be cancelled. The API now ends it after 10 minutes
  without activity (`worker_did_not_start`) or 60 seconds after an unanswered cancel, and
  refuses a late `running` report for an ended run with `409` so the harness never starts.
- Execution detail kept showing the status from when the page opened: status changes
  that come without an event (picked up, cancel requested) now arrive by polling while a
  run is active. A run a worker took but never started is flagged after two minutes.
- Saving an agent in Agent Studio silently removed its harness, and the placement and
  execution-mode settings were discarded. Studio now has an optional **Harness** choice:
  "Choose per run" (each run picks one, or the organization default) or a specific harness
  with its placement and execution mode, and saving keeps it. New agents start on "Choose
  per run" instead of the mock harness.
- Saving an agent whose spec did not change said "New version created"; it now says
  "Agent saved", or names the version that was created.
- Environments: "Attach to a worker" opens that worker profile with the mapping already
  ticked; "Used by" links open the profile.
- The agent **Executions** tab listed every agent's executions: the web client sent the
  filter as `agent_id` while the API expects `agentId`.
- Agent YAML export pointed at `http://localhost:8080` when no public API URL was set; it
  now goes through the same origin with the session token.
- Newly created draft agents were hidden by the agent list's default "Active" filter.
- Execution detail rendered the approval and error notices twice.
- Running an agent with no harness and no organization default failed with a raw API
  error; the run dialog now asks for the harness up front.
- OpenAI-compatible model providers could not be created: the web client sent a
  `settings` field the API rejects for that kind.
- An agent's default task was dropped on save because the shared schema did not know
  `runDefaults`.
- "Restore" on an archived agent always failed (the API has no such transition); the
  button is gone and the archive dialog now says archiving cannot be undone.
- Start work no longer offers options it does not act on (event and connector starts,
  auto-approve, keep running); they are disabled and say so.
- New agent teams no longer save a hardcoded repository and description nobody typed.
- Start work: choosing a harness hid every agent without a harness of its own, and draft
  agents could not be chosen. It also tried to launch a worker when one was already online.
- Start work now warns when a repository is entered for an agent that cannot check it out.
- Time spent waiting for an approval no longer counts toward the execution timeout
  (`PauseClock` in the harness contract); an approval request now stays open for 15 minutes
  regardless of the run's remaining time.
- An action a person rejected is refused if the agent asks for it again in the same run,
  instead of queueing a second identical approval.
- OpenCode results contain the answer written after the last tool call, not every remark
  the agent made along the way.
- The "dispatched" event records the agent's provider and model, shown in execution details.

## [0.19.1] - 2026-11-19

### Fixed

- Pi platform providers use a temporary `models.json` with the configured base URL and
  `PI_OFFLINE=1`; airgapped workers no longer contact `api.openai.com`.
- Added a unit test for the generated Pi platform model configuration.

## [0.19.0] - 2026-11-18

### Changed

- Access is one consistent, license-gated Card in edit forms for agents, skills,
  connections, providers, worker profiles and Agent Teams.
- Access shows disabled visibility controls for unsaved resources.
- Frontend polish for worker profiles, Agent Teams and agent lists.

## [0.18.0] - 2026-11-18

### Added

- Rerun terminal executions from execution detail (`POST /api/v1/executions/{id}/rerun`).
- Secret-free resource export (`GET /api/v1/export/{resource}/{id}`) with recursive
  redaction and Export actions.
- Worker profiles with an empty image tag follow the platform image tag and auto-relaunch
  after an upgrade (token revoke, drain, replace, audit).
- Settings now renders Runtime, Access policy and Users & roles cards.

### Changed

- Resource visibility/team access moved from Share into Edit forms.
- Worker-profile and Agent Team editors grouped into sections with sticky footers.
- Agent lists have a compact list/grid view; worker/team secondary actions moved to More
  menus.
- Regenerated the OpenAPI TypeScript client.

### Fixed

- Description-only agent edits no longer fail as duplicate versions.
- Planned/scaffolded harnesses are hidden from run dialogs and rejected by the API.
- Pi auth works for launcher-created profile workers (writable copy of the Secret).
- Stale SSO tokens are cleared; login shows SSO again and invalid sessions redirect.
- Invalid/expired tokens are handled centrally on 401.

## [0.17.0] - 2026-11-18

### Breaking

- Agents no longer store `spec.harness`. Harness is selected per execution/Agent Team
  step/trigger, with an organization default fallback. Legacy versions remain readable.
- Harness-specific options move to `harnessConfig.<kind>`; run dialogs expose the harness
  override.

### Fixed

- Pi worker auth Secret is copied from a read-only seed volume into a writable
  `/home/orbit/.pi/agent` by an init container. Pi no longer fails with
  "Credential store read failed ... read-only file system".

### Added

- Run dialogs for agents and Agent Teams expose an optional harness override, so one
  agent can run on OpenCode or Pi without changing its definition.

### Documentation

- ADR 0012: agents become harness-agnostic; harness is selected per execution/team step.

### Added

- Worker-profile `nativeToolPermissions` JSON map (OpenCode `permission`), e.g. `bash:
  deny`, `shell: allow`, `edit: ask`. Editable in the worker profile UI.

### Changed

- Team roles: `lead`/`member` can use team resources, `viewer` is read-only. Execution
  creation enforces use access for the agent and referenced connections.

### Added

- Strict team isolation (default on): non-admins only see resources owned by their teams,
  regardless of org visibility. Migration adds existing users to the Default team.
- Settings → Access policy toggle and `GET/PUT /api/v1/settings/access`.

### Added

- Settings → Users & roles UI: organization role assignment and custom-role CRUD with
  permission checkboxes (owner/admin only).

### Added

- OIDC `emailClaim` / `ORBIT_OIDC_EMAIL_CLAIM` selects the unique identity/email claim
  (default `email`); use `name` when multiple IdP accounts share an email.

### Added

- OIDC `autoCreateTeams` / `ORBIT_OIDC_AUTO_CREATE_TEAMS`: create missing access teams
  from `teamGroups` mappings (Helm: `auth.oidc.autoCreateTeams`).

## [0.16.3] - 2026-11-18

### Fixed

- The OpenCode MCP config writer no longer overwrites `opencode.json`; it merges, so
  `websearch`, `tools` and `permission` settings survive.

## [0.16.2] - 2026-11-18

### Fixed

- OpenCode Web Search is disabled through the root `websearch: false` setting, which
  removes the provider prompt/tool. `tools`/`permission` alone still allowed the model to
  trigger the interactive provider request.

## [0.16.1] - 2026-11-18

### Fixed

- OpenCode Web Search/WebFetch are now hidden from the model via `tools.* = false`, not
  only permission-denied. A denied tool was still advertised and caused repeated failing
  tool calls.

## [0.16.0] - 2026-11-18

### Added

- OIDC `teamGroups` mapping (env `ORBIT_OIDC_TEAM_GROUPS`) adds users to access teams by
  IdP group, additively, with `lead`/`member`/`viewer` roles.
- Helm: `auth.oidc.teamGroups` is exported as `ORBIT_OIDC_TEAM_GROUPS`.

## [0.15.0] - 2026-11-18

### Added

- Helm: `auth.oidc.teamGroups` is exported as `ORBIT_OIDC_TEAM_GROUPS`.
- OIDC `teamGroups` mapping (env `ORBIT_OIDC_TEAM_GROUPS`) adds users to access teams by
  IdP group, additively, with `lead`/`member`/`viewer` roles.

- `glab` and `gh` are bundled in the OpenCode/Pi worker images (pinned versions).
- Worker-profile `allowGlab` / `allowGh` (default false). When enabled and a team GitLab/
  GitHub connection is attached, the execution environment receives `GITLAB_TOKEN` /
  `GH_TOKEN` (plus host) for CLI tools. Tokens are exposed to shell tools in that execution.
- Worker-profile OpenCode toggles `allowWebSearch` / `allowWebFetch` (default false).
  ORBIT writes OpenCode `permission.websearch` / `permission.webfetch`, preventing
  headless runs from aborting on an interactive Web Search request.

## [0.14.0] - 2026-11-17

### Added

- Agent Teams can attach connections and environment mappings directly
  (`connectionIds`, `envMappingIds`). Team connections are merged into step grants with
  visibility validation; team env mappings are resolved into the execution environment.
  Both are editable in the Agent Team UI.

### Fixed

- Native Agent Team subagents no longer write a per-agent model into OpenCode agent
  files. They inherit the primary agent's resolved provider/model, avoiding missing
  provider API keys in subagents; pipeline teams keep per-step models.

### Added

- Agent Team steps default to a 1-hour timeout (configurable per team via
  `timeoutSeconds`) instead of inheriting the 5-minute agent default.
- Agent Team skill picker has Select all / Clear controls.

### Added

- Read-only Kubernetes connector: ServiceAccount-token or strict static kubeconfig auth,
  namespace/resource allowlists, namespaces/pods/deployments/events list/get and bounded pod
  logs. Example reader RBAC and guide included. Cluster E2E and test-connection remain.

### Multi-team access model

Enterprise (`teams` + `resource_permissions`): access teams, private/team/org visibility,
sharing API/UI, team-scoped worker claims, execution/team-run/approval isolation. Community
installations keep the Default team and org-wide visibility; existing data stays readable
after license expiry, only new team/share writes return `402 license_required`.

### Added

- Access-team owner guide (`docs/guide/access-teams.md`) and a community-license test
  proving team/sharing writes return `402 license_required`.

### Added

- Connections UI renders connector `configSchema` fields (string/number/boolean) for
  connectors other than GitLab and webhook, while keeping advanced JSON editing.

### Security

- Worker claims are now restricted to the authenticated worker's organization. Previously
  the claim query could pick a queued execution from another org.

### Documentation

- Documented Agent Team ownership/visibility and worker-profile team claim/context rules.

### Security

- Approval decisions now require visibility of the underlying execution's agent. Added
  cross-team tests for execution get/events/SSE/cancel and approval lists.

### Added

- Agent Teams now carry owner/visibility and are filtered like other shareable resources.
  Team-run lists join the owning Agent Team; run detail and start require team visibility;
  create/update/delete require manage access.

### Security

- Worker execution endpoints (context, running, events, completion, cancel) now re-check
  that the execution's agent is org-visible or owned by the worker profile's team, and
  enforce the profile's selected agent scope. Knowing an execution ID is not enough.

### Added

- Worker profile create/update rejects referenced agents and skills the caller cannot see,
  preventing a team profile from being configured against another team's resources.

### Changed

- List visibility is now enforced by repository SQL predicates for agents, skills,
  providers, connections, worker profiles, triggers and tool policies. Handlers no longer
  post-filter, and agent totals reflect the visible set.

### Fixed

- Newly created skills, providers, triggers, tool policies and worker profiles now record
  the creating user as owner, so private/team sharing does not lock the creator out.
- Added a second IDOR integration test covering private skills.

### Added

- Connection test endpoint and UI for GitLab/GitHub (`/user` with the stored token) and
  webhooks (reachability probe). Results update connection status/last-checked.

### Added

- Access teams (`teams`) and resource sharing (`resource_permissions`) are license-gated
  premium features. Documented the enterprise access model, the IDOR matrix and the
  cross-team threat.

### Added

- Execution visibility follows the agent's access scope for list/get/events/SSE/cancel and
  approvals. Team workers claim only org-visible or owning-team agents.

### Added

- Worker claims are scoped by the worker profile's owning team: a team worker only claims
  queued executions whose agent is org-visible or owned by that team. Profile-less worker
  tokens keep the existing org-wide behavior.

### Added

- Generic resource sharing: `GET/PATCH /api/v1/sharing/{resource}/{id}` with
  private/team/org visibility, ownership validation and audit. Share dialog on the agent
  detail page (same API works for skills, connections, providers, workers, triggers and
  tool policies).

### Added

- Access team management: `/api/v1/teams` CRUD, members/roles (lead/member/viewer), and a
  Library → Teams UI. Default team protected; team creation makes the creator lead.

### Added

- Access scope for multi-team visibility: agents, skills, connections, providers, worker
  profiles, triggers and tool policies enforce `private | team | org` visibility on
  list/get/update/delete/export; agent execution requires access to the agent and its
  referenced connections. Org owners/admins govern; team leads manage their team resources.

### Added

- Multi-team schema foundation (ADR 0011): `access_teams`, `team_members`, and
  `owner_user_id`/`owner_team_id`/`visibility` on agents, skills, connections, providers,
  worker profiles, triggers and tool policies. Existing rows are backfilled to a Default
  team with `org` visibility; new organizations get a Default team. Behaviour remains
  org-wide until enforcement lands.

### Added

- Connection dialog exposes a first-class **GitLab URL** (deriving `/api/v4` and clone
  URL) and **Webhook URL** instead of requiring raw JSON; advanced JSON remains for the
  other connector fields.

## [0.13.0] — 2026-11-16

### Added

- Dedicated **Library → Environments** page for reusable environment mappings, with
  row-by-row creation (`ENV_VAR` + connection/secret), deletion and a direct link from the
  worker profile editor. Settings no longer owns worker configuration.
- Provider model discovery is clearer: **Fetch models** in the provider dialog, **Test &
  discover** on saved providers, catalog count on OpenAI-compatible cards, and a hint that
  the default model becomes a dropdown after discovery.
- Self-hosted GitLab connection templates (`baseUrl`, `cloneBaseUrl`) and explicit
  `GIT_SSL_CAINFO` for workers and launcher-created pods, so git clone/push trusts a
  private CA as well as API calls.

## [0.12.1] — 2026-11-16

### Added

- Sign-out control in the sidebar user chip, mobile header and Settings. Clears the
  browser token and returns to `/login`; API tokens are unaffected.

## [0.12.0] — 2026-11-16

### Added

- **Agent default task** (`spec.runDefaults.task`): pre-fills manual runs and is used when a
  run omits the task. System instructions remain separate and are never copied into it.
- **Work → Then** shows the outputs configured for the selected agent or team, with a link
  to manage them.
- **Team-scoped outputs**: an output can target an Agent Team and fires once per terminal
  team run. `agentId` and `teamId` are mutually exclusive.
- Worker profiles pinned to an older image tag are flagged in Workers, with a one-click
  **Use platform tag** and a Relaunch action for active profiles.

### Fixed

- Worker launches default to active instances only; stopped/failed launches are behind
  **Show history** and have a Relaunch button.
- Launcher RBAC splits `pods/log` into its own `get`-only rule and drops unused `patch` on
  Deployments/Jobs, satisfying RBAC escalation scanners.
- Output webhook labels are explicit per kind (Slack/Mattermost incoming webhook vs generic
  webhook).

## [0.11.0] — 2026-11-16

### Added

- Reusable environment mappings (`/api/v1/env-mappings`) attachable to multiple worker
  profiles; Settings library + worker multi-select; inline mappings override attached ones.
- Worker profile changes that need a restart mark the launch `relaunchRequired`. Saving
  with a live worker asks **Save only / Save & relaunch** and warns that running agents may
  be interrupted.
- Outputs API (`/api/v1/outputs`) with Slack, Mattermost and generic webhook delivery,
  durable outbox retries and a test endpoint. Outputs page with create/edit/test/delete;
  SSRF/HMAC hardening remains in #28.

### Fixed

- Launcher-created workers now receive the chart's custom CA Secret and
  `SSL_CERT_FILE`/`REQUESTS_CA_BUNDLE`/`NODE_EXTRA_CA_CERTS`. This fixes
  `UNABLE_TO_GET_ISSUER_CERT_LOCALLY` when an OpenCode worker calls a provider signed by a
  private CA (#40).

- Execution Output card is width-constrained; long output wraps/scrolls instead of widening
  the page (#39).
- Launcher ServiceAccount can create/read/delete Deployments and Jobs on clusters that
  enforce RBAC (namespaced least-privilege Role).

### Changed

- Agent Studio and worker profiles use provider model catalogs: the model is a select when
  the provider exposes `settings.models`, with "Provider default"/custom fallback. Worker
  profiles now list ORBIT-managed and harness-managed providers.
- Added the ORBIT wordmark/icon/social preview, README header and matching web favicon.

## [0.10.0] — 2026-11-16

### Added

- **Offline enterprise licensing**: Ed25519-signed JWS license, compiled-in public keys,
  expiry + 30-day grace, feature flags and limits. No installation ID, activation call or
  network access; `ORBIT_LICENSE` / `ORBIT_LICENSE_FILE` (default
  `/etc/orbit-license.jwt`) with 60s hot reload.
- **License enforcement**: `custom_roles` feature gate plus seats, agents, enabled worker
  profiles and executions/24h limits (80% warnings, hard cap at 100%). Status, usage and
  warnings in `GET /api/v1/license`, `/api/v1/meta` and **Settings → License**.
- **Vendor signing tool**: `services/license-cli` (`keygen`, `sign`, `public-key`), offline
  only. Vendor public key `orbit-2026-01` is embedded; the private key lives outside the
  repository.
- **Documentation**: task-oriented `docs/guide` (agents, teams, worker profiles, skills,
  RBAC, licensing, tool policies, providers), owner and vendor licensing guides, ADR 0010
  and a trademark/branding policy.
- Helm `license.existingSecret` / `license.key` / `license.value`.

### Fixed

- Release builds ignore `ORBIT_LICENSE_PUBLIC_KEYS`; only compiled public keys are trusted,
  closing the self-signing bypass. Debug builds retain the dev key for local testing.
- Helm upgrades from pre-0.9 values with `--reuse-values`: the new `license` block is
  optional and PostgreSQL/NATS security values fall back to safe defaults (no nil and no
  accidental root execution).
- Licensing CLI added to the Docker dependency-cache layers.

## [0.9.1] — 2026-11-16

### Changed

- Long-lived worker launches now create a **Deployment** instead of a bare Pod, so
  Kubernetes recreates workers after node drains, evictions and maintenance. Single
  execution launches stay Jobs. The worker id remains stable across restarts.
- Launcher status/stop/logs are workload-aware (Deployment, Job and legacy Pods) and logs
  are read by the `orbit.launch-id` label.
- The shell shows the signed-in user (avatar, display name, role) next to the theme
  toggle on desktop and mobile, linking to Settings (#23).

### Fixed

- Worker OOM kills are detected for Kubernetes and Docker (`OOMKilled`/exit 137) and
  surfaced as a failed launch with the memory limit and a remediation hint. Running
  executions on that worker are failed with a `worker_oom` event instead of hanging until
  timeout (#24).

## [0.9.0] — 2026-11-16

### Added

- Worker profiles have an explicit agent scope: **All agents (dynamic)** includes agents
  created later; **Selected agents** keeps a fixed list. Migration `0022` backfills
  existing profiles. The worker session/claim no longer relies on empty `agentIds` to mean
  "all" implicitly.
- Worker profiles have typed CPU/memory requests and limits plus a workspace size
  (`resources.requests`, `resources.limits`, `resources.workspace`). Values are validated
  (quantities, request ≤ limit) and defaulted when omitted.
- The launcher applies those resources to Kubernetes Pods/Jobs and Docker containers
  (`--cpus`/`--memory`); profile bootstrap output includes them.
- Worker profiles default to the platform image tag (`ORBIT_IMAGE_TAG`, set by the chart to
  the release `appVersion`) instead of `latest` when no tag is provided.
- Provider models: `POST /api/v1/providers/probe` probes an unsaved config; a successful
  probe on a saved provider persists discovered models to `settings.models`. The provider
  dialog can test the connection before saving, fills the default model and catalog, and
  OpenAI-compatible providers now support a model catalog.

### Changed

- Agent Studio no longer exposes unused `memoryMb` / `cpuMillis` as worker sizing; the
  card is labelled **Execution limits** (timeout, concurrency, max output tokens).
  Worker sizing belongs to the worker profile.

### Added

- **OIDC group → role mapping** (issue #18): `roleClaim`, `groupRoles` and `syncRoles`
  via env, Helm values and Settings. The most privileged mapped role wins; a token
  without the claim never downgrades an existing user.
- **Custom roles** (issue #15 foundation): org-scoped named permission sets with
  `/api/v1/roles` CRUD, user assignment by role name, and permission resolution at
  authentication time. Built-in roles remain virtual. Resource-level permissions,
  shared connections, quotas and license gating are still open.

- **Tool policy layer** (issue #19): org and per-agent rules with tool/capability/effect/
  connection/path/command matchers, deterministic `allow | require_approval | deny`
  evaluation, and argument-level restrictions that cannot weaken autonomy or grants.
- Effective rules are delivered in the worker execution context; the worker broker
  enforces them for connector tools and exposes `POST /policy` for native harness hooks
  (Claude Code `PreToolUse`, OpenCode hooks). Native tools default to `require_approval`.
- `GET/POST /api/v1/tool-policies` and `PATCH/DELETE /api/v1/tool-policies/{id}` plus
  migration `0019_tool_policies.sql`, audit events and approval reason/impact fields.

## [0.8.4] — 2026-11-16

### Fixed

- OIDC browser discovery and authorization-code token exchange now use the shared
  custom-CA HTTP client. They previously built plain `reqwest` clients, so a private CA
  (for example Dex behind an internal CA) worked for `curl` and JWKS validation but the
  login flow failed TLS.

## [0.8.3] — 2026-11-16

### Fixed

- Rust HTTP clients now load **every** certificate in the configured CA bundle
  (`SSL_CERT_FILE` / `REQUESTS_CA_BUNDLE` / `NODE_EXTRA_CA_CERTS`). `reqwest` with
  `rustls-tls` ignores the system store and previously only the first PEM block was
  added, so an OIDC provider signed by a later certificate in the bundle failed TLS
  while `curl` worked. Mixed bundles fall back to per-block parsing and failed reads are
  logged.
- The web container now exports `NODE_EXTRA_CA_CERTS` (and `SSL_CERT_FILE`) when
  `caCertificates` is enabled, so Node-side OIDC discovery/token exchange also trusts
  the custom CA.

## [0.8.2] — 2026-11-16

### Fixed

- Bundled NATS runs as the unprivileged `nats` user (1000) instead of root, so the pod
  is admitted under the restricted Pod Security profile.

## [0.8.1] — 2026-11-16

### Changed

- Every chart workload is now consistent with the Kubernetes **restricted** Pod
  Security profile: `runAsNonRoot`, `allowPrivilegeEscalation: false`,
  `capabilities.drop: [ALL]`, `seccompProfile: RuntimeDefault`.
- The launcher container gets a security context, a writable `/tmp`, `HOME=/tmp` for
  kubectl's cache and the chart's `imagePullSecrets`.
- The bundled PostgreSQL no longer runs as root or adds `CHOWN`/`SETUID`/… capabilities;
  it runs as the upstream `postgres` user (999) with `fsGroup: 999`. Use a managed
  database in production; its data directory must be writable by that user/group.
- NATS and the web container state `runAsNonRoot` explicitly.

### Fixed

- The launcher now emits restricted-PSS-compatible Pods and Jobs for platform workers
  (`runAsNonRoot`, RuntimeDefault seccomp, no privilege escalation, all capabilities
  dropped). OpenCode/Pi get writable emptyDir mounts for their home data.
- `externalDatabase` key names fall back to `host`/`port`/`dbname`/`user`/`password`
  when `--reuse-values` comes from an older chart.

## [0.8.0] — 2026-11-16

### Added

- `externalDatabase.existingSecret` accepts an operator-managed PostgreSQL Secret (for
  example a CloudNativePG application Secret). With `urlKey` empty the API composes the
  URL from `host`, `port`, `dbname`, `user` and `password` (`ORBIT_DATABASE_*`), with
  percent-encoding and configurable `sslMode`. Set `urlKey: uri` to use a pre-composed
  URL instead. The chart omits `database-url` from its own Secret in that case.
- `CHANGELOG`/chart version aligned so a release tag publishes every image the default
  chart needs (launcher and harness workers included).

### Changed

- **Workers are launched from profiles by default.** The chart enables `launcher`
  (`launcher.enabled=true`) and no longer requires a worker token to install; the API
  mints a scoped token per launch and revokes it on stop.
- Fixed worker Deployments are opt-in (`worker.enabled=false` by default). Enabling one
  still requires `worker.token`.
- `launcher.token` is generated by the chart and reused from the release Secret when
  empty. Set it explicitly for GitOps or when `secrets.create=false`.
- On first start the API seeds one launchable mock profile (`Platform worker (mock)`)
  when a launcher is configured (`platform.seedWorkerProfile`, `ORBIT_SEED_WORKER_PROFILE`).
  Generated profile images follow `ORBIT_IMAGE_PREFIX` / `ORBIT_IMAGE_TAG`, which the
  chart sets to its registry and release tag.
- Worker, OpenCode/Pi worker, launcher and API pod templates carry a `checksum/config`
  annotation so token/secret changes roll the pods.
- The launcher inherits the first `imagePullSecrets` entry for Pods it creates when
  `launcher.imagePullSecret` is empty.

### Fixed

- `scripts/deploy.sh` no longer claims to issue tokens; it enables the fixed worker only
  when a token exists and leaves the launcher token to the chart.
- Chart docs no longer tell operators to pass `worker.token` for a default install.

## [0.7.0] — 2026-11-13

The release that turns ORBIT into a usable self-hosted platform: guided work creation,
automatic issue dispatch, multi-repository teams, memory, and a hardened release
pipeline.

### Added

**Work creation**
- New **Work** wizard (`/work`) that walks through *When → Where → Who → What → Then →
  Summary* and reads the choices back before creating anything.
- Optional work name, shown as the title of the execution instead of the agent name.
- Harness selection is honoured: choosing a harness filters the agents to that harness
  and is passed to the execution as a harness override.
- When no worker profile exists for the chosen harness, one is created, enabled and
  launched automatically.

**Agent Teams**
- Durable work queue with deduplication per `(teamId, externalKey)`.
- Background **dispatcher** in the API: per-team Postgres advisory lock, GitHub/GitLab
  issue listing, label-based claiming, max items per poll, `autoRun`, and last
  poll/error tracking.
- **Close the loop**: on success the issue gets a comment with PR/MR links and the done
  label; failures apply the failed label or release the claim for a retry.
- Retry / requeue / cancel actions for queue items (API + UI) and an attempts policy.
- **Multi-repository workspace**: `repositories[]` with `connectionId`, `path`,
  `primary` and `branch`, checked out sequentially with path-traversal validation and a
  manifest in the prompt. Git tools accept a `repository` argument.
- **Model selection per agent**: a team-level model override plus a per-step override;
  each agent otherwise uses its own model.
- Work source configuration (provider, connection, repository/project/group, labels,
  max items/attempts, poll interval, auto-run).

**Harnesses and providers**
- Platform-managed OpenAI-compatible providers now work with **OpenCode** (written into
  `opencode.json` with base URL, headers and a leased credential) and **Pi**
  (`OPENAI_API_KEY` / `OPENAI_BASE_URL` plus `--provider openai`).
- Harness override per execution (agent teams and the Work wizard).
- OpenCode bumped to **v2.3.0**, Pi to **v1.5.2**.
- Dedicated **`orbit-worker-pi`** image and Helm `piWorker` deployment.

**Skills and memory**
- Skill scripts are materialized executable (`chmod +x`) and validated by extension
  (`.sh`, `.bash`, `.py`, `.js`, `.ts`, `.rb`, `.pl`, `.go`) with per-file (100 KB) and
  total (1 MB) limits.
- Skills page supports uploading files and editing the bundled-file map.
- **Agent memory**: short-term and summary memory stored per agent, injected into the
  execution payload, and pruned hourly.

**Workers and execution plane**
- **Environment variable mappings** on worker profiles: map ORBIT connections/secrets to
  environment variables, resolved by the API and injected at worker start.
- Select-all / clear for profile agents and skills; image tags default to `latest`.
- Kubernetes **Job-per-execution**: single-execution launches run as a Job with
  `restartPolicy: Never`, `backoffLimit: 0` and a 300s TTL; long-lived profiles stay Pods.
- `orbit-worker` is a single binary; the MCP stdio bridge is the `mcp-broker`
  subcommand, so custom images only need one executable.
- Custom worker image guide, with binaries published per GitHub release.

**Platform**
- **OIDC configurable in Settings**, merged over environment values and applied at API
  startup.
- **Custom CA certificates**: the Helm chart mounts a CA bundle into every workload and
  sets `SSL_CERT_FILE` / `REQUESTS_CA_BUNDLE` / `NODE_EXTRA_CA_CERTS`; all Rust HTTP
  clients trust the bundle.
- **Dark/light theme** with a sidebar toggle, persisted and applied before hydration.
- Execution history filtering (active/archived/all) for executions, team runs and worker
  launches; terminal records can be archived and old runs pruned by age.
- Human-readable execution timeline (event-kind badges) and input/output panels that
  render text with a **Show raw JSON** toggle.
- Navigation grouped into Work / Library / Operations; Skills promoted to the top level;
  Connections renamed to **Connectors**.
- Unified provider picker in Agent Studio: one grouped dropdown that sets harness-managed
  or platform management automatically.
- Long-running execution mode selectable for agents (24/7).

**Delivery**
- GitHub Actions CI on push/PR/tags: rustfmt, clippy, tests (with PostgreSQL), cargo
  audit, pnpm audit, trufflehog, container builds for all six images, and release
  binaries for tagged versions.
- Helm chart image tags default to `.Chart.AppVersion`, matching the images CI builds for
  a release; the stale `api.tag` was removed.
- Docs rewritten and genericized (no environment-specific hosts or registry addresses);
  added a threat model, a documentation index, an architecture diagram that includes the
  Pi worker, and a release guide.

### Changed
- Workers consolidated to one binary (`orbit-worker` + `mcp-broker` subcommand).
- Container images are published to GHCR; Dockerfiles run `apt-get upgrade` and build for
  `linux/amd64`.
- Chart defaults now target local/test clusters (`platform.environment=development`,
  bundled PostgreSQL/NATS, dev auth) and generic registry/host names.

### Fixed
- Archiving an already-archived agent is idempotent instead of returning
  `cannot transition agent from archived to archived`.
- Archived agents are hidden from the default list and the dashboard, and can be restored
  or deleted permanently.
- Cursor round-trip test no longer fails on timestamp precision.
- Hard delete blocked by a foreign key now returns a clear conflict instead of a generic
  storage error.
- Stale service-worker caching removed so UI updates appear after a deploy.
- `deploy.sh` reuses existing worker/launcher tokens and sets the CORS and public base URL
  from the ingress host.

### Known limitations
- GitHub/GitLab **writes** still need live end-to-end verification.
- Claude Code and generic harness adapters are registered but fail closed.
- Inbound webhook verification and X posting are not implemented.
- Parallel subagents and conditional workflow branching are not implemented.
- Enterprise RBAC, quotas, shared connections and the marketplace are not started.

## [0.6.0] — 2026-11-10

### Added
- Native OpenCode **Agent Teams** (primary + native subagents) and execution-level
  skills.
- GitHub coding capabilities through the tool broker: repo checkout/commit/push, pull
  requests, checks, issue updates.
- **GitLab** connector: projects, group issues, boards, issues, merge requests,
  pipelines, repository actions.
- **Worker launcher** (`orbit-launcher`) for Docker and Kubernetes, with single-profile
  launch reuse and namespaced Pod RBAC; the control plane never receives cluster or
  Docker credentials.
- Connection editing with credential rotation, and worker profile editing.
- **OIDC web login** (Authorization Code + PKCE) for Dex and Keycloak.
- Agent Team repository/branch settings and automatic worker launch.
- Per-run L4 auto-approve override and execution grouping by Agent Team run.

### Changed
- Native Agent Teams are restricted to OpenCode; Pi uses pipeline mode.
- Repository checkout is skipped when no repository is configured.
- Connection capabilities and write approvals are presented in human-readable form.

## [0.5.1] — 2026-11-09

### Added
- Optional launcher service with Docker and Kubernetes drivers, protected by an internal
  launcher token.
- Durable `worker_launches` records with status, runtime id, worker id and errors.
- Workers UI: Launch, Stop, status and last-200-lines logs.
- Helm launcher Deployment, namespaced Pod RBAC and launcher token plumbing.

### Changed
- Worker launches use profile-scoped, per-launch tokens revoked on stop.
- The launcher is opt-in; without it the API returns `launcher_unavailable`.

## [0.5.0] — 2026-11-08

### Added
- Scoped **worker tokens** (`orbit_wk_…`), stored as SHA-256 hashes with
  organization/profile/harness scoping, revocation and last-used tracking.
- Worker API: session, heartbeat, claim, execution context, running, events, completion,
  cancellation, approvals, audit and connector credential leases.
- Worker token administration endpoints (list, create, revoke).

### Changed
- Workers no longer receive `ORBIT_DATABASE_URL`, `ORBIT_NATS_URL` or
  `ORBIT_MASTER_KEY`; they use `ORBIT_API_URL` + `ORBIT_WORKER_TOKEN`.
- Scheduled triggers and orphaned-execution reaping moved into `orbit-api`.
- Per-execution containers inherit the worker token instead of database credentials.

### Removed
- Worker-side scheduler, reaper and direct PostgreSQL/NATS event publishing.

## [0.4.0] — 2026-11-07

### Changed (breaking)
- Renamed the agent-framework concept from **runtime** to **harness** across domain,
  database, API, worker, profiles, UI and docs.
- Database: `runtimes` → `harnesses`, `runtime_kind` → `harness_kind`, `runtime_id` →
  `harness_id`, `worker_heartbeats.runtimes` → `harnesses`; agent specs
  `spec.runtime` → `spec.harness`; provider kind `runtime_managed` →
  `harness_managed`.
- API: `/api/v1/harnesses`; `runtime`/`runtimeKind` → `harness`/`harnessKind`;
  `harnesses:read` permission; regenerated TypeScript client.
- Env: `ORBIT_WORKER_HARNESSES`, `ORBIT_EXECUTION_IMAGE_<HARNESS>`; profile field
  `harnessKind`.

### Added
- ADR-0008 defining harness/worker/sandbox/provider/model terminology and the
  worker-token + launcher design.

## [0.3.0] — 2026-11-05

### Added
- **Skills library**: organization-scoped `SKILL.md` packages with supporting files,
  versioning, CRUD API/UI and audit; attachable to agents in Agent Studio.
- Skill materialization into `.pi/skills/<slug>` and `.opencode/skills/<slug>`.
- **Worker profiles**: pin harness, image/tag, target, execution driver, agents, skills,
  provider/model and an external auth secret reference; workers claim only their agent
  allowlist and materialize profile skills.
- Bootstrap generator (`/worker-profiles/{id}/bootstrap`) for Docker, Compose and
  Kubernetes.
- **Pi harness adapter** (tested against Pi 1.5.0): headless JSON, sessions, read-only
  tools at L1/L2, cancellation/timeout and `.pi/mcp.json` broker bridge generation.
- Pi worker image and `make worker-pi` host runner.
- `GET /api/v1/workers` and the Active workers view.

### Changed
- Skills are validated (slug, required SKILL.md, safe relative files) before persistence.

## [0.2.1] — 2026-11-03

### Added
- `harness.placement` on agents (`auto`, `local`, `docker`, `kubernetes`), editable in
  Agent Studio; workers advertise a target so the claim query only takes `auto` or
  matching-target executions.
- `GET /api/v1/workers` with target, hostname and liveness.
- Queued executions get a timeline event and an in-UI diagnostic about matching workers.
- Worker target defaults: host native `local`, Compose/Docker `docker`, Kubernetes
  `kubernetes`.

### Fixed
- Reaper grace uses claim time instead of creation time, so retried queued executions are
  not reaped before their worker writes a heartbeat.
- `/api/v1/workers` was missing from the router.

## [0.2.0] — 2026-11-02

### Added
- **Approvals UI**: pending queue with agent/execution context, argument preview,
  approve/reject with comment, recent decisions, navigation badge and dashboard banner.
- **Worker lease and reaper**: `worker_heartbeats`; stale running executions fail with a
  diagnostic and their pending approvals expire.
- **Per-execution Docker containers** (`ORBIT_EXECUTION_DRIVER=docker`) running the worker
  in single-execution mode with its own workspace, broker and heartbeat.
- Real GitHub read verified end-to-end: OpenCode → MCP → broker → GitHub `issues.list`.

### Changed
- Execution timelines hide reasoning, step and usage noise by default; "show verbose"
  reveals the raw stream.
- OpenCode `PWD` is pinned to the per-execution workspace; stderr diagnostics are gated
  behind `debugLogs`.

## [0.1.4] — 2026-11-01

### Added
- Architecture drawings (SVG/PNG) and Mermaid diagrams for context, execution sequence,
  broker policy path, domain model and deployment topology.

### Fixed
- Agent pages crashed with `crypto.randomUUID is not a function` when served over plain
  HTTP. Added secure-context-safe `randomId()` / `copyText()` helpers with fallbacks.

## [0.1.3] — 2026-10-31

### Added
- **OpenCode runtime execution** (`opencode run --standalone --format json`): NDJSON event
  mapping, session capture/resume, cancellation via process-group termination, per-
  execution workspace and `AGENTS.md` instructions. Tested against OpenCode v2.2.5.
- Separate `orbit-worker-opencode` image and Helm Deployment; credentials imported from an
  `opencode auth export` Secret by an init container.
- **X read connector**: `posts.search` hydration with untrusted `externalContext`.
- Agent Studio connection picker and an execution payload/query field.

### Fixed
- Agent detail no longer crashes when a connector binding has a null `connectionId`.
- OpenCode worker gets writable XDG directories and imports auth before start.

## [0.1.0] — 2026-10-29

### Added
- Monorepo: Rust workspace (`crates/*`, `services/*`), pnpm workspaces (`apps/*`,
  `packages/*`) and a generated OpenAPI TypeScript client.
- **Domain** (`orbit-domain`): versioned `orbit.io/v1` agent definitions with semantic
  validation, autonomy levels 1–4, capability evaluation, execution state machine and
  keyset pagination cursors.
- **Contracts** (`orbit-contracts`): harness adapter trait with capabilities,
  lifecycle, execute/resume, streaming events, cancellation and typed errors; provider
  contracts with explicit compatibility flags; connector manifest/capability/effect
  model; event envelope and subject filters.
- **Persistence** (`orbit-store`): PostgreSQL schema with migrations, org scoping, keyset
  pagination, atomic execution claiming and terminal-state guards.
- **Security** (`orbit-security`): OIDC JWT validation with JWKS caching, hashed API
  tokens, role/permission matrix, AES-256-GCM secret store with reference-bound AAD.
- **Events** (`orbit-events`): `EventBus` abstraction with in-memory and NATS JetStream
  drivers; PostgreSQL remains the source of truth.
- **Harness registry** (`orbit-harness`): mock harness (streaming, cancellation,
  timeouts), OpenCode adapter scaffold and registered descriptors for Claude Code, Pi and
  generic, plus a shared contract test harness.
- **Control-plane API**: health/readiness/metrics, OpenAPI 3.1 + Swagger UI, agent
  CRUD/versioning/export/activate/pause, manual executions with idempotency, concurrency
  and rate limits, execution events and SSE replay, harnesses, providers, connectors,
  connections, triggers, dashboard, audit log, API tokens and user/org endpoints.
- **Execution-plane worker**: durable queue polling, concurrency limiting, harness
  dispatch, incremental event persistence, cancellation watcher and timeout enforcement.
- **Frontend**: Next.js App Router + Tailwind + shadcn-style components, TanStack Query,
  Zod schemas, typed client, mobile-first shell and PWA assets.
- **Deployment**: Dockerfiles (multi-stage, non-root), a Compose stack and a Helm chart.
- **Documentation**: architecture, domain model, harness/provider/connector contracts,
  events, API design, ADRs 0001–0007, local development, testing and deployment guides.

### Known limitations
- The OpenCode adapter validated configuration and performed a real CLI preflight but did
  not execute tasks yet.
- Claude Code, Pi and generic harnesses were descriptor-only and failed closed.
- Connector invocation, OAuth flows, webhook verification and approvals were not
  implemented.
- Per-execution isolation was not implemented; the mock harness ran in the worker process.
- The web app did not implement the browser OIDC login flow; dev auth was the supported
  local mode.
