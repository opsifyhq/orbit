# Outputs (webhooks, Slack, Mattermost, Telegram)

Outputs deliver an execution result after it reaches a terminal state. Manage them under
**Outputs** in the sidebar (or via the API below).

## UI

Agent and Agent Team detail pages show their linked outputs and an **Add output** link
that opens Output Studio with that agent/team preselected. The Work wizard's **Then**
section ([Start work](start-work.md)) lists the enabled outputs that apply to the selected
agent or team before the run is created.

**Outputs** lists every output with whether it will send: **On**, **Failing** (its last
message was refused or gave up), **Off**, or **Cannot send** (no credential or no events),
and how its last delivery went. **New output** opens Output Studio:

- **Destination**: Slack, Mattermost, Telegram or Webhook, a name, and the destination
  credential (incoming webhook URL or Telegram bot token; write-only — leave it empty when
  editing to keep the stored one).
- **Trigger**: every agent, one agent, or one agent team, and which results send a message
  (succeeded, failed, cancelled).
- **Message**: an optional note added under the run summary, an example of the message, and
  whether the output is on.

For Telegram, Output Studio also asks for a numeric **chat id** and an optional numeric
**message thread id** (forum topic). The bot token is password-masked and write-only; the chat
id and topic id are returned by the API so they remain editable.

`/outputs/new?agent=<id>` or `?team=<id>` starts with that agent or team chosen. The output
page has **Send test** (a test message, with the note), **Edit**, and **Turn off/on** and
**Delete** under ⋯. Runs that finish while an output is off are not sent later.

Creating, changing, deleting and testing outputs needs `settings:write`.

## Telegram bot setup

1. Create a bot with [@BotFather](https://t.me/BotFather) and copy its token (only to ORBIT —
   never into chat, logs or a repository).
2. Add the bot to the target group or channel. For a channel, promote the bot to
   **administrator** with permission to post messages; otherwise Telegram rejects sends with
   a 400 and ORBIT shows the setup hint.
3. Find the numeric chat id (for example `-1001234567890`). `@channel` usernames are not
   accepted by `sendMessage`. A forum topic needs its numeric `message_thread_id`.
4. Create a Telegram output, then use **Send test**. The test message is
   `ORBIT test notification — your output is configured correctly.` plus the note (if any);
   Telegram error descriptions are shown in the failure toast.

This feature is **output-only**: ORBIT does not receive Telegram messages, approve runs over
Telegram, or expose Telegram as an agent tool. No media, keyboards or inline buttons are sent.

## Create via API

Slack / Mattermost / generic webhook:

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

- `kind`: `slack`, `mattermost`, `webhook` or `telegram`
- `agentId` or `teamId`: set one, not both (the API returns a validation error for
  both). With neither, the output applies to every execution in the organization, and only
  an owner or admin may create or change it.
- `teamId` fires once when the team run finishes, not per child execution.
- `events`: `succeeded`, `failed`, `cancelled` (default: all three)
- `webhookUrl`: write-only for webhook kinds; encrypted at rest and never returned
- `botToken`: write-only for Telegram; encrypted at rest, never returned, logged or published
  in an event. On `PATCH`, omit it to keep the stored token; changing the kind requires a new
  credential for that kind.
- `chatId` / `messageThreadId`: numeric Telegram destination. Both are returned by the API;
  only `chatId` is required.
- `template`: an optional note added under the run summary (plain text, no placeholders)

For Slack/Mattermost use an **incoming webhook URL**. The body is `{"text": "..."}`.

The text is `<agent> <status> — <task>` (or `Agent team run <status> — <task>`), then up to
3,000 characters of the output or error, then the note after a blank line.

For `webhook`, ORBIT POSTs JSON with `event` (`execution.<status>`, `team_run.<status>` or
`test`), `deliveryId`, `text`, an `execution` object (`id`, `agentName`, `status`, `output`,
`error`) and, for a team run, a `teamRun` object (`id`, `teamId`, `status`, `output`,
`error`).

Telegram:

```bash
curl -X POST "$API/api/v1/outputs" \
  -H "Authorization: Bearer $ORBIT_TOKEN" \
  -H 'content-type: application/json' \
  -d '{
    "name": "release-telegram",
    "kind": "telegram",
    "events": ["succeeded", "failed"],
    "botToken": "123456:bot-token",
    "chatId": "-1001234567890",
    "messageThreadId": null,
    "template": "Release finished"
  }'
```

For `telegram`, ORBIT calls the official Bot API:

```http
POST https://api.telegram.org/bot<token>/sendMessage
Content-Type: application/json

{
  "chat_id": "-1001234567890",
  "text": "<rendered output>",
  "disable_web_page_preview": true,
  "message_thread_id": "42"
}
```

`message_thread_id` is omitted when no topic is configured. Text is split on line boundaries
into messages below Telegram's 4096-character limit, and every part carries a
`[ORBIT delivery <id> — part n/N]` footer when sent from the outbox. The delivery records how
many parts Telegram accepted; a retry after a failure on a later part resumes after that
counter instead of reposting the earlier parts. A crash between accepting a part and
recording the counter can still repeat that one part, so delivery remains at-least-once.

## Test

```bash
curl -X POST "$API/api/v1/outputs/$OUTPUT_ID/test" \
  -H "Authorization: Bearer $ORBIT_TOKEN"
```

The test sends `ORBIT test notification — your output is configured correctly.` (plus the
note, if any) and returns `502 output_delivery_failed` with the provider description when
delivery fails. Telegram 401/403 map to "bot token or chat access invalid"; chat-related 400s
include the group/channel administrator hint. Tokens are redacted from all error text.

Webhook URLs and the fully assembled Telegram URL (including the token path) are checked
against the outbound network policy before any request (loopback, link-local and
cloud-metadata targets are refused; see [Providers](providers.md#outbound-network-policy)).
The Telegram host is fixed to `api.telegram.org`. An operator may set
`ORBIT_TELEGRAM_API_BASE_URL` for a proxy, mock server or test harness; the override is
still guarded on every send and is never tenant-configurable.

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
- **Rendering**: the built-in run summary is used for every kind and the configured template
  note is appended to it. Telegram text is then split as described above.
- **Idempotency**: every delivery sends its id as `Idempotency-Key` and
  `X-Orbit-Delivery-Id` headers (and `deliveryId` in `webhook` bodies). Telegram parts use
  `<delivery-id>-<part>` as the idempotency key. Slack/Mattermost/Telegram ignore the headers;
  receivers should de-duplicate on the delivery id shown in the message footer. Telegram
  retries resume from the last accepted part (see above), so earlier parts are not reposted.
- **Failures**:

| Result | Action |
|---|---|
| 2xx | `succeeded` |
| 5xx, 408, 429, network error/timeout | retry with backoff (30 s x attempt); `dead` after 5 attempts |
| other 4xx / 3xx, missing credential, Telegram auth/chat error | `failed` immediately, never retried |
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
`responseStatus`, `error` (never the webhook URL or bot token), `nextAttemptAt` for a
pending delivery, and the run: `executionId` or `teamRunId`, `task` and `agentName`.
`GET /api/v1/outputs` adds `lastDelivery` (the same fields without the run) to each output
that has one.

The run fields are left out unless the caller may see the run: an output for one agent or
team shows them to anyone who can see that agent or team, an organization-wide output only
to an owner or admin, because it reports on every agent's runs.

Outputs that name an agent or team need manage access to it; an organization-wide output
is visible to everyone with `settings:read`.

Terminal delivery rows (`succeeded`, `failed`, `dead`, `skipped`) are pruned after 30 days
by the background reaper; pending and in-flight deliveries are never removed. Set
`ORBIT_OUTPUT_DELIVERY_RETENTION_DAYS` (Helm `platform.outputDeliveryRetentionDays`) to a
different number of days, or `0` to keep delivery history forever.

## Testing Telegram

No real bot token or chat id is needed for the automated mock-server suite; it covers
success, 401 auth failure, chat-not-found, long text splitting and the outbound guard.

## Optional live E2E

A live check is deliberately manual because it needs a real bot token and destination:

1. Create a private group, add the bot, and read its numeric chat id.
2. Create a Telegram output with that token/chat id in a disposable ORBIT organisation.
3. Press **Send test** and confirm the group receives
   `ORBIT test notification — your output is configured correctly.`
4. Run a short agent execution subscribed to `succeeded` and confirm its rendered summary
   (plus template note) arrives. Optionally force a `failed` run and a very long output to
   verify the error description and message splitting.
5. Delete or disable the output afterwards. Never commit the token or chat id to a workspace,
   fixture or issue.

Not yet implemented: HMAC signing for generic webhooks and placeholders in the note.
