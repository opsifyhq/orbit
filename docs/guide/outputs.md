# Outputs (webhooks, Slack, Mattermost)

Outputs deliver an execution result after it reaches a terminal state. Manage them under
**Outputs** in the sidebar (or via the API below).

## UI

The Work wizard's **Then** section ([Start work](start-work.md)) lists the enabled outputs
that apply to the selected agent or team before the run is created.

To create one, open **Outputs → New output** and fill in:

- **Name**
- **Destination**: Slack, Mattermost or Webhook
- **Webhook URL**: stored encrypted and never shown again. When editing, leave it blank
  to keep the current URL.
- **Applies to**: "All agents", "One agent" or "One agent team". An agent sends one
  message per execution; a team sends one message when its whole run finishes.
- **Send when a run is**: Succeeded, Failed and Cancelled (choose at least one)
- **Note**: optional. It is stored with the output, but ORBIT does not add it to
  messages yet; delivered messages contain only the run summary.
- **Enabled**

Each output has an On/Off switch, **Send test**, **Edit** and a menu. **Send test** sends a
sample notification. Creating, changing, deleting and testing outputs needs `settings:write`.

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
    "webhookUrl": "https://hooks.slack.com/services/..."
  }'
```

- `kind`: `slack`, `mattermost` or `webhook`
- `agentId` or `teamId`: set one, not both (the API returns a validation error for
  both). With neither, the output applies to every execution in the organization, and only
  an owner or admin may create or change it.
- `teamId` fires once when the team run finishes, not per child execution.
- `events`: `succeeded`, `failed`, `cancelled` (default: all three)
- `webhookUrl`: write-only; encrypted at rest and never returned
- `template`: stored and returned, but not used when messages are built yet

For Slack/Mattermost use an **incoming webhook URL**. The body is `{"text": "..."}`.

For `webhook`, ORBIT POSTs JSON with `event` (`execution.<status>`), `deliveryId`, `text`,
an `execution` object (`id`, `agentName`, `status`, `output`, `error`) and, for a team run,
a `teamRun` object (`id`, `teamId`, `status`, `output`, `error`).

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

Not yet implemented: HMAC signing for generic webhooks, a delivery-log UI, and message
templates (the `template` field, shown as "Note", is stored only).

Outputs that name an agent or team need manage access to it; an organization-wide output
is visible to everyone with `settings:read`.
