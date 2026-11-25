# Approvals

Approvals is the queue where a person decides whether an agent may do something that
changes data or acts outside ORBIT. When the platform requires approval for a call, the
run pauses, the request appears here with the exact call, and nothing happens until
someone approves or rejects it. Approvals are enforced by the platform, never by the
agent's prompt.

## When an approval is created

A call needs approval when the platform decision for it is "require approval". That
happens in these cases:

- The agent's autonomy level is 3 (Prepare) and the call is a write or an external side
  effect (approval mode "Follow the autonomy level").
- The approval mode is "Ask for approval every time" or "Ask before external side
  effects", at level 3 or above. Both ask for every write and external side effect today;
  see [Agents](agents.md#autonomy).
- A [tool policy](tool-policies.md) rule with action `require_approval` matches the call.
  A rule can make a decision stricter, never weaker.

Reads on the connector allowlist do not need approval unless a tool policy rule asks for
it. At levels 1 (Observe) and 2
(Recommend) a write or external side effect is denied outright, with no approval request,
whatever the approval mode. At level 4 (Autonomous) allowlisted writes run without
approval unless a rule or the approval mode asks. Native harness tools such as `bash` and
`edit` follow the same rules.

While a request is open the worker waits on it and the run's timeout clock is paused for
that wait (the run normally remains `running`; `awaiting_approval` is kept for older and
imported runs). A request stays open for up to 15 minutes. A request that nobody answers
expires and counts like a rejection: the action is refused, and the agent cannot ask for
the same action again in that run. A run can only wait a limited total time for approvals;
once that is used up, further calls that need approval are denied and waiting counts
toward the run's timeout. A run can have at most 50 pending requests. When a run ends or is
cancelled, its pending requests are expired in the same transaction and can no longer be
decided (409).

## Decide in the queue

To decide, open **Approvals** in the sidebar. **Waiting for a decision** lists each request
with the agent, the call (for example "Comment on issue #12"), its arguments and when it
expires. The arguments are shown in full up to 64 KiB. A longer call shows only its
start and its end, with an "Arguments truncated" warning that gives the full size; approving
it allows the whole call, so reject it if you need to see the rest. Choose **Approve** or **Reject**. If a request has expired, the card says "Expired
...; the action was refused" and both buttons are disabled. **Recent decisions** shows the
status (Approved, Rejected or Expired), the agent, the time, the comment if there is one,
and a link to the execution.

You can also decide on the execution page, in the "Paused: waiting for approval" notice
([Executions](executions.md)).

The web app sends no comment. To record one, call the API with `comment`; it appears in
Recent decisions and the audit log (`approval.approved`, `approval.rejected`).

## Who can decide

To see the queue or decide, you need the `approvals:decide` permission. The built-in Owner
and Admin roles have it; Operator and Viewer do not ([roles](rbac.md)). You also need
**use** access to the agent that raised the request (you own it, you are a lead or member
of its team, or you are an owner or admin); someone who can only view the agent cannot
decide. The queue only lists requests for agents you can see.

Deciding after the request expired fails with 409 "approval expired", and deciding a
request that is already decided fails with 409 as well.

## Require a different approver

By default the person who started a run can approve its requests. Turn on the four-eyes
setting to stop that: with `approvalsRequireDifferentApprover` on, the user who requested
an execution cannot approve or reject the approvals it raises (403 "approvals require a
different approver"). The setting is off by default, and the web app has no control for it
yet. Set it through the API with the `settings:write` permission:

```bash
curl -X PUT "$API/api/v1/settings/approvals" \
  -H "Authorization: Bearer $ORBIT_TOKEN" -H 'content-type: application/json' \
  -d '{"approvalsRequireDifferentApprover": true}'
```

## Auto-approve

A team run started with `autoApprove` (the "Auto-approve writes for this run" checkbox)
requires `approvals:decide` and only switches the approval mode to Auto for agents already
at level 3 or 4; it never raises the level. See [Agent teams](agent-teams.md#auto-approve).

## API

- `GET /api/v1/approvals`: query `status` (`pending`, `approved`, `rejected` or
  `expired`), `executionId` and `limit` (50 by default, at most 200).
- `POST /api/v1/approvals/{id}/approve` and `.../reject`: optional body `{"comment": "..."}`.
- `GET/PUT /api/v1/settings/approvals`: the four-eyes setting.
