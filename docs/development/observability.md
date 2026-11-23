# Observability: Prometheus metrics

The API (`orbit-api`) serves Prometheus metrics in the text exposition format (0.0.4) at
`GET /metrics`. This page covers how to scrape it, what it exposes and how to keep it
private. Logs and health endpoints are described in
[the architecture overview](../architecture/overview.md#8-observability).

Only the API exposes metrics. Workers and the launcher do not; the API sees every
execution, worker heartbeat and launch, so its metrics cover them. See
[ADR 0014](../decisions/0014-prometheus-metrics.md) for that decision and for the choice
of Prometheus over an OTLP metrics exporter.

## Scraping

`/metrics` is on the API port (`8080`) by default and needs no authentication. The Helm
chart's ingress does not route `/metrics`, so in a default install it is reachable only
from inside the cluster.

| Setting | Helm value | Effect |
|---|---|---|
| `ORBIT_METRICS_ADDR` (e.g. `0.0.0.0:9090`) | `monitoring.port` | Serve `/metrics` (and `/healthz`) on a dedicated listener. The API port then answers `/metrics` with 404. |
| `ORBIT_METRICS_TOKEN` | `monitoring.token.existingSecret`, `monitoring.token.key` | Require `Authorization: Bearer <token>`. Without it the endpoint answers 401. |

On the API port, `/metrics` shares the per-IP budget for anonymous routes
(`ORBIT_ANON_RATE_LIMIT_PER_MINUTE`, default 120 per minute). One Prometheus scraping
every 15 seconds or slower stays far below it. The dedicated listener has no rate limit.

### Prometheus Operator (ServiceMonitor)

```yaml
monitoring:
  serviceMonitor:
    enabled: true
    labels:
      release: kube-prometheus-stack   # whatever label your Prometheus selects on
    interval: 30s
    scrapeTimeout: 10s
```

The ServiceMonitor selects the API Service and scrapes the `http` port, or the `metrics`
port when `monitoring.port` is set. It needs the `monitoring.coreos.com/v1` CRDs.

### Annotation-based discovery

```yaml
monitoring:
  annotations: true
```

This adds `prometheus.io/scrape`, `prometheus.io/path` and `prometheus.io/port` to the
API pods and Service. Annotation discovery cannot send a bearer token, so the chart
refuses `annotations` together with `monitoring.token`.

### Locked down: dedicated port, token and NetworkPolicy

```bash
kubectl -n orbit create secret generic orbit-metrics-token \
  --from-literal=token="$(openssl rand -hex 32)"
```

```yaml
monitoring:
  port: 9090
  token:
    existingSecret: orbit-metrics-token
  serviceMonitor:
    enabled: true
  allowFrom:
    - namespaceSelector:
        matchLabels:
          kubernetes.io/metadata.name: monitoring
networkPolicy:
  enabled: true
```

With a token, the ServiceMonitor must live in the release namespace, because Prometheus
reads the token Secret from the ServiceMonitor's namespace.

### Without Kubernetes

```yaml
scrape_configs:
  - job_name: orbit-api
    metrics_path: /metrics
    static_configs:
      - targets: ["orbit-api:8080"]
    # authorization:
    #   credentials_file: /etc/prometheus/orbit-metrics-token
```

## Reading the numbers

- **Counters and histograms** count what one API process handled. They reset when the
  process restarts and are per replica: use `rate()` and `sum()` across replicas.
- **Gauges marked "installation-wide"** are read from the database when the endpoint is
  scraped (cached for 10 seconds). Every replica reports the same value: use `max()`, not
  `sum()`. If the database cannot be read, these gauges are left out of that scrape and
  `orbit_metrics_collect_errors_total` goes up; the rest is still served.
- Execution counters are recorded by the replica that wrote the transition. A run
  created on one replica and finished on another is still counted exactly once for each.

## Metrics

### Process and HTTP

| Metric | Type | Labels | Meaning |
|---|---|---|---|
| `orbit_info` | gauge | `version`, `environment` | Always 1; carries the build and `ORBIT_ENV`. |
| `orbit_uptime_seconds` | gauge | | Seconds since the process started. |
| `orbit_http_requests_total` | counter | `method`, `route`, `status_class` | Requests handled. `route` is the route template (`/api/v1/agents/{agent_id}`); requests that match no route are `unmatched`. |
| `orbit_http_errors_total` | counter | | Requests answered with 4xx or 5xx. |
| `orbit_http_request_duration_seconds` | histogram | `method`, `route` | Time to answer. Event streams (SSE) are counted when the stream opens. |
| `orbit_http_in_flight_requests` | gauge | | Requests being handled right now (includes the scrape itself). |
| `orbit_rate_limited_total` | counter | | Anonymous requests rejected with 429. |

### Executions

| Metric | Type | Labels | Meaning |
|---|---|---|---|
| `orbit_executions_created_total` | counter | `harness`, `trigger_source` | Executions accepted. Idempotent replays are not counted. |
| `orbit_executions_started_total` | counter | `harness` | Harness reported `running` for the first time. |
| `orbit_executions_completed_total` | counter | `status`, `harness` | Reached a terminal state (`succeeded`, `failed`, `timed_out`, `cancelled`, `rejected`). |
| `orbit_execution_duration_seconds` | histogram | `harness`, `status` | `running` to terminal. Runs that never ran have no sample. |
| `orbit_execution_queue_wait_seconds` | histogram | | Accepted to `running`: queueing, worker start-up and checkout. |
| `orbit_execution_timeouts_total` | counter | | Ended as `timed_out`. |
| `orbit_executions_reaped_total` | counter | `reason` | Ended by the control plane: `worker_lost`, `harness_unresponsive`, `never_started`, `timed_out`. |
| `orbit_executions_in_flight` | gauge | `status` | Installation-wide: `queued`, `dispatched`, `running`, `awaiting_approval`. |

### Workers

| Metric | Type | Labels | Meaning |
|---|---|---|---|
| `orbit_workers_online` | gauge | | Installation-wide: workers with a heartbeat in the last 90 seconds. |
| `orbit_worker_heartbeats_total` | counter | | Heartbeats accepted. |
| `orbit_worker_claims_total` | counter | `result` | Claim requests: `claimed`, `empty` (nothing to do), `error`. |
| `orbit_worker_launches` | gauge | `status` | Installation-wide: launched workers that are `pending`, `starting` or `running`. |
| `orbit_worker_launch_requests_total` | counter | `result` | Launch requests sent to the launcher: `ok`, `error`. |
| `orbit_worker_launch_duration_seconds` | histogram | | Time the launcher took to accept a launch. |

### Harnesses, providers and agents

| Metric | Type | Labels | Meaning |
|---|---|---|---|
| `orbit_harness_available` | gauge | `kind` | 1 when the adapter's implementation status is `available`. |
| `orbit_provider_probes_total` | counter | `kind`, `result` | Model provider tests (**Test connection**): `ok`, `error`. |
| `orbit_agents_created_total` | counter | | Agents created. |

### Background loops, database and event bus

| Metric | Type | Labels | Meaning |
|---|---|---|---|
| `orbit_background_tick_duration_seconds` | histogram | `loop` | One pass of a control-plane loop: `license`, `outputs`, `scheduler`, `reaper`, `team_runs`, `dispatcher`, `worker_tags`, `runtime`, `memory_pruner`. |
| `orbit_background_tick_errors_total` | counter | `loop` | Passes that ended in an error. |
| `orbit_scheduler_triggers_fired_total` | counter | | Scheduled triggers fired. |
| `orbit_team_runs_advanced_total` | counter | | Team run advance passes. |
| `orbit_db_pool_connections` | gauge | `state` | This process's PostgreSQL pool: `idle`, `active`. |
| `orbit_events_published_total` | counter | `subject` | Events published to the bus, by subject (a fixed set). |
| `orbit_events_publish_errors_total` | counter | | Publishes the bus refused or failed. |

### Licensing

| Metric | Type | Labels | Meaning |
|---|---|---|---|
| `orbit_license_phase` | gauge | `phase` | 1 for the current phase, 0 for the others: `community`, `active`, `grace`, `expired`, `invalid`. |
| `orbit_license_usage` | gauge | `resource` | Installation-wide use: `seats`, `agents`, `workers`, `executions_per_day`. |
| `orbit_license_limit` | gauge | `resource` | The license limit. Absent when the resource is unlimited. |

### The metrics pipeline

| Metric | Type | Meaning |
|---|---|---|
| `orbit_metrics_collect_errors_total` | counter | Scrapes for which the database gauges could not be read. |
| `orbit_metrics_dropped_series_total` | counter | Samples dropped because a family reached its series cap (200). Should stay 0. |

## Cardinality and privacy

- No metric has an organization, user, agent, execution, worker or connection label, and
  no name, email, slug or license customer is exported. Per-execution and per-user
  metrics are out of scope by design; use the audit log and execution history for that.
- `route` is the route template, never the request path. Label values outside a
  conservative character set are replaced by `other`.
- Every label has a fixed, small set of values. As a backstop, a family stops accepting
  new label sets at 200 series and counts the drops.

## Example queries and alerts

```promql
# Error ratio of the API
sum(rate(orbit_http_requests_total{status_class="5xx"}[5m]))
  / sum(rate(orbit_http_requests_total[5m]))

# p95 latency per route
histogram_quantile(0.95,
  sum by (le, route) (rate(orbit_http_request_duration_seconds_bucket[5m])))

# Failure ratio of executions per harness
sum by (harness) (rate(orbit_executions_completed_total{status=~"failed|timed_out"}[30m]))
  / sum by (harness) (rate(orbit_executions_completed_total[30m]))

# Work is queued and nothing can take it
max(orbit_executions_in_flight{status="queued"}) > 0 and max(orbit_workers_online) == 0

# License: in grace or expired, or within 10% of a limit
max(orbit_license_phase{phase=~"grace|expired|invalid"}) == 1
max by (resource) (orbit_license_usage) / max by (resource) (orbit_license_limit) > 0.9

# The control plane is ending runs for workers
sum by (reason) (increase(orbit_executions_reaped_total[1h])) > 0
```

## Changes from earlier releases

- `orbit_http_requests_total`, `orbit_executions_created_total` and
  `orbit_provider_probes_total` now carry labels. Queries that read them without labels
  need `sum(...)`.
- `orbit_provider_probe_failures_total` is replaced by
  `orbit_provider_probes_total{result="error"}`.
- `orbit_info` is one series with both `version` and `environment`, instead of one series
  per label.
