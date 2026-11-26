# ORBIT UI test plan

A practical, repeatable user flow for validating the web experience. Run it against the
fastest environment available:

- **Local Compose** — `http://localhost:3000` (recommended; see `docs/development/local.md`)
- **Kubernetes** — `http://orbit.example.com` (slower, but validates ingress/proxy/SSE)

Use a fresh database for the full flow (`docker compose ... down -v` locally). Dev auth is
enabled, so the browser needs no login; the web container injects the dev token server-side.

Time: ~25 minutes for flows 1–5, plus ~25 minutes for flows 6–14 and ~25 minutes for
flows 15–19.

---

## Flow 1 — First-run overview (2 min)

| Step | Expected |
|---|---|
| Open `/` | Overview renders; one status strip with four figures (running, executions 24 h, failed 24 h, agents); recent executions or empty state |
| Check seeded agents | The “Agents” list shows the demo agents with their status |
| Resize to 390×844 | Bottom tab bar appears (Overview, Executions, Approvals, Agents, More); **More** lists every page; no horizontal scrolling |
| Press `⌘K` / `Ctrl+K` | Command palette opens; typing an agent or page name and Enter navigates |
| Click an agent | Agent detail opens without a page error |

**Watch for:** console errors, skeleton that never resolves, `401/500` in the network tab.

---

## Flow 2 — Create, activate, and run a mock agent (6 min)

1. **Agents → New agent**
2. Identity: name `UI Smoke Agent`, description “UI test”.
3. Instructions: “You are a concise test agent. Summarize the task.”
4. Model: managed by **The harness**; leave provider/model empty.
5. Add a **Schedule** trigger: it starts as **Weekly**, weekdays at 09:00 in your own time zone, with a
   plain-English summary and the next three runs. Leave it enabled.
6. Autonomy: **Recommend** (level 2); under **Run limits** set timeout 120s, concurrency 1.
7. **Create agent** → redirected to detail page.

Expected:
- Status `Draft`, version 1, trigger listed, and a “This agent is a draft” notice with **Activate**.
- No validation summary. The summary rail in the form shows “Not set” for missing required input.
- The agent is visible in **Agents** under the default “All” filter.

Then:
8. **Activate** → status `Active`.
9. **Run agent** → task “Summarize the current ORBIT deployment.” → choose harness **Mock** (required
   when the agent has no harness and no organization default is set) → **Run agent**.
10. Execution page opens; status moves `queued → dispatched/running → succeeded`.
11. Timeline streams events (dispatch, harness session, logs, output).
12. Output is shown first once the run finishes (Text/JSON switch); Details shows tokens and
    duration; the agent link works; **Run again** queues the same task.
13. Open **Executions** → the run appears; filter by `Succeeded` and `Running`. The filter is in
    the URL (`?status=succeeded`) and survives opening a row and going back.
14. The agent’s **Executions** tab lists only that agent’s runs.

---

## Flow 3 — Failure, cancellation, and live streaming (4 min)

1. On the agent, **Run agent** with a task containing `slow` → queue.
2. Execution page: status `running`; click **Cancel** and confirm in the dialog (focus starts on
   **Keep running**).
3. Expected: `cancelled`, timeline shows the cancellation, no orphaned running state.
4. Run with a task containing `fail` → expected `failed` with an error block and error event.
5. While a run is active, refresh the execution page:
   - SSE reconnects, replays events from `seq 1`, then tails.
   - No duplicated events.

---

## Flow 4 — Versioning and editing (3 min)

1. **Edit** the agent → change instructions and autonomy to **L3 Prepare**.
2. **Save new version** → detail shows version 2.
3. **Versions** tab: v2 current, v1 preserved with checksum/timestamps.
4. Edit again and save without changes → saved, no new version. Set autonomy back to the
   version 1 level and save → version 3, with the same checksum as version 1.
5. **Configuration** tab: YAML matches the current version; **YAML** export downloads.

---

## Flow 5 — Credentials, providers, and audit (4 min)

### Models
1. **Model providers**: same layout as Agents (search, type filter with counts, status
   column, row checkboxes with **Change access**, count).
2. **Add provider** opens Provider Studio: type OpenAI-compatible, Connection name `Broken
   Gateway`, base URL `https://example.invalid/v1`, credential `sk-test`. **Test
   connection** shows "Test failed" with the reason. Saving without a default model jumps
   to Models; set `test` and save.
3. The provider page shows **Credential: Stored, encrypted**, never the value. **Test** →
   status **Test failed** and a callout; the list shows the same until reload.
4. Edit → change the default model and save (a tested provider keeps its catalog). Change
   the base URL without a credential: Connection asks for it again. Replace the credential;
   it never appears in the response or UI.
5. ⋯ **Disable** → status Disabled and a callout with **Enable**. At 360 px rows show the
   type on the meta line and Save is pinned above the tab bar.

### Connections
5. **Connectors** list: same layout as Agents (search, **All / Connected / Check failed / Not
   checked** filter, status column, row checkboxes with **Change access**, count). **Connect**
   on a catalogue card opens Connection Studio at `/connections/new?connector=<slug>` on the
   Account section.
6. **New connection**: service **X**, credential `invalid-test-token`, capability
   `posts.search` (Capabilities groups them as Read only / Change data / Act outside ORBIT,
   each with **Allow all**), Advanced config `{"query":"#orbit"}`. Create with no name or an
   invalid URL jumps to Account and lists the problem.
7. The connection page shows the allowed capabilities by group and **Credential: Stored,
   encrypted**; no secret in the network payload. Edit keeps the stored credential when the
   field is left empty. **Test**, then remove the connection from the ⋯ menu.

### Settings
8. **Settings**: identity, org, permissions, platform/feature readiness.
9. **New token** → token shown once; copy works; list shows prefix only; revoke it.
10. Recent audit activity contains agent/execution/provider/token actions.

---

## Flow 6 — Real OpenCode harness (5 min, optional)

Requires a host OpenCode worker (`make worker-opencode`) or the Kubernetes OpenCode
worker with imported auth.

1. Create an agent: harness **OpenCode**, model management **Harness**,
   provider `opencode-go`, model `deepseek-v4-pro`, timeout 240s.
2. Add a harness config override if the UI exposes it:
   `{"autoApprove": true, "standalone": true, "model": "opencode-go/deepseek-v4-pro"}`.
3. Activate, run: “Reply with exactly: UI_OPENCODE_OK”.
4. Expected: `succeeded`, output text contains `UI_OPENCODE_OK`, session ref present,
   token usage > 0, timeline shows text/tool/usage events.
5. Run a long task and cancel → `cancelled`.

---

## Flow 7 — GitHub read via the tool broker (3 min, working locally)

Prerequisite: a host OpenCode worker (`make worker-opencode`) or the Kubernetes OpenCode
worker, plus a GitHub connection for a public repo (no credential needed for reads).

1. Create a GitHub connection with `{"allowedRepositories":["cli/cli"],"defaultRepository":"cli/cli"}`.
2. Create an agent: harness OpenCode, GitHub connector, capability `issues.list`,
   autonomy L1.
3. Run: “Use tools.orbit.github_issues_list({owner:'cli',repo:'cli',state:'open'})
   and return 3 titles.”
4. Expected: `succeeded`; timeline shows the broker ready, the `issues.list` tool call,
   and completion. Reasoning/step/usage noise is hidden by default; click
   “show verbose” to reveal it.
5. `pwd` in a harness shell must print `/tmp/orbit-workspaces/<execution-id>`, never the
   ORBIT repository directory.

## Flow 8 — Connector read path (X) (3 min)

1. Create an X connection with a **real** bearer token (or keep the invalid one for a
   negative test) and config `{"query":"#orbit"}`.
2. Create/bind an agent: X connector, connection selected, capability `posts.search`,
   autonomy L1/L2.
3. Run with task “Summarize recent posts” and optional payload `{"query":"#orbit"}`.
4. Expected with a valid token: execution succeeds and the timeline shows
   “Fetched N X post(s)”; output summarizes untrusted posts.
5. Expected with an invalid token: execution `failed` with a clear 401 message
   (“check bearer token/API plan”), not a silent empty result.

---

## Flow 9 — Approvals / GitHub writes

These are the acceptance steps for the broker milestone; the backend/API exists, the
approvals UI is still being finished.

1. Create a GitHub connection (fine-grained PAT or read-only public access) with
   `allowedRepositories` set to a test repo.
2. Agent: harness OpenCode, GitHub connector, `issues.list` + `issues.comment`,
   autonomy **L3**.
3. Run “List open issues and propose a comment on #1”.
4. Expected: read tool succeeds; the write tool creates an approval and pauses.
5. Open **Approvals** (pending): approve with a comment.
6. Expected: comment appears on GitHub; execution continues; `tool.call.approved` audit.
7. Repeat and reject → write does not execute; agent receives a denied tool result.
8. Switch autonomy to L2 → writes are denied immediately (no approval prompt).
9. Switch to L4 with the write allowlisted → write executes without approval.

---

## Flow 10 — Start work (4 min)

| Step | Expected |
|---|---|
| Open `/work` (sidebar **Start work**) | The Studio layout: a section rail (Who, What, When, Where, Then) that shows each answer, one section on screen at a time, and the start button in the rail (pinned at the bottom on phones) |
| Who → Agent or Agent Team | Active and draft agents (drafts marked); teams are listed |
| What | Task, plus optional name and repository (hidden for a schedule: a schedule only carries its task) |
| When → Now | Default; **Start work** queues one execution |
| When → On a schedule | Schedule picker: Minutes, Hourly, Daily, Weekly, Monthly or Custom, with only the controls that frequency needs, a time zone (yours by default), a plain-English summary, the next three runs and the cron it stores. The button reads **Create schedule**; saving adds a schedule trigger with this task and time zone to the agent (new version) and opens the agent, where it reads e.g. “Every weekday at 09:00 Amsterdam time”. Disabled for an agent team |
| When → On an event / From a connector | Shown, disabled and marked “Not available yet” |
| When → Custom | Invalid cron shows why (for example “Invalid value for minute: 61”); **Edit as cron** from any other frequency opens it with the current expression |
| Where | Placement and harness cards, including **Agent default**; harnesses other than the agent's own are disabled. For a schedule, a notice that runs go to any free worker instead |
| Where → pick a harness | Matching worker profiles are shown, or a create link |
| Then | “Auto-approve writes” and “Keep the agent running” are shown disabled (not wired yet); outputs for the chosen agent/team |
| Start with something missing | Jumps to the first section with a problem, marks it in the rail and lists what to fix |
| Start work | Execution starts, a matching worker profile is created/launched, redirect to the execution |

**Watch for:** a schedule whose next runs disagree with the agent page after saving, and controls wrapping off-screen at 360px.

## Flow 11 — Agent Teams and queue (5 min)

| Step | Expected |
|---|---|
| Open `/agent-teams` → New | Primary + subagents; native mode disabled for non-OpenCode primaries |
| Save a team | Card shows mode, agents (primary/subagent), skills, optional work source |
| Run | Primary executes, then each subagent in order; steps visible in the run dialog |
| Queue | Enqueue `owner/repo#123`, run pending, retry/requeue/cancel per item |
| Team Studio → Agents → **Workflow** | The agents chosen so far become a line of steps on the canvas. **Examples** inserts a workflow; dragging from an output dot to another step adds a line in that output's colour and with its condition; letting go over empty canvas adds a connected step; the selected step offers its other outputs (done, failed, always) under its card without moving anything; selecting a step or line opens its fields below the canvas (beside it on very wide screens and in **Full screen**), with **Inputs** and **Outputs** sections; dragging a step enables **Tidy up**, which arranges the steps again; Cmd/Ctrl-click selects several steps and dragging one moves and saves them all; on a touch screen, dragging from an output to empty canvas adds a step too; **Undo** and **Redo** step through changes; **List** shows the same steps as a form and is the default on a phone |
| Workflow problems | Saving with a step that no line leads to, a step without an agent, or an outcome line without that outcome names the problem in the Agents section |
| Run a workflow team | The run page draws the graph: finished steps are coloured, the working step spins, steps and lines not reached are dashed; a step that ran twice shows `2×`; the step list shows **Run 2** and the reported outcome. A run that fails on a loop limit marks the step as **limit reached**. The Run dialog starts with a blank task; blank runs the team's goal. A workflow too large for the card pans, shows a small map and opens whole with the **Open larger** button; at 360px the picture is an overview and **Steps as text** is open |
| `/agent-teams` list | Same layout as Agents: search, **All / Picks up issues / On demand** filter, status column, row checkboxes with **Change access**, count under the list |
| **Runs** (header) | `/agent-teams/runs`: search plus Active/Archived/All filter; old `/agent-teams#runs` links land here |
| Stop a running team run | Start a run whose first step is slow (mock harness, task containing `slow`); its page shows **Stop run**. Confirming ends the run as cancelled and its steps show cancelled without a reload; a finished run, or a user without `executions:cancel`, has no button; the button fits at 360px |

## Flow 12 — Skills (5 min)

| Step | Expected |
|---|---|
| `/skills` | Rows like Agents: whole row opens the skill, columns Skill / Used by / Updated, **All / In use / Unused** filter, header checkbox selects all shown |
| **New skill** | Skill Studio at `/skills/new`: rail with Identity, Instructions, Bundled files, Access control; Create stays in view (pinned on phones) |
| Create with nothing filled | Jumps to Identity and lists “Enter a name.” and “Enter a slug.” |
| Name `Release Notes Writer!` | Slug fills in as `release-notes-writer` until edited by hand |
| Instructions → Preview | The body renders as Markdown |
| Bundled files → New file `scripts/check.txt` | Path error: scripts must end in an allowed extension |
| Upload a PNG and a `.py` | The PNG is refused (“not a text file”), the `.py` is added |
| Create | Opens `/skills/<id>`; its SKILL.md frontmatter has `name: <slug>` and the description from the form |
| Skill page | Formatted / SKILL.md views, bundled files expand to their contents, **Used by** lists agents, teams and worker profiles with links |
| A skill whose SKILL.md name or description differs (e.g. imported or older) | Warning on the skill page; Edit shows what Save will change; one Save fixes it and keeps other frontmatter keys |
| Delete a skill an agent uses | The confirmation names the agent; afterwards Agent Studio shows “A selected skill no longer exists” with a remove button, and the agent page marks it missing |
| Run an agent with the skill | Script is materialized and executable in the workspace |

## Flow 13 — Workers, launches, and env mappings (4 min)

| Step | Expected |
|---|---|
| `/workers` → New profile | Harness, target, driver, image/tag (defaults to `latest`), provider/model |
| Agents / Shared skills | **Select all** and **Clear** work |
| Env variable mappings | Add row, choose connection, set the env var name; saved and reloaded |
| `/environments` (and `/environment-mappings` redirects) | Same layout as Agents: search, **All / In use / Unused / Needs attention** filter with counts, columns Environment · Status · Used by · Updated, row checkboxes with **Change access** |
| **New environment** | Environment Studio with Details / Variables / Access control; saving with no name, no variable, a bad or duplicate name, or a variable without a source opens the section and lists why |
| Environment page | Variables with their source, **Used by** with **Attach to a worker** (opens the profile with it ticked), **Edit**, ⋯ **Duplicate** (opens a prefilled copy) and **Delete** (names the profiles that use it) |
| A variable whose connection was removed | Status **Needs attention**, a callout naming the variable, and the editor says to choose another connection |
| Enable + Launch | Launch appears in Launches; Pod/Job created when a launcher is configured |
| Without a launcher | Clear error: no worker launcher configured |
| Launches filter | Active / History / Archived |
| An ended launch in History | **Archive** moves it to Archived (badge "Archived"); **Delete** asks first, then removes it; a running launch offers neither; without `settings:write` neither is shown |
| Live workers | Workers that report a heartbeat are listed first, with Live or Stale state |

## Flow 14 — OIDC, theme, and execution detail (4 min)

| Step | Expected |
|---|---|
| `/settings` → OIDC / SSO | Save issuer/client/scopes; reload shows stored values; restart applies them |
| Theme (account menu at the bottom of the sidebar, or `⌘K` → “theme”) | Light / Dark / System; switches instantly; persists across reloads; no flash on load |
| `/settings` tabs | Account, API tokens, Single sign-on, Members and roles, Platform, Audit log; the open tab is in the URL hash |
| Execution detail | Timeline with **Story / Everything** and **Follow**; type chips, tool/action text and effect filters with URL state (`?events=tools&tool=github_issues_list`), hidden count and clear, expandable `tool × N` groups (errors/approvals expanded); filters keep applying to live events and do not reorder them; Details rail with trigger, harness, duration, tokens and IDs; Input and Output each switch between Text and JSON and can be copied |

## Flow 15 — Outputs (3 min)

| Step | Expected |
|---|---|
| `/outputs` | Same layout as Agents: search, **All / On / Failing / Off / Cannot send** filter with counts, columns Output · Status · Sends · Updated, count under the list; the whole row opens the output |
| New output | Output Studio with Destination / Trigger / Message; Create with no name or URL jumps to Destination and lists both |
| Trigger | **Every agent / One agent / One agent team**; choosing one shows its picker; no events chosen blocks saving |
| Message | The example updates with the note; for **Webhook** it shows the JSON body |
| Output page | Status On, **Send test**, **Edit**, ⋯ **Turn off** (status Off and a callout with **Turn on**), **Delete output** returns to the list |
| Edit | The URL field says a URL is stored; saving with it empty keeps it |
| Create with **On** unticked | The output starts Off |
| After a run that matches | **Recent deliveries** lists it (Delivered, Refused, Retrying with the next try, or Gave up) with its run, HTTP status and error; a refused or abandoned last delivery shows **Failing** in the list and a callout on the page |
| A member opens an organisation-wide output | Deliveries show their status but not which run they were about |
| 360 px | Rows collapse to name, status and one meta line; Save is pinned above the tab bar |
| A URL that points at `http://169.254.169.254/` | Refused; Destination shows the server's reason (cloud-metadata addresses are always blocked) |
| An organisation-wide output as a non-admin | Refused: only an owner or admin may manage it |

## Flow 16 — Access teams, sharing and roles (5 min)

Needs a licence with `teams`, `resource_permissions` and `custom_roles` (see
[licensing.md](licensing.md), “Local test license”). Without one the checks below show the
licence messages instead.

| Step | Expected |
|---|---|
| `/teams` without a licence | A notice “More teams need a license”; the Default team exists |
| `/teams` with a licence → New team | Name and slug; the team appears in the list |
| Add a member | Choose a user and a role: Lead (manages), Member (uses) or Viewer (sees) |
| **Agents** list → tick rows → **Change access** | Dialog “Change access for N selected”: owner, team and visibility (private, team, organisation) |
| Open the same agent as a member of another team | The agent is not listed and its URL shows “not found”, not “no access” |
| Settings → **Members and roles** → new custom role | Permissions are picked from the catalogue; you cannot add one you do not hold |
| Change a user's role | The audit log shows `user.role_changed`; you cannot change your own role |

## Flow 17 — Harnesses and the in-app handbook (3 min)

| Step | Expected |
|---|---|
| `/harnesses` | Harnesses grouped as Available (mock, OpenCode, Pi) and Planned (Claude Code, generic); a “Workers reporting in” section lists which harnesses each connected worker offers |
| `/documentation` | The handbook opens without network access, with the sidebar from `docs/navigation.yaml` |
| Open a page and use a link between pages | Navigates inside the handbook; no page is missing from the sidebar |

## Flow 18 — Export and import (4 min)

| Step | Expected |
|---|---|
| Agent page → **Export as YAML** | A YAML file downloads; it holds the agent spec and no secret values |
| Skill page → **Export SKILL.md** and **Export with files (YAML)** | Both download |
| Agents list → **Import** | Dialog “Import from file” with a mode (Create only, Create or update, Update only); choosing a file runs a dry run that lists each item before anything is written |
| Import the exported agent again with **Create only** | The dry run reports a conflict for the existing name; with **Create or update** it reports an update |
| Tool policies | Managed through the API (`/api/v1/tool-policies`) or an import (kind Tool policy); there is no screen for them |

## Flow 19 — Bulk archive and delete (3 min)

| Step | Expected |
|---|---|
| Agents list → tick two agents | A selection bar with **Change access**, archive and delete |
| **Archive** | A preview explains mixed selections (already archived, in use) before you confirm; archived agents leave the list |
| **Archived** filter → **Restore agent** on the agent page | The agent returns as before |
| **Delete permanently** | Names the agents and says execution history is kept; a partial failure leaves the failed ids selected |
| Skills list → select a skill an agent uses → delete | The confirmation names the agents and teams that use it |
| Change the filter or search with rows ticked | Only ticked rows that are still on screen are counted and acted on |

---

## Negative and security checks

- No secret value appears in HTML, JSON responses, execution events, logs, or tool output.
- Read-only access cannot mutate: start the API with `ORBIT_DEV_USER_ROLE=viewer` and check
  that create, edit, run and delete controls are absent or refused with a clear message
  (dev mode is owner by default).
- Validation errors return field paths; the form highlights the section.
- Invalid cron, empty instructions, unknown harness/provider are rejected.
- Idempotency: double-click **Run agent** in the run dialog with the same key creates one execution.
- Destructive actions (archive, delete, revoke, cancel) ask in an in-app dialog that names the
  object; none use the browser’s native confirm.
- Deleting/archiving an agent preserves execution history by default.
- Unsaved changes (#439): on `/skills/new` and `/agent-teams/new`, type a name, then press
  browser Back, press Forward after going back to the form, and pick a page in the command
  palette (Ctrl/Cmd+K). Each opens "Leave without saving?"; **Keep editing** stays on the form
  with the draft and the URL intact, **Leave page** goes to the chosen page. A clean form
  navigates without a prompt, and saving never prompts.

## What to record

For each failure: page, action, expected vs actual, request id (response header
`x-request-id`), screenshot, and whether it reproduces after `down -v`.

### Workflow canvas keyboard and touch (#448)

- Tab to a step on the canvas, press Enter: the editor panel shows that step and a screen
  reader announces "Step … selected". Escape clears it.
- With a step selected, arrow keys move it 5px (Shift: 20px); after the key is released the
  new position stays when you select another step, and Tidy up is enabled.
- Connection dots react within 10px of the dot (touch or mouse), except where a line starts.
- Examples over an existing workflow open the in-app "Replace the current workflow?" dialog.
- With the OS "reduce motion" setting on, selecting a step and "show everything" do not animate.
