# ORBIT UI test plan

A practical, repeatable user flow for validating the web experience. Run it against the
fastest environment available:

- **Local Compose** — `http://localhost:3000` (recommended; see `docs/development/local.md`)
- **Kubernetes** — `http://orbit.example.com` (slower, but validates ingress/proxy/SSE)

Use a fresh database for the full flow (`docker compose ... down -v` locally). Dev auth is
enabled, so the browser needs no login; the web container injects the dev token server-side.

Time: ~25 minutes for flows 1–5, plus ~25 minutes for flows 6–14.

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
4. Edit again and save the **same** configuration → expected conflict toast
   (“identical to an existing version”), no duplicate version.
5. **Configuration** tab: YAML matches the current version; **YAML** export downloads.

---

## Flow 5 — Credentials, providers, and audit (4 min)

### Models
1. **Models → Add provider**: name `Broken Gateway`, kind OpenAI-compatible,
   base URL `https://example.invalid/v1`, model `test`, credential `sk-test`.
2. Create → card shows `credential configured (encrypted)`; never the value.
3. **Test** → graceful failure toast (DNS/HTTP), not a crash.
4. Edit → replace credential; confirm it still never appears in the response or UI.

### Connections
5. **Connections → New connection**: connector **X**, credential `invalid-test-token`,
   capability `posts.search`, advanced config `{"query":"#orbit"}`.
6. Card shows `credential set`; no secret in the network payload.
7. Remove the connection afterwards.

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
| Runs list | Active/Archived/All filter works |

## Flow 12 — Skills with scripts (3 min)

| Step | Expected |
|---|---|
| `/skills` → New | SKILL.md content + bundled files JSON + **Upload files** |
| Upload `scripts/check.sh` | File appears in the JSON map |
| Save | No validation error for an allowed extension |
| Upload `scripts/check.exe` | Validation error: script extension not allowed |
| Run an agent with the skill | Script is materialized and executable in the workspace |

## Flow 13 — Workers, launches, and env mappings (4 min)

| Step | Expected |
|---|---|
| `/workers` → New profile | Harness, target, driver, image/tag (defaults to `latest`), provider/model |
| Agents / Shared skills | **Select all** and **Clear** work |
| Env variable mappings | Add row, choose connection, set the env var name; saved and reloaded |
| `/environment-mappings` | New mapping, Edit (rename a variable, switch Connection / Secret), Duplicate, Delete; invalid or duplicate names block saving with a reason; cards show "Used by" worker profiles |
| Enable + Launch | Launch appears in Launches; Pod/Job created when a launcher is configured |
| Without a launcher | Clear error: no worker launcher configured |
| Launches filter | Active / History |
| Live workers | Workers that report a heartbeat are listed first, with Live or Stale state |

## Flow 14 — OIDC, theme, and execution detail (4 min)

| Step | Expected |
|---|---|
| `/settings` → OIDC / SSO | Save issuer/client/scopes; reload shows stored values; restart applies them |
| Theme (account menu at the bottom of the sidebar, or `⌘K` → “theme”) | Light / Dark / System; switches instantly; persists across reloads; no flash on load |
| `/settings` tabs | Account, API tokens, Single sign-on, Members and roles, Platform, Audit log; the open tab is in the URL hash |
| Execution detail | Timeline with **Key events / Everything** and **Follow**; Details rail with trigger, harness, duration, tokens and IDs; Input and Output each switch between Text and JSON and can be copied |

## Negative and security checks

- No secret value appears in HTML, JSON responses, execution events, logs, or tool output.
- Viewer-style access cannot mutate (when RBAC UI lands); dev mode is owner.
- Validation errors return field paths; the form highlights the section.
- Invalid cron, empty instructions, unknown harness/provider are rejected.
- Idempotency: double-click **Run agent** in the run dialog with the same key creates one execution.
- Destructive actions (archive, delete, revoke, cancel) ask in an in-app dialog that names the
  object; none use the browser’s native confirm.
- Deleting/archiving an agent preserves execution history by default.

## What to record

For each failure: page, action, expected vs actual, request id (response header
`x-request-id`), screenshot, and whether it reproduces after `down -v`.
