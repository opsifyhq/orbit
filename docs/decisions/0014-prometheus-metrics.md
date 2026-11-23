# 0014 — Prometheus metrics from the API only

Date: 2026-11-21

## Context

Issue #297 asks for metrics that a Prometheus pod or a Prometheus Operator
`ServiceMonitor` can scrape, and for much wider coverage than the handful of unlabelled
counters `orbit-api` had. Two design questions came with it: whether workers and the
launcher should expose metrics too, and whether the Prometheus endpoint or an OTLP
metrics exporter is the long-term source.

Facts that shaped the answer:

- Workers are token-only clients of the API. Every execution transition, heartbeat,
  claim and launch already passes through the API or the database.
- Platform workers are launched per profile and often per execution (Kubernetes Jobs).
  They are short-lived, so a pull-based scrape would miss most of them, and giving each
  one a scrape port adds an inbound network surface to pods that run agent code.
- Execution state changes in several places, including SQL-only reapers and other API
  replicas. Counters kept only in handler code would miss some of them.
- The API has no OpenTelemetry SDK today; logs are structured `tracing` output.

## Decision

- **Prometheus text exposition at `GET /metrics` on the API is the source of truth for
  metrics.** No OTLP metrics exporter is added. The registry stays dependency-free.
- **Only the API is scraped.** Workers and the launcher expose no metrics endpoint. What
  operators need to know about the execution plane (runs by status and harness,
  durations, queue wait, workers online, claims, launches, reaped runs) is measured in
  the control plane.
- **Execution metrics are recorded where the row is written.** `orbit-store` reports
  created, started and finished transitions to an optional `ExecutionObserver`; the API
  registers its metrics registry as that observer. Reaped and cancelled runs are covered
  without instrumenting each caller.
- **Current-state gauges are read at scrape time** from the database (cached for 10
  seconds), the license and the connection pool, so they are correct with several
  replicas and after a restart.
- **Labels are low-cardinality by rule**: route templates, harness kinds, statuses and
  other fixed sets. No organization, user, agent, execution or worker labels. A
  per-family series cap is a backstop.
- **Exposure is configurable, open by default inside the cluster.** `/metrics` stays
  unauthenticated by default (the chart's ingress does not route it).
  `ORBIT_METRICS_ADDR` moves it to a dedicated listener and removes it from the API
  port; `ORBIT_METRICS_TOKEN` requires a bearer token.

## Consequences

- Not measured: what happens only inside a worker. That is tool-call counts and
  durations per connector capability, and workspace size
  (`orbit_worker_tool_calls_total`, `orbit_worker_tool_call_duration_seconds`,
  `orbit_worker_workspace_bytes` from the issue). Doing this properly means workers
  report aggregates to the API over the worker protocol, which is a contract change and
  a separate piece of work. Model counts per provider (`orbit_provider_models_count`) and
  per-query database timings are also not exported.
- Counters are per API replica and reset on restart; dashboards must use `rate()` and
  `sum()`. Database-backed gauges are identical on every replica and need `max()`.
- If traces or OTLP metrics are wanted later, an exporter can be added beside the
  registry; this needs a new ADR because it adds the OpenTelemetry SDK as a dependency.
- Three existing metric names changed shape (labels added, one replaced). They are listed
  in `docs/development/observability.md`.
