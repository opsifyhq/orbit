# Executions

Executions is the record of every time an agent ran: what it was asked, what happened
while it ran, and what it returned. Use the list to find a run and the execution page to
follow it live, read its output, cancel it or run it again. Agent team runs have their own
pages. You need `executions:read` to see runs, `executions:create` to run again and
`executions:cancel` to cancel.

## Find an execution

To find a run, open **Executions** in the sidebar. The **Execution status** switch groups
every status into one filter:

| Filter | Statuses it shows |
|---|---|
| All | every status |
| In flight | `queued`, `dispatched`, `running` |
| Needs approval | `awaiting_approval` |
| Succeeded | `succeeded` |
| Failed | `failed`, `timed_out` |
| Cancelled | `cancelled`, `rejected` |

The Overview tiles link to the same groups. The **Show** menu chooses **Current**,
**Archived** or **Current and archived**. The filter and the menu are kept in the page
address, so you can bookmark a filtered view. Runs that belong to a team run are grouped
under an "Agent team run" header that links to the team run. The list loads the latest 50
runs and shows "Showing the latest N of M"; choose **Load older** for more.

## Follow and read one execution

Open a run to see its page. While the run is active the **timeline** updates live, and
the page shows the status, the agent, the task and when it started. Once there is a
result, the **Output** leads; for a run that ended without one (a timeout or a
cancellation), the card shows "Last message from the agent". Other cards are **Input**,
**Error detail** and a details list: Trigger, Harness, Model, Duration, Tokens (in and
out), Created, Started, Finished, Timeout, Worker, Session and Idempotency key.

To narrow the timeline:

- **Event detail**: **Story** shows the key events; **Everything** shows all of them.
- **Event type**: All, Messages/output, Tool calls, Approvals, Errors, System/lifecycle.
- **Tool or action**: search by name, capability or connector. **Effect**, shown when the
  run has tool calls: All effects, Read, Write or External side effect.
- **Follow new events** keeps the view at the latest event, and **Group repeated tool
  calls** collapses repeats. The filters stay in the page address.

A run in `awaiting_approval` shows "Paused: waiting for approval" with **Approve** and
**Reject** ([Approvals](approvals.md)). A `queued` run shows "Waiting for a worker" and
what kind of worker it needs, with a link to **Workers**.

## Cancel or run again

To stop an active run, choose **Cancel** and confirm "Cancel execution". A run that is
still queued is cancelled at once; for a running run the harness is told to stop, and work
already done is not rolled back.

To repeat a finished run, choose **Run again**. ORBIT queues a new execution with the same
task, input and harness and opens it. Only finished runs can be run again. A step of an
agent team run cannot be run again on its own (409); the page shows **Open team run**
instead, and you start a new team run from the team.

## Statuses and retention

A run moves from `queued` (waiting for a worker) to `dispatched` (a worker took it), then
`running`, and ends as `succeeded`, `failed`, `timed_out`, `cancelled` or `rejected`.
`awaiting_approval` remains a valid in-flight state for older and imported runs, and the
reapers and in-flight metrics still cover it; current workers wait for an approval while the
run stays `running`, with the wait added to its timeout clock. Nothing in the current API or
worker sets `rejected`; the Cancelled filter includes it. An approval whose run has already
ended is expired and cannot be decided (409).

- `timed_out`: the run was still running when its time limit passed. The clock starts when
  the harness reports `running`, so checkout and worker start-up do not count, and time
  spent waiting for an approval is added. Raise the timeout in Agent Studio ("Run limits")
  or give the agent a smaller task.
- `worker_lost` (a `failed` run): the worker running it stopped sending heartbeats for 90
  seconds, or restarted with a new boot id while the run was still active, so the outcome
  is unknown. Check whether it changed anything before running it again.
- `harness_unresponsive` (a `failed` run): the worker is still sending its own heartbeats
  but stopped reporting this run for 90 seconds, so the harness may be hung. Check for side
  effects before running it again.
- `worker_did_not_start` (a `failed` run): a worker took the run but never started it.
  ORBIT ends it after 10 minutes without activity, or one minute after you cancel it. The
  worker's logs on the Workers page usually say why, for example a worker older than the
  platform.

ORBIT archives finished executions, team runs and worker launches older than
`ORBIT_ARCHIVE_AFTER_DAYS` (30 by default; Helm `platform.archiveAfterDays`; a value of 0
turns it off). History is kept: archived runs leave the default list and appear under
**Show → Archived**.

## Team runs

**Agent teams → Team runs** (`/agent-teams/runs`) lists team runs with a filter field
(task, team or status) and a **Show runs** switch: Active, Archived or All. A team run
page shows the task, the team, repository, start and finish times and "Auto-approved" when
the run was started with auto-approve. Under **Steps** it shows "N of M steps done", each
step with its agent, status, duration and an **Open execution** link; the first step is
marked "Primary". Steps that have not started show "Up next". If a team run failed, the
page shows "The run failed" with the error.

## API

- `GET /api/v1/executions`: query `status` (one or comma-separated, for example
  `failed,timed_out`), `agentId`, `archived`, `limit`, `cursor`.
- `GET /api/v1/executions/{id}`, `.../events` (query `afterSeq`, `kind`, `tool`, `q`,
  `limit`) and `.../events/stream` (server-sent events).
- `POST /api/v1/executions/{id}/cancel`, `POST /api/v1/executions/{id}/rerun`.
- `GET /api/v1/team-runs` (query `teamId`, `archived`, `q`, `limit`, `cursor`): returns
  `{ items, nextCursor, total }`; pass `nextCursor` back for older runs.
  `GET /api/v1/team-runs/{id}` returns one run with its steps.
- `POST /api/v1/team-runs/{id}/cancel`: stops a running team run. Its live execution is
  asked to stop and the run's queue item, if any, is cancelled. Deleting an agent team
  does the same for all of its runs.
