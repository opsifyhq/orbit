# Outputs (webhooks, Slack, Mattermost)

Outputs deliver an execution result after it reaches a terminal state. Manage them under
**Outputs** in the sidebar (or via the API below).

## UI

The Work wizard's **Then** step lists outputs configured for the selected agent/team before
the run is created.

**Outputs** lists every output with whether it will send: **On**, **Off**, or **Cannot
send** (no webhook URL or no events). **New output** opens Output Studio:

- **Destination**: Slack, Mattermost or Webhook, a name, and the incoming webhook URL
  (write-only; leave it empty when editing to keep the stored one).
- **Trigger**: every agent, one agent, or one agent team, and which results send a message
  (succeeded, failed, cancelled).
- **Message**: an optional note added under the run summary, an example of the message, and
  whether the output is on.

`/outputs/new?agent=<id>` or `?team=<id>` starts with that agent or team chosen. The output
page has **Send test** (a test message, with the note), **Edit**, and **Turn off/on** and
**Delete** under ⋯. Runs that finish while an output is off are not sent later.

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
- `agentId`: one agent, or `null` together with `teamId` for all executions
- `teamId`: one Agent Team; fires once when the team run finishes (not per child execution)
- `events`: `succeeded`, `failed`, `cancelled` (default: all three)
- `webhookUrl`: write-only; encrypted at rest and never returned
- `template`: an optional note added under the run summary (plain text, no placeholders)

For Slack/Mattermost use an **incoming webhook URL**. The body is `{"text": "..."}`.

The text is `<agent> <status> — <task>` (or `Agent team run <status> — <task>`), then up to
3,000 characters of the output or error, then the note after a blank line.

For `webhook`, ORBIT POSTs JSON with `event` (`execution.<status>`, `team_run.<status>` or
`test`), `deliveryId`, `text`, and an `execution` or `teamRun` object.

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

Not yet implemented: HMAC signing for generic webhooks, a delivery log in the UI, and
placeholders in the note.
