# Outputs (webhooks, Slack, Mattermost)

Outputs deliver an execution result after it reaches a terminal state. Manage them under
**Outputs** in the sidebar (or via the API below).

## UI

The Work wizard's **Then** section ([Start work](start-work.md)) lists the enabled outputs
that apply to the selected agent or team before the run is created.

**Outputs** lists every output with whether it will send: **On**, **Failing** (its last
message was refused or gave up), **Off**, or **Cannot send** (no webhook URL or no events),
and how its last delivery went. **New output** opens Output Studio:

- **Destination**: Slack, Mattermost or Webhook, a name, and the incoming webhook URL
  (write-only; leave it empty when editing to keep the stored one).
- **Trigger**: every agent, one agent, or one agent team, and which results send a message
  (succeeded, failed, cancelled).
- **Message**: an optional note added under the run summary, an example of the message, and
  whether the output is on.

`/outputs/new?agent=<id>` or `?team=<id>` starts with that agent or team chosen. The output
page has **Send test** (a test message, with the note), **Edit**, and **Turn off/on** and
**Delete** under ⋯. Runs that finish while an output is off are not sent later.

Creating, changing, deleting and testing outputs needs `settings:write`.

## Create via API

```bash
curl -X POST "$API/api/v1/outputs" \
  -H "Authorization: Bearer $ORBIT_TOKEN" \
  -H 'content-type: application/json' \
  -d '{
    "name": "release-slack",
    "kind": "slack",
    "agentId": null,
    "events": ["succeeded", "failed"],
    "webhookUrl": "https://hooks.slack.com/services/...",
    "template": "@release-team please review"
  }'
```

- `kind`: `slack`, `mattermost` or `webhook`
- `agentId` or `teamId`: set one, not both (the API returns a validation error for
  both). With neither, the output applies to every execution in the organization, and only
  an owner or admin may create or change it.
- `teamId` fires once when the team run finishes, not per child execution.
- `events`: `succeeded`, `failed`, `cancelled` (default: all three)
- `webhookUrl`: write-only; encrypted at rest and never returned
- `template`: an optional note added under the run summary (plain text, no placeholders)

For Slack/Mattermost use an **incoming webhook URL**. The body is `{"text": "..."}`.

The text is `<agent> <status> — <task>` (or `Agent team run <status> — <task>`), then up to
3,000 characters of the output or error, then the note after a blank line.

For `webhook`, ORBIT POSTs JSON with `event` (`execution.<status>`, `team_run.<status>` or
`test`), `deliveryId`, `text`, an `execution` object (`id`, `agentName`, `status`, `output`,
`error`) and, for a team run, a `teamRun` object (`id`, `teamId`, `status`, `output`,
`error`).

## Test

```bash
curl -X POST "$API/api/v1/outputs/$OUTPUT_ID/test" \
  -H "Authorization: Bearer $ORBIT_TOKEN"
```

Webhook URLs are checked against the outbound network policy (loopback, link-local and
cloud-metadata targets are refused; see [Providers](providers.md#outbound-network-policy)).

## Delivery

Delivery is **at-least-once** and driven by a background outbox loop on every API replica.

- **Enqueue**: each output keeps a high-water mark and only delivers runs that finished
  after it was created (or last re-enabled), so a new output never back-fills history and an
  API outage of any length still delivers every finished run once. Deliveries are unique per
  `(output, execution)` / `(output, team run)`.
- **Claim**: rows are claimed atomically (`FOR UPDATE SKIP LOCKED`) with a 120 s lease, so
  replicas and overlapping ticks never send the same delivery concurrently. A sender that
  crashes mid-send leaves the row `sending`; it is reclaimed after the lease expires and
  counts as an attempt.
- **Idempotency**: every delivery sends its id as `Idempotency-Key` and
  `X-Orbit-Delivery-Id` headers (and `deliveryId` in `webhook` bodies). Because a crash
  after the receiver processed the request but before ORBIT recorded it causes a resend,
  receivers should de-duplicate on this id. Slack/Mattermost ignore the headers.
- **Failures**:

| Result | Action |
|---|---|
| 2xx | `succeeded` |
| 5xx, 408, 429, network error/timeout | retry with backoff (30 s x attempt); `dead` after 5 attempts |
| other 4xx / 3xx, missing webhook secret | `failed` immediately, never retried |
| output disabled or deleted, run deleted | `skipped` (no request sent) |

- List outputs: `GET /api/v1/outputs`; update/delete via `PATCH/DELETE /api/v1/outputs/{id}`.

## Delivery history

The output page lists **Recent deliveries**: Delivered, Refused (`failed`), Gave up
(`dead`), Retrying (with the next try), Queued, Sending or Skipped, with the run each one
reported on, the HTTP status and error, and the number of attempts. Test messages are sent
directly and are not listed. When the newest delivery of an output that is on was refused
or gave up, the list shows the output as **Failing** and its page says why.

```bash
curl "$API/api/v1/outputs/$OUTPUT_ID/deliveries?limit=50" \
  -H "Authorization: Bearer $ORBIT_TOKEN"
```

Newest first; `limit` is 1–200 (default 50). Each item has `status`, `attempts`,
`responseStatus`, `error` (never the webhook URL), `nextAttemptAt` for a pending delivery,
and the run: `executionId` or `teamRunId`, `task` and `agentName`. `GET /api/v1/outputs`
adds `lastDelivery` (the same fields without the run) to each output that has one.

The run fields are left out unless the caller may see the run: an output for one agent or
team shows them to anyone who can see that agent or team, an organization-wide output only
to an owner or admin, because it reports on every agent's runs.

Not yet implemented: HMAC signing for generic webhooks, placeholders in the note, and
pruning old delivery rows.

Outputs that name an agent or team need manage access to it; an organization-wide output
is visible to everyone with `settings:read`.
