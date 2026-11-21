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
> stream is a retained log (7 days); `subscribe` is a plain core NATS subscription, and
> nothing in the API consumes it today. Durable pull consumers with explicit ack remain a
> possible future addition.

Readiness: `GET /readyz` depends on the database only. The bus is reported in the body
(`checks.eventBus`, `degraded: ["eventBus"]`) but an outage does not fail the probe.

## Subjects

Source: `orbit_contracts::events::subjects`.

```
orbit.execution.requested      execution accepted, awaiting dispatch
orbit.execution.dispatched     worker claimed the execution
orbit.execution.updated        status transition (also drives the UI tail)
orbit.execution.event          individual harness event
orbit.execution.cancel         cancellation requested
orbit.trigger.fired            schedule/webhook/connector event matched a trigger
orbit.connector.event          normalized inbound connector event
orbit.agent.event              agent-emitted event (reserved for agent-to-agent flows)
orbit.worker.heartbeat         worker liveness
```

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

- At-least-once delivery; consumers are idempotent.
- `processed_events(event_id PRIMARY KEY)` records handled events; duplicate inserts are
  ignored and the message acked.
- Execution claiming is a SQL compare-and-set (`queued → dispatched`), so duplicate
  delivery cannot double-run an execution.
- Orphan reaping: the API marks `running` executions whose worker heartbeat is older
  than 90 seconds as `failed` with a `worker_lost` diagnostic and publishes
  `orbit.execution.updated`. Expired approvals on that execution are marked `expired`.
- Terminal states are never overwritten.

## Scheduling and dispatch

All background loops run in `orbit-api`; workers are token-only and never touch the
database or the event bus directly.

- **Continuous runtimes** (every 5s) launch the next execution once the previous one is
  terminal. `failed`, `timed_out` and `rejected` all count towards backoff and the
  5-failure stop; only successes (and explicit cancellations) reset the counter.
- **Scheduler** (`background::scheduler_tick`, every 15s) claims due `schedule` triggers
  with `FOR UPDATE SKIP LOCKED`, so multiple API replicas cannot double-fire one trigger.
  Each run creates an execution and publishes `orbit.execution.requested`. Failures
  increment `consecutive_failures`; a trigger is auto-disabled after `max_failures`.
- **Team run advance** (every 2s) advances pipeline team runs after a step finishes. It
  walks all running runs in bounded keyset pages under the existing per-run lease, so a
  backlog of more than one page cannot starve newer runs.
- **Work-source dispatcher** (every 15s) polls GitHub/GitLab issues for Agent Teams with
  an enabled work source, takes a per-team advisory lock, claims issues via labels, and
  enqueues deduplicated `team_queue_items`.
- **Memory pruner** (hourly) deletes expired agent memory entries.
- **Retention** (every 6h) archives terminal executions, team runs and worker launches
  older than `ORBIT_ARCHIVE_AFTER_DAYS` (default 30, `0` disables).
- **Audit retention** (daily, opt-in) deletes `audit_logs` rows older than
  `ORBIT_AUDIT_RETENTION_DAYS` (unset or `0`, the default, keeps them forever). Deletions
  run in 5,000-row batches under supervision, and a cluster-wide advisory lock keeps only
  one API replica pruning at a time.
- **Execution reaper** (every 15s) marks orphaned running executions failed.
