# Events & Triggers

## Transport

``crates/orbit-events`` exposes:

```rust
#[async_trait]
pub trait EventBus: Send + Sync {
    async fn publish(&self, event: &OrbitEvent) -> Result<(), EventBusError>;
    async fn subscribe(&self, filter: SubjectFilter) -> Result<EventStream, EventBusError>;
    async fn health(&self) -> Result<(), EventBusError>;
}
```

Drivers: `memory` (tokio broadcast, development and tests) and `nats` (JetStream,
durable stream `ORBIT`). Selected with `ORBIT_EVENTS_DRIVER`; the API and worker must
agree.

`publish` on the NATS driver returns `EventBusError::Unavailable` when the message cannot be
handed to the client within 3 s (`DEFAULT_PUBLISH_ACK_TIMEOUT`). The JetStream
acknowledgement is then confirmed in the background with the same timeout; a missing stream,
missing ack or timeout is logged as a warning (it never blocks the request that published). Callers log and continue, because PostgreSQL is the source of
truth and workers poll the durable queue.

> Current state: there are **no durable JetStream consumers and no event replay**. The
> stream is a retained log (7 days, capped at 1 GiB by default; set
> `ORBIT_NATS_STREAM_MAX_BYTES` to change it, the oldest events are dropped first). An
> event with an idempotency key is published with it as `Nats-Msg-Id`, so a republished
> event is stored once. `subscribe` is a plain core NATS subscription, and
> nothing in the API consumes it today. Durable pull consumers with explicit ack remain a
> possible future addition.

Readiness: `GET /readyz` depends on the database only. The bus is reported in the body
(`checks.eventBus`, `degraded: ["eventBus"]`) but an outage does not fail the probe.

## Subjects

Source: `orbit_contracts::events::subjects`.

```
orbit.execution.requested      execution accepted, awaiting dispatch
orbit.execution.updated        status transition
orbit.execution.event          individual harness event
orbit.execution.cancel         cancellation requested
```

Five more subjects are defined but nothing publishes them yet:
`orbit.execution.dispatched`, `orbit.trigger.fired`, `orbit.connector.event`,
`orbit.agent.event` and `orbit.worker.heartbeat`.

Envelope (`OrbitEvent`):

```json
{
  "id": "0192f0c1-…",
  "subject": "orbit.execution.requested",
  "source": "orbit-api",
  "time": "2025-01-01T00:00:00Z",
  "org_id": "…",
  "idempotency_key": "exec:9f0c…",
  "payload": { "executionId": "…", "agentId": "…" },
  "trace_parent": "00-…"
}
```

`org_id`, `idempotency_key` and `trace_parent` are omitted when unset.

## Reliability

- PostgreSQL is the source of truth. A lost or duplicated bus message changes nothing,
  because workers claim work by polling the API and the browser reads events from the
  database.
- Execution claiming is a SQL compare-and-set (`queued → dispatched`), so an execution
  cannot run twice. Creating an execution is idempotent per organization on
  `idempotency_key`.
- Published events carry their idempotency key as `Nats-Msg-Id`; JetStream drops a
  duplicate that arrives within its two-minute duplicate window.
- Terminal states are never overwritten.
- The `processed_events` table from the first migration is reserved for future durable
  consumers. Nothing reads or writes it today.

### Execution reaper

Every 15 seconds the API ends executions that can no longer finish on their own. Each
pass is a conditional update, so several API replicas can run it and a worker that
reports in at the same moment wins.

| Pass | Condition | Result |
|---|---|---|
| Worker lost | `dispatched`, `running` or `awaiting_approval`, claimed at least 15 seconds ago (`COALESCE(started_at, created_at)`), and with no heartbeat in the last 90 seconds. A `dispatched` run whose worker has never sent a heartbeat is still starting (a launched container or Job pulls its image first) and is excluded from this rule entirely, left to the next rule | `failed` with `worker_lost`; open approvals on the execution become `expired` |
| Never started | `dispatched` and nothing has happened on it (claim, cancel request or event) for 600 seconds, or for 60 seconds after a cancel request | `failed` with `worker_did_not_start`, or `cancelled` |
| Timed out | `running` for longer than `timeout_seconds` plus a grace period | `timed_out` |

The 15-second floor is a claim grace period: every pass skips runs claimed less than 15
seconds ago, so a fresh worker has time to write its first heartbeat before it can be
reaped.

The timeout clock starts when the harness reports `running` (`running_started_at`), not at
dispatch, so repository checkout and worker start-up do not consume it. Time spent
waiting for an approval is added to the deadline. The grace period is
`ORBIT_EXECUTION_TIMEOUT_GRACE_SECONDS` (default 300); it leaves the worker room to
enforce the timeout itself and report the result. Each reaped execution gets an error
event and an `orbit.execution.updated` message.

The same loop deletes worker heartbeat rows that have been silent for
`ORBIT_WORKER_STALE_RETENTION_HOURS` (default 24).

## Scheduling and dispatch

All background loops run in `orbit-api`; workers are token-only and never touch the
database or the event bus directly.

- **Scheduler** (`background::scheduler_tick`, every 15s) claims due `schedule` triggers
  with `FOR UPDATE SKIP LOCKED`, so multiple API replicas cannot double-fire one trigger.
  It repeats bounded claim batches within a tick so more than ten due triggers can fire.
  Each run creates an execution and publishes `orbit.execution.requested`. A firing is
  skipped when the agent is not `active` or already runs its maximum number of concurrent
  executions. Failures increment `consecutive_failures`; a trigger is auto-disabled after
  `max_failures`. On shutdown the loop finishes the batch it already claimed within a short
  grace window instead of dropping those fires.
- **Team run advance** (every 2s) advances team runs after a step finishes.
- **Work-source dispatcher** (every 15s) polls GitHub/GitLab issues for Agent Teams with
  an enabled work source, takes a per-team advisory lock, claims issues via labels, and
  enqueues deduplicated `team_queue_items`.
- **Agent runtimes** (every 5s) start the next run of each continuous runtime when its
  interval has passed; see [domain model](domain-model.md).
- **Outputs** (every 5s) deliver the results of finished executions to their outputs.
- **Execution reaper** (every 15s): see above.
- **Worker image updates** (every 60s) relaunch workers whose profile follows the
  platform version after an upgrade: the old token is revoked, running executions drain,
  then the launcher replaces the worker.
- **License** (every 60s) reloads the installed license.
- **Memory pruner** (hourly) deletes expired agent memory entries.
- **Retention** (every 6h) archives terminal executions, team runs and worker launches
  older than `ORBIT_ARCHIVE_AFTER_DAYS` (default 30, `0` disables).
- **Output delivery pruning** (in the 15s reaper) deletes terminal `output_deliveries`
  rows older than `ORBIT_OUTPUT_DELIVERY_RETENTION_DAYS` (default 30, `0` disables) in
  bounded batches. Pending and in-flight deliveries are never pruned.
- **Audit retention** (daily) deletes `audit_logs` rows older than
  `ORBIT_AUDIT_RETENTION_DAYS` (unset or `0`, the default, keeps them forever).

Every loop runs under a supervisor that restarts it after a panic and stops it on
shutdown.

## Triggers

A trigger has one of four kinds. Only two of them start executions today:

| Kind | State |
|---|---|
| `manual` | Works: a run started from the web app or the API. |
| `schedule` | Works: a cron expression evaluated by the scheduler. |
| `webhook` | Can be created and stored, but never fires: there is no inbound webhook endpoint. |
| `event` | Can be created and stored, but never fires: nothing consumes the event bus. |

Agent teams can also be fed from a GitHub or GitLab issue queue (the work-source
dispatcher above); that path does not use triggers.
