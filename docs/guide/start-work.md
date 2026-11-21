# Start work

Start work (`/work`) is where you hand a task to an agent or an agent team. You choose who
does the work, describe the task, decide whether it runs now or on a schedule, pick where
it runs, and see what happens afterwards. Open it with the **Start work** button in the
sidebar, or find it in the command palette. You need the `executions:create` permission and
use access to the agent or team.

The page shows one section at a time (Who, What, When, Where, Then). The section list shows
a summary of each choice and marks what is missing. On a phone the sections are a row of
steps and the action button stays at the bottom of the screen.

## Choose who does the work

To pick the executor, open **Who** and choose **One agent** or **An agent team**. Then pick
a card. Agents that are Active or Draft are offered, and a Draft agent carries a "Draft"
badge: you can run it by hand, but its schedules only fire once it is activated. Each
agent card shows its harness (or "Any harness") and autonomy level. Archived and Paused
agents are not offered.

## Describe the task

To describe the work, open **What** and fill in:

- **Task** (required): the outcome you want, as you would describe it to a colleague.
- **Name** (optional): shown in the executions list instead of the agent name.
- **Repository** (optional, `owner/repo`): checked out into the workspace when the agent
  has a GitHub or GitLab connector that allows it. If the agent has no connector, Start
  work warns that the repository will not be checked out.

For a schedule, only **Task** is shown; it is sent to the agent every time the schedule
fires.

## Run now or on a schedule

Open **When** and choose one of these:

- **Now**: runs once, as soon as you start it.
- **On a schedule**: repeats. Schedules belong to one agent, so this is disabled for an
  agent team ("Schedules are set per agent, not per team").
- **On an event** and **From a connector** are shown disabled ("Not available yet").
  Event and webhook triggers can be saved on an agent, but nothing fires them yet; see
  [Agents](agents.md#triggers).

To set a schedule, choose how often it **Repeats** (Minutes, Hourly, Daily, Weekly, Monthly
or Custom) and fill in the matching fields, or choose Custom and enter a **Cron
expression** with five fields (minute, hour, day of month, month, day of week), for
example `0 9 * * 1-5`. The **Time zone** starts as your browser's time zone; UTC is used
when it is empty. The page refuses an invalid cron expression.

The button then reads **Create schedule**. It adds a schedule trigger (cron, time zone and
this task) to the agent by saving a new version of the agent, the same thing Agent Studio
does, and opens the agent page. Nothing runs until the schedule next fires. To change or
remove the schedule later, edit the agent's Triggers in Agent Studio.

A schedule fires only while the agent is **Active**, and skips a tick when the agent is
already at its concurrent-run limit. After five consecutive failures to start a run, ORBIT
disables the trigger. A schedule runs on the agent's own harness or the organization
default harness, so set one under **Settings → Platform → Default harness**
([Settings](settings.md)) if the agent has none; Start work refuses to create the schedule
otherwise.

## Choose where it runs

For a run you start now, open **Where**:

- **Placement**: **Anywhere**, **Local**, **Docker** or **Kubernetes**. This says which
  kind of worker may take the run. "Anywhere" leaves the agent's own placement in charge.
- **Harness**: **Agent default** (the agent's own harness, or the organization default),
  or a specific harness. A harness that is not available is disabled, and so is any
  harness other than the one an agent already has.

Below the harness, the page tells you whether a matching worker is online. If none is, it
lists the worker profiles for that harness, or offers **Create profile**. When you start
the work and no matching worker is online, Start work reuses an enabled profile for that
harness that covers the agent, or creates one named "Auto: <harness>", enables it and
launches it. If it cannot start a worker, the run stays queued until one connects; see
[Workers](worker-profiles.md#workers-page). For a schedule this section only explains that
runs go to any free worker. For an agent team only the harness choice is sent, not the
placement.

## Outputs and options

Open **Then** to see what happens afterwards:

- The enabled [outputs](outputs.md) that apply to the chosen agent or team, with a link
  to **Manage outputs**. Outputs are set up on the Outputs page, not here.
- **Auto-approve writes** and **Keep the agent running** are shown disabled ("Not
  available from here yet"). Whether writes need approval follows the agent's autonomy
  level ([Approvals](approvals.md)); each piece of work runs once and then stops. Team
  runs can auto-approve from the "Run <team>" dialog on the team page.

## Start

Choose **Start work**. For an agent, ORBIT creates an execution and opens its live
timeline ([Executions](executions.md)). Pressing the button twice, or retrying after a
network error, returns the first run because the page sends an idempotency key. For an
agent team, ORBIT starts a team run and opens its page.

If something is missing, the page shows "Fix before starting" with the sections that need
attention.

## API

- `POST /api/v1/agents/{agent_id}/executions`: body `task`, `name`, `payload`,
  `harness` (`harnessOverride` is an older name), `placement` and `idempotencyKey`. A
  placement other than `auto`, `local`, `docker` or `kubernetes` is a validation error.
- `POST /api/v1/agent-teams/{team_id}/runs`: body `task`, `payload`, `autoApprove`,
  `harnessOverride`.
- `PATCH /api/v1/agents/{agent_id}` with `expectedVersion`: how a schedule is added.
- `POST /api/v1/worker-profiles`, `.../{id}/enable` and `.../{id}/launch`: used for the
  automatic worker.
