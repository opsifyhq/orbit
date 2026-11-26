//! Prometheus metrics for the control plane, exposed at `GET /metrics`.
//!
//! The registry is dependency-free and renders the Prometheus text exposition format
//! (0.0.4). It holds two kinds of data:
//!
//! * **Counters and histograms** for things this process handled (HTTP requests, worker
//!   protocol calls, background ticks, execution transitions it wrote). They are kept in
//!   memory, start at zero on restart and are per replica: aggregate with `sum()`.
//! * **Gauges** for current state. They are computed when `/metrics` is scraped, from
//!   the database (cached for a few seconds), the license and the connection pool. Every
//!   replica reports the same database-backed value: aggregate with `max()`.
//!
//! Label values are low-cardinality by construction: routes are route templates, never
//! request paths, and there are no organization, user, agent or execution labels. A
//! per-family series cap guards against a bug turning a label into an unbounded one.
//! See `docs/development/observability.md` for the catalogue.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use orbit_contracts::{OrbitEvent, SubjectFilter};
use orbit_events::{EventBus, EventBusError, EventStream};
use orbit_store::metrics::MetricsSnapshot;
use orbit_store::observe::{ExecutionObserver, FinishCause};
use orbit_store::ExecutionRecord;

/// Upper bound on distinct label sets per metric family.
const MAX_SERIES_PER_FAMILY: usize = 200;
/// Cap for the HTTP families, whose label sets grow with the route table: every route
/// template times every method label (8, including `other`) times every status class
/// (5) must fit, so normal traffic (405s and CORS preflights included) never drops a
/// series. `routes_fit_the_http_series_cap` keeps this in step with `routes.rs`.
const MAX_HTTP_SERIES: usize = 8192;
/// How long a database snapshot is reused, so scraping cannot load the database.
const SNAPSHOT_TTL: Duration = Duration::from_secs(10);

const HTTP_BUCKETS: &[f64] = &[
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0,
];
const EXECUTION_BUCKETS: &[f64] = &[
    1.0, 5.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 1800.0, 3600.0, 7200.0, 14400.0,
];
const QUEUE_BUCKETS: &[f64] = &[
    0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0, 600.0, 1800.0, 3600.0,
];
const TICK_BUCKETS: &[f64] = &[0.005, 0.025, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 15.0, 60.0];
const LAUNCH_BUCKETS: &[f64] = &[0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0];

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Counter,
    Gauge,
    Histogram(&'static [f64]),
}

struct Family {
    name: &'static str,
    kind: Kind,
    help: &'static str,
}

const fn counter(name: &'static str, help: &'static str) -> Family {
    Family {
        name,
        kind: Kind::Counter,
        help,
    }
}

const fn gauge(name: &'static str, help: &'static str) -> Family {
    Family {
        name,
        kind: Kind::Gauge,
        help,
    }
}

const fn histogram(name: &'static str, buckets: &'static [f64], help: &'static str) -> Family {
    Family {
        name,
        kind: Kind::Histogram(buckets),
        help,
    }
}

/// Every metric family, in the order it is rendered. A sample for a name that is not
/// listed here is a programming error and is dropped (and caught by the unit tests).
const FAMILIES: &[Family] = &[
    gauge(
        "orbit_info",
        "Build and environment of this API process (always 1).",
    ),
    gauge(
        "orbit_uptime_seconds",
        "Seconds since this API process started.",
    ),
    // HTTP
    counter(
        "orbit_http_requests_total",
        "HTTP requests handled, by method, route template and status class.",
    ),
    counter(
        "orbit_http_errors_total",
        "HTTP requests answered with a 4xx or 5xx status.",
    ),
    histogram(
        "orbit_http_request_duration_seconds",
        HTTP_BUCKETS,
        "Time to answer an HTTP request, by method and route template.",
    ),
    gauge(
        "orbit_http_in_flight_requests",
        "HTTP requests currently being handled by this process.",
    ),
    counter(
        "orbit_rate_limited_total",
        "Anonymous requests rejected by the per-IP rate limiter.",
    ),
    // Executions
    counter(
        "orbit_executions_created_total",
        "Executions accepted, by harness and trigger source.",
    ),
    counter(
        "orbit_executions_started_total",
        "Executions whose harness reported running, by harness.",
    ),
    counter(
        "orbit_executions_completed_total",
        "Executions that reached a terminal state, by status and harness.",
    ),
    histogram(
        "orbit_execution_duration_seconds",
        EXECUTION_BUCKETS,
        "Time from running to a terminal state, by harness and status.",
    ),
    histogram(
        "orbit_execution_queue_wait_seconds",
        QUEUE_BUCKETS,
        "Time from accepted to running (queueing, worker start-up and checkout).",
    ),
    counter(
        "orbit_execution_timeouts_total",
        "Executions that ended as timed_out.",
    ),
    counter(
        "orbit_executions_reaped_total",
        "Executions ended by the control plane instead of their worker, by reason.",
    ),
    gauge(
        "orbit_executions_in_flight",
        "Executions not yet finished, by status (installation-wide).",
    ),
    // Workers
    gauge(
        "orbit_workers_online",
        "Workers with a recent heartbeat (installation-wide).",
    ),
    counter(
        "orbit_worker_heartbeats_total",
        "Worker heartbeats accepted by this process.",
    ),
    counter(
        "orbit_worker_claims_total",
        "Worker claim requests, by result (claimed, empty, error).",
    ),
    gauge(
        "orbit_worker_launches",
        "Launched workers that are pending, starting or running, by status.",
    ),
    counter(
        "orbit_worker_launch_requests_total",
        "Worker launch requests sent to the launcher, by result.",
    ),
    histogram(
        "orbit_worker_launch_duration_seconds",
        LAUNCH_BUCKETS,
        "Time the launcher took to accept a worker launch request.",
    ),
    // Harnesses and providers
    gauge(
        "orbit_harness_available",
        "1 when the harness adapter is certified as available, 0 otherwise.",
    ),
    counter(
        "orbit_provider_probes_total",
        "Model provider probes, by provider kind and result.",
    ),
    counter("orbit_agents_created_total", "Agents created."),
    // Background loops
    histogram(
        "orbit_background_tick_duration_seconds",
        TICK_BUCKETS,
        "Duration of one pass of a control-plane background loop.",
    ),
    counter(
        "orbit_background_tick_errors_total",
        "Background loop passes that ended in an error.",
    ),
    counter(
        "orbit_scheduler_triggers_fired_total",
        "Scheduled triggers fired by this process.",
    ),
    counter(
        "orbit_team_runs_advanced_total",
        "Team run advance passes made by this process.",
    ),
    // Store
    gauge(
        "orbit_db_pool_connections",
        "PostgreSQL pool connections of this process, by state (idle, active).",
    ),
    // Event bus
    counter(
        "orbit_events_published_total",
        "Events published to the event bus, by subject.",
    ),
    counter(
        "orbit_events_publish_errors_total",
        "Events the event bus refused or failed to publish.",
    ),
    // Licensing
    gauge(
        "orbit_license_phase",
        "1 for the current license phase, 0 for the others.",
    ),
    gauge(
        "orbit_license_usage",
        "Current use of a licensed resource (installation-wide).",
    ),
    gauge(
        "orbit_license_limit",
        "License limit for a resource; absent when the resource is unlimited.",
    ),
    // The metrics pipeline itself
    counter(
        "orbit_metrics_collect_errors_total",
        "Scrapes for which the database gauges could not be read.",
    ),
    counter(
        "orbit_metrics_dropped_series_total",
        "Samples dropped because a metric family reached its series cap.",
    ),
];

pub const LICENSE_PHASES: [&str; 5] = ["community", "active", "grace", "expired", "invalid"];

type Labels = Vec<(&'static str, String)>;

#[derive(Debug, Clone)]
struct Histogram {
    /// Non-cumulative count per bucket; the last slot is the overflow (`+Inf`) bucket.
    counts: Vec<u64>,
    sum: f64,
}

#[derive(Debug, Default)]
struct Registry {
    counters: BTreeMap<&'static str, BTreeMap<Labels, u64>>,
    histograms: BTreeMap<&'static str, BTreeMap<Labels, Histogram>>,
}

/// A current-state value supplied at render time.
#[derive(Debug, Clone, PartialEq)]
pub struct GaugeSample {
    pub name: &'static str,
    pub labels: Vec<(&'static str, String)>,
    pub value: f64,
}

impl GaugeSample {
    pub fn new(name: &'static str, labels: &[(&'static str, &str)], value: f64) -> Self {
        Self {
            name,
            labels: labels
                .iter()
                .map(|(key, value)| (*key, value.to_string()))
                .collect(),
            value,
        }
    }
}

#[derive(Debug)]
pub struct Metrics {
    started: Instant,
    registry: Mutex<Registry>,
    in_flight: AtomicI64,
    dropped: AtomicU64,
    collect_errors: AtomicU64,
    rate_limited: AtomicU64,
    snapshot: tokio::sync::Mutex<Option<(Instant, Arc<MetricsSnapshot>)>>,
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            registry: Mutex::new(Registry::default()),
            in_flight: AtomicI64::new(0),
            dropped: AtomicU64::new(0),
            collect_errors: AtomicU64::new(0),
            rate_limited: AtomicU64::new(0),
            snapshot: tokio::sync::Mutex::new(None),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Registry> {
        self.registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn inc(&self, name: &'static str, labels: &[(&'static str, &str)]) {
        debug_assert!(matches!(family(name).map(|f| f.kind), Some(Kind::Counter)));
        let labels = owned(labels);
        let mut registry = self.lock();
        let series = registry.counters.entry(name).or_default();
        if !series.contains_key(&labels) && series.len() >= series_cap(name) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        *series.entry(labels).or_insert(0) += 1;
    }

    fn observe(&self, name: &'static str, labels: &[(&'static str, &str)], seconds: f64) {
        let Some(Kind::Histogram(buckets)) = family(name).map(|f| f.kind) else {
            debug_assert!(false, "{name} is not a histogram");
            return;
        };
        let seconds = if seconds.is_finite() && seconds > 0.0 {
            seconds
        } else {
            0.0
        };
        let labels = owned(labels);
        let mut registry = self.lock();
        let series = registry.histograms.entry(name).or_default();
        if !series.contains_key(&labels) && series.len() >= series_cap(name) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let histogram = series.entry(labels).or_insert_with(|| Histogram {
            counts: vec![0; buckets.len() + 1],
            sum: 0.0,
        });
        let slot = buckets
            .iter()
            .position(|bound| seconds <= *bound)
            .unwrap_or(buckets.len());
        histogram.counts[slot] += 1;
        histogram.sum += seconds;
    }

    // ---------------------------------------------------------------- HTTP

    /// Call when a request starts; pair with [`Self::http_finished`].
    pub fn http_started(&self) {
        self.in_flight.fetch_add(1, Ordering::Relaxed);
    }

    /// `route` must be the route template (`/api/v1/agents/{agent_id}`), never the path.
    pub fn http_finished(&self, method: &str, route: &str, status: u16, elapsed: Duration) {
        self.in_flight.fetch_sub(1, Ordering::Relaxed);
        let method = http_method(method);
        let route = label_value(route);
        self.inc(
            "orbit_http_requests_total",
            &[
                ("method", method),
                ("route", &route),
                ("status_class", status_class(status)),
            ],
        );
        if status >= 400 {
            self.inc("orbit_http_errors_total", &[]);
        }
        self.observe(
            "orbit_http_request_duration_seconds",
            &[("method", method), ("route", &route)],
            elapsed.as_secs_f64(),
        );
    }

    /// A request ended without a response (the client went away or the server is
    /// shutting down): release its in-flight slot without recording a status.
    pub fn http_abandoned(&self) {
        self.in_flight.fetch_sub(1, Ordering::Relaxed);
    }

    /// The anonymous rate limiter keeps its own count; copy it in before rendering.
    pub fn set_rate_limited(&self, total: u64) {
        self.rate_limited.store(total, Ordering::Relaxed);
    }

    // ------------------------------------------------------------- workers

    pub fn worker_heartbeat(&self) {
        self.inc("orbit_worker_heartbeats_total", &[]);
    }

    /// `result` is one of `claimed`, `empty`, `error`.
    pub fn worker_claim(&self, result: &'static str) {
        self.inc("orbit_worker_claims_total", &[("result", result)]);
    }

    pub fn worker_launch_request(&self, ok: bool, elapsed: Duration) {
        self.inc(
            "orbit_worker_launch_requests_total",
            &[("result", result_label(ok))],
        );
        self.observe(
            "orbit_worker_launch_duration_seconds",
            &[],
            elapsed.as_secs_f64(),
        );
    }

    // ------------------------------------------------- catalog and agents

    pub fn inc_agents(&self) {
        self.inc("orbit_agents_created_total", &[]);
    }

    pub fn provider_probe(&self, kind: &str, ok: bool) {
        self.inc(
            "orbit_provider_probes_total",
            &[("kind", &label_value(kind)), ("result", result_label(ok))],
        );
    }

    // ------------------------------------------------------ background loops

    pub fn background_tick(&self, name: &'static str, elapsed: Duration, ok: bool) {
        self.observe(
            "orbit_background_tick_duration_seconds",
            &[("loop", name)],
            elapsed.as_secs_f64(),
        );
        if !ok {
            self.inc("orbit_background_tick_errors_total", &[("loop", name)]);
        }
    }

    pub fn trigger_fired(&self) {
        self.inc("orbit_scheduler_triggers_fired_total", &[]);
    }

    pub fn team_run_advanced(&self) {
        self.inc("orbit_team_runs_advanced_total", &[]);
    }

    // ------------------------------------------------------------ event bus

    pub fn event_published(&self, subject: &str, ok: bool) {
        if ok {
            self.inc(
                "orbit_events_published_total",
                &[("subject", &label_value(subject))],
            );
        } else {
            self.inc("orbit_events_publish_errors_total", &[]);
        }
    }

    // ------------------------------------------------------------- snapshot

    /// Database-backed gauges, reused for [`SNAPSHOT_TTL`]. Returns `None` (and counts a
    /// collect error) when the database cannot be read, so a scrape still succeeds with
    /// the in-memory metrics.
    pub async fn snapshot(
        &self,
        store: &orbit_store::Store,
        heartbeat_stale_seconds: i64,
    ) -> Option<Arc<MetricsSnapshot>> {
        // Held across the query on purpose: concurrent scrapes share one read.
        let mut cached = self.snapshot.lock().await;
        if let Some((at, snapshot)) = cached.as_ref() {
            if at.elapsed() < SNAPSHOT_TTL {
                return Some(snapshot.clone());
            }
        }
        match store.metrics_snapshot(heartbeat_stale_seconds).await {
            Ok(snapshot) => {
                let snapshot = Arc::new(snapshot);
                *cached = Some((Instant::now(), snapshot.clone()));
                Some(snapshot)
            }
            Err(error) => {
                tracing::warn!(%error, "metrics: reading database gauges failed");
                self.collect_errors.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }

    // --------------------------------------------------------------- render

    /// Render the exposition text. `gauges` carries the current-state values computed by
    /// the caller; process-level gauges are added here.
    pub fn render(&self, gauges: &[GaugeSample]) -> String {
        let (counters, histograms) = {
            let registry = self.lock();
            (registry.counters.clone(), registry.histograms.clone())
        };
        let mut out = String::with_capacity(8 * 1024);
        for family in FAMILIES {
            out.push_str(&format!("# HELP {} {}\n", family.name, family.help));
            let kind = match family.kind {
                Kind::Counter => "counter",
                Kind::Gauge => "gauge",
                Kind::Histogram(_) => "histogram",
            };
            out.push_str(&format!("# TYPE {} {kind}\n", family.name));
            match family.kind {
                Kind::Counter => {
                    let own = match family.name {
                        "orbit_metrics_collect_errors_total" => {
                            Some(self.collect_errors.load(Ordering::Relaxed))
                        }
                        "orbit_rate_limited_total" => {
                            Some(self.rate_limited.load(Ordering::Relaxed))
                        }
                        "orbit_metrics_dropped_series_total" => {
                            Some(self.dropped.load(Ordering::Relaxed))
                        }
                        _ => None,
                    };
                    if let Some(value) = own {
                        out.push_str(&format!("{} {value}\n", family.name));
                        continue;
                    }
                    match counters.get(family.name) {
                        Some(series) => {
                            for (labels, value) in series {
                                out.push_str(&format!(
                                    "{}{} {value}\n",
                                    family.name,
                                    render_labels(labels, None)
                                ));
                            }
                        }
                        // An unlabelled counter is reported from the start, so `rate()`
                        // sees its first increment.
                        None if !labelled(family.name) => {
                            out.push_str(&format!("{} 0\n", family.name));
                        }
                        None => {}
                    }
                }
                Kind::Gauge => {
                    match family.name {
                        "orbit_uptime_seconds" => out.push_str(&format!(
                            "orbit_uptime_seconds {}\n",
                            self.started.elapsed().as_secs()
                        )),
                        "orbit_http_in_flight_requests" => out.push_str(&format!(
                            "orbit_http_in_flight_requests {}\n",
                            self.in_flight.load(Ordering::Relaxed).max(0)
                        )),
                        _ => {}
                    }
                    for sample in gauges.iter().filter(|sample| sample.name == family.name) {
                        out.push_str(&format!(
                            "{}{} {}\n",
                            family.name,
                            render_labels(&sample.labels, None),
                            number(sample.value)
                        ));
                    }
                }
                Kind::Histogram(buckets) => {
                    let Some(series) = histograms.get(family.name) else {
                        continue;
                    };
                    for (labels, histogram) in series {
                        let mut cumulative = 0;
                        for (index, bound) in buckets.iter().enumerate() {
                            cumulative += histogram.counts[index];
                            out.push_str(&format!(
                                "{}_bucket{} {cumulative}\n",
                                family.name,
                                render_labels(labels, Some(&number(*bound)))
                            ));
                        }
                        cumulative += histogram.counts[buckets.len()];
                        out.push_str(&format!(
                            "{}_bucket{} {cumulative}\n",
                            family.name,
                            render_labels(labels, Some("+Inf"))
                        ));
                        out.push_str(&format!(
                            "{}_sum{} {}\n",
                            family.name,
                            render_labels(labels, None),
                            number(histogram.sum)
                        ));
                        out.push_str(&format!(
                            "{}_count{} {cumulative}\n",
                            family.name,
                            render_labels(labels, None)
                        ));
                    }
                }
            }
        }
        out
    }
}

impl ExecutionObserver for Metrics {
    fn created(&self, execution: &ExecutionRecord) {
        self.inc(
            "orbit_executions_created_total",
            &[
                ("harness", &label_value(&execution.harness_kind)),
                ("trigger_source", &label_value(&execution.trigger_source)),
            ],
        );
    }

    fn started(&self, execution: &ExecutionRecord) {
        let harness = label_value(&execution.harness_kind);
        self.inc("orbit_executions_started_total", &[("harness", &harness)]);
        if let Some(running) = execution.running_started_at {
            self.observe(
                "orbit_execution_queue_wait_seconds",
                &[],
                seconds_between(execution.created_at, running),
            );
        }
    }

    fn finished(&self, execution: &ExecutionRecord, cause: FinishCause) {
        let harness = label_value(&execution.harness_kind);
        let status = label_value(&execution.status);
        self.inc(
            "orbit_executions_completed_total",
            &[("status", &status), ("harness", &harness)],
        );
        if execution.status == "timed_out" {
            self.inc("orbit_execution_timeouts_total", &[]);
        }
        if let Some(reason) = cause.reaped_reason() {
            self.inc("orbit_executions_reaped_total", &[("reason", reason)]);
        }
        // Only runs that actually ran have a duration; a run cancelled in the queue or
        // lost before it started would otherwise drag the histogram towards zero.
        if let (Some(running), Some(finished)) =
            (execution.running_started_at, execution.finished_at)
        {
            self.observe(
                "orbit_execution_duration_seconds",
                &[("harness", &harness), ("status", &status)],
                seconds_between(running, finished),
            );
        }
    }
}

/// Counts publishes on the way to the real event bus.
pub struct MeteredBus {
    inner: Arc<dyn EventBus>,
    metrics: Arc<Metrics>,
}

impl MeteredBus {
    pub fn wrap(inner: Arc<dyn EventBus>, metrics: Arc<Metrics>) -> Arc<dyn EventBus> {
        Arc::new(Self { inner, metrics })
    }
}

#[async_trait]
impl EventBus for MeteredBus {
    fn driver(&self) -> &'static str {
        self.inner.driver()
    }

    async fn publish(&self, event: OrbitEvent) -> Result<(), EventBusError> {
        let subject = event.subject.clone();
        let result = self.inner.publish(event).await;
        self.metrics.event_published(&subject, result.is_ok());
        result
    }

    async fn subscribe(&self, filter: SubjectFilter) -> Result<EventStream, EventBusError> {
        self.inner.subscribe(filter).await
    }

    async fn health(&self) -> Result<(), EventBusError> {
        self.inner.health().await
    }
}

/// Series cap for one family: the HTTP families are keyed by route template.
fn series_cap(name: &str) -> usize {
    match name {
        "orbit_http_requests_total" | "orbit_http_request_duration_seconds" => MAX_HTTP_SERIES,
        _ => MAX_SERIES_PER_FAMILY,
    }
}

fn family(name: &str) -> Option<&'static Family> {
    FAMILIES.iter().find(|family| family.name == name)
}

/// Counter families that only ever appear with labels (no zero line before first use).
fn labelled(name: &str) -> bool {
    matches!(
        name,
        "orbit_http_requests_total"
            | "orbit_executions_created_total"
            | "orbit_executions_started_total"
            | "orbit_executions_completed_total"
            | "orbit_executions_reaped_total"
            | "orbit_worker_claims_total"
            | "orbit_worker_launch_requests_total"
            | "orbit_provider_probes_total"
            | "orbit_background_tick_errors_total"
            | "orbit_events_published_total"
    )
}

fn owned(labels: &[(&'static str, &str)]) -> Labels {
    labels
        .iter()
        .map(|(key, value)| (*key, value.to_string()))
        .collect()
}

fn seconds_between(from: chrono::DateTime<chrono::Utc>, to: chrono::DateTime<chrono::Utc>) -> f64 {
    (to - from).num_milliseconds().max(0) as f64 / 1000.0
}

fn result_label(ok: bool) -> &'static str {
    if ok {
        "ok"
    } else {
        "error"
    }
}

fn status_class(status: u16) -> &'static str {
    match status / 100 {
        1 => "1xx",
        2 => "2xx",
        3 => "3xx",
        4 => "4xx",
        _ => "5xx",
    }
}

/// Request methods are caller-controlled; anything unusual is folded into `other`.
fn http_method(method: &str) -> &'static str {
    match method {
        "GET" => "GET",
        "POST" => "POST",
        "PUT" => "PUT",
        "PATCH" => "PATCH",
        "DELETE" => "DELETE",
        "OPTIONS" => "OPTIONS",
        "HEAD" => "HEAD",
        _ => "other",
    }
}

/// Keep a value usable as a label: short and from a conservative character set.
/// Anything else becomes `other`, so a surprising value cannot create series or break
/// the exposition format.
fn label_value(value: &str) -> String {
    let ok = !value.is_empty()
        && value.len() <= 96
        && value.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '{' | '}' | ':' | '*')
        });
    if ok {
        value.to_string()
    } else {
        "other".to_string()
    }
}

fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

fn render_labels(labels: &[(&'static str, String)], le: Option<&str>) -> String {
    if labels.is_empty() && le.is_none() {
        return String::new();
    }
    let mut parts: Vec<String> = labels
        .iter()
        .map(|(key, value)| format!("{key}=\"{}\"", escape(value)))
        .collect();
    if let Some(le) = le {
        parts.push(format!("le=\"{le}\""));
    }
    format!("{{{}}}", parts.join(","))
}

fn number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
        text.lines().find(|line| line.starts_with(prefix))
    }

    #[test]
    fn every_family_has_help_and_type_once() {
        let text = Metrics::new().render(&[]);
        for family in FAMILIES {
            let help = format!("# HELP {} ", family.name);
            let kind = format!("# TYPE {} ", family.name);
            assert_eq!(text.matches(&help).count(), 1, "{}", family.name);
            assert_eq!(text.matches(&kind).count(), 1, "{}", family.name);
            assert!(family.name.starts_with("orbit_"));
        }
        let mut names: Vec<&str> = FAMILIES.iter().map(|family| family.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), FAMILIES.len(), "duplicate family");
    }

    #[test]
    fn exposition_lines_are_well_formed() {
        let metrics = Metrics::new();
        metrics.http_started();
        metrics.http_finished(
            "GET",
            "/api/v1/agents/{agent_id}",
            200,
            Duration::from_millis(7),
        );
        metrics.worker_claim("claimed");
        metrics.background_tick("scheduler", Duration::from_millis(3), false);
        let text = metrics.render(&[GaugeSample::new(
            "orbit_license_phase",
            &[("phase", "active")],
            1.0,
        )]);
        for line in text.lines().filter(|line| !line.starts_with('#')) {
            let (series, value) = line.rsplit_once(' ').expect("name and value");
            assert!(value.parse::<f64>().is_ok(), "bad value in {line}");
            let name = series.split('{').next().unwrap();
            assert!(
                name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
                "bad name in {line}"
            );
            if let Some(rest) = series.strip_prefix(name) {
                assert!(rest.is_empty() || (rest.starts_with('{') && rest.ends_with('}')));
            }
        }
    }

    #[test]
    fn http_requests_are_labelled_and_timed() {
        let metrics = Metrics::new();
        for (status, millis) in [(200, 4), (200, 40), (404, 4), (503, 700)] {
            metrics.http_started();
            metrics.http_finished(
                "GET",
                "/api/v1/agents",
                status,
                Duration::from_millis(millis),
            );
        }
        let text = metrics.render(&[]);
        assert!(text.contains(
            "orbit_http_requests_total{method=\"GET\",route=\"/api/v1/agents\",status_class=\"2xx\"} 2\n"
        ));
        assert!(text.contains("status_class=\"4xx\"} 1\n"));
        assert!(text.contains("status_class=\"5xx\"} 1\n"));
        assert!(text.contains("orbit_http_errors_total 2\n"));
        assert!(text.contains("orbit_http_in_flight_requests 0\n"));
        // Buckets are cumulative and end in +Inf == count.
        let bucket = |le: &str| {
            let prefix = format!(
                "orbit_http_request_duration_seconds_bucket{{method=\"GET\",route=\"/api/v1/agents\",le=\"{le}\"}} "
            );
            line(&text, &prefix)
                .unwrap_or_else(|| panic!("bucket {le}"))
                .rsplit_once(' ')
                .unwrap()
                .1
                .parse::<u64>()
                .unwrap()
        };
        assert_eq!(bucket("0.005"), 2);
        assert_eq!(bucket("0.05"), 3);
        assert_eq!(bucket("1"), 4);
        assert_eq!(bucket("+Inf"), 4);
        assert!(text.contains(
            "orbit_http_request_duration_seconds_count{method=\"GET\",route=\"/api/v1/agents\"} 4\n"
        ));
    }

    #[test]
    fn in_flight_counts_open_requests() {
        let metrics = Metrics::new();
        metrics.http_started();
        metrics.http_started();
        assert!(metrics
            .render(&[])
            .contains("orbit_http_in_flight_requests 2\n"));
        metrics.http_finished("GET", "/healthz", 200, Duration::ZERO);
        assert!(metrics
            .render(&[])
            .contains("orbit_http_in_flight_requests 1\n"));
    }

    #[test]
    fn unusual_label_values_cannot_create_series_or_break_the_format() {
        let metrics = Metrics::new();
        metrics.http_started();
        metrics.http_finished("BREW\"\n", "/x\"} 1\nevil 1", 200, Duration::ZERO);
        metrics.provider_probe(&"k".repeat(500), true);
        let text = metrics.render(&[GaugeSample::new(
            "orbit_info",
            &[("version", "1.0\"\\\n")],
            1.0,
        )]);
        assert!(text.contains(
            "orbit_http_requests_total{method=\"other\",route=\"other\",status_class=\"2xx\"} 1\n"
        ));
        assert!(text.contains("orbit_provider_probes_total{kind=\"other\",result=\"ok\"} 1\n"));
        assert!(text.contains("orbit_info{version=\"1.0\\\"\\\\\\n\"} 1\n"));
        assert!(!text.contains("\nevil 1"));
    }

    #[test]
    fn a_family_stops_growing_at_the_series_cap() {
        let metrics = Metrics::new();
        for index in 0..(MAX_SERIES_PER_FAMILY + 25) {
            metrics.provider_probe(&format!("kind-{index}"), true);
        }
        let text = metrics.render(&[]);
        let series = text
            .lines()
            .filter(|line| line.starts_with("orbit_provider_probes_total{"))
            .count();
        assert_eq!(series, MAX_SERIES_PER_FAMILY);
        assert!(text.contains("orbit_metrics_dropped_series_total 25\n"));
        // An existing series keeps counting once the cap is reached.
        metrics.provider_probe("kind-0", true);
        assert!(metrics
            .render(&[])
            .contains("orbit_provider_probes_total{kind=\"kind-0\",result=\"ok\"} 2\n"));
    }

    /// Route templates registered in `routes.rs` (the first string after each `.route(`).
    fn route_templates() -> Vec<String> {
        let source = include_str!("routes.rs");
        let mut templates: Vec<String> = source
            .split(".route(")
            .skip(1)
            .filter_map(|rest| {
                let start = rest.find('"')? + 1;
                let end = start + rest[start..].find('"')?;
                Some(rest[start..end].to_string())
            })
            // routes.rs also mentions `.route(` in code; templates start with "/".
            .filter(|template| template.starts_with('/'))
            .collect();
        // Swagger UI and the 404 fallback add a few labels of their own.
        templates
            .extend(["/docs", "/docs/", "/docs/{*rest}", "/api/v1/openapi.json"].map(String::from));
        templates.push("unmatched".to_string());
        templates.sort();
        templates.dedup();
        templates
    }

    #[test]
    fn routes_fit_the_http_series_cap() {
        let templates = route_templates();
        assert!(templates.len() > 100, "parsed {} routes", templates.len());
        let methods = [
            "GET", "POST", "PUT", "PATCH", "DELETE", "OPTIONS", "HEAD", "TRACE",
        ];
        let statuses = [101, 200, 304, 404, 503];
        assert!(
            templates.len() * methods.len() * statuses.len() <= MAX_HTTP_SERIES,
            "{} routes need a larger MAX_HTTP_SERIES",
            templates.len()
        );
        // Every route with every method and status class: nothing is dropped.
        let metrics = Metrics::new();
        for template in &templates {
            for method in methods {
                for status in statuses {
                    metrics.http_started();
                    metrics.http_finished(method, template, status, Duration::from_millis(3));
                }
            }
        }
        let text = metrics.render(&[]);
        assert!(text.contains("orbit_metrics_dropped_series_total 0\n"));
        let series = text
            .lines()
            .filter(|line| line.starts_with("orbit_http_requests_total{"))
            .count();
        assert_eq!(series, templates.len() * methods.len() * statuses.len());
    }

    fn execution(status: &str) -> ExecutionRecord {
        let now = chrono::Utc::now();
        ExecutionRecord {
            id: uuid::Uuid::now_v7(),
            org_id: uuid::Uuid::now_v7(),
            agent_id: uuid::Uuid::now_v7(),
            agent_version_id: uuid::Uuid::now_v7(),
            agent_name: "agent".into(),
            harness_kind: "mock".into(),
            trigger_id: None,
            trigger_source: "manual".into(),
            placement: "any".into(),
            status: status.into(),
            session_ref: None,
            input: serde_json::json!({}),
            skills: Vec::new(),
            output: None,
            error: None,
            usage: serde_json::json!({}),
            timeout_seconds: 60,
            idempotency_key: None,
            requested_by: None,
            worker_id: None,
            team_run_id: None,
            team_step_index: None,
            created_at: now - chrono::Duration::seconds(50),
            started_at: None,
            running_started_at: None,
            last_execution_heartbeat_at: None,
            execution_lease_until: None,
            worker_boot_id: None,
            worker_boot_started_at: None,
            finished_at: None,
            updated_at: now,
            archived_at: None,
        }
    }

    #[test]
    fn execution_lifecycle_is_counted_and_timed() {
        let metrics = Metrics::new();
        let mut record = execution("queued");
        metrics.created(&record);
        record.status = "running".into();
        record.running_started_at = Some(record.created_at + chrono::Duration::seconds(8));
        metrics.started(&record);
        record.status = "succeeded".into();
        record.finished_at =
            Some(record.running_started_at.unwrap() + chrono::Duration::seconds(40));
        metrics.finished(&record, FinishCause::Reported);

        let text = metrics.render(&[]);
        assert!(text.contains(
            "orbit_executions_created_total{harness=\"mock\",trigger_source=\"manual\"} 1\n"
        ));
        assert!(text.contains("orbit_executions_started_total{harness=\"mock\"} 1\n"));
        assert!(text.contains(
            "orbit_executions_completed_total{status=\"succeeded\",harness=\"mock\"} 1\n"
        ));
        assert!(text.contains("orbit_execution_queue_wait_seconds_sum 8\n"));
        assert!(text.contains(
            "orbit_execution_duration_seconds_sum{harness=\"mock\",status=\"succeeded\"} 40\n"
        ));
        assert!(text.contains("orbit_execution_timeouts_total 0\n"));
        assert!(!text.contains("orbit_executions_reaped_total{"));
    }

    #[test]
    fn reaped_and_unstarted_executions_are_counted_without_a_duration() {
        let metrics = Metrics::new();
        let mut lost = execution("failed");
        lost.finished_at = Some(chrono::Utc::now());
        metrics.finished(&lost, FinishCause::NeverStarted);
        let mut timed_out = execution("timed_out");
        timed_out.running_started_at = Some(timed_out.created_at);
        timed_out.finished_at = Some(timed_out.created_at + chrono::Duration::seconds(90));
        metrics.finished(&timed_out, FinishCause::TimedOut);
        let cancelled = execution("cancelled");
        metrics.finished(&cancelled, FinishCause::CancelledWhileQueued);

        let text = metrics.render(&[]);
        assert!(text.contains("orbit_executions_reaped_total{reason=\"never_started\"} 1\n"));
        assert!(text.contains("orbit_executions_reaped_total{reason=\"timed_out\"} 1\n"));
        assert!(text.contains("orbit_execution_timeouts_total 1\n"));
        assert!(text.contains(
            "orbit_executions_completed_total{status=\"cancelled\",harness=\"mock\"} 1\n"
        ));
        // Only the run that actually ran has a duration sample.
        assert!(text.contains(
            "orbit_execution_duration_seconds_count{harness=\"mock\",status=\"timed_out\"} 1\n"
        ));
        assert!(!text.contains(
            "orbit_execution_duration_seconds_count{harness=\"mock\",status=\"failed\"}"
        ));
    }

    #[test]
    fn background_ticks_events_and_workers_are_counted() {
        let metrics = Metrics::new();
        metrics.background_tick("reaper", Duration::from_millis(20), true);
        metrics.background_tick("reaper", Duration::from_millis(20), false);
        metrics.event_published("orbit.execution.updated", true);
        metrics.event_published("orbit.execution.updated", false);
        metrics.worker_heartbeat();
        metrics.worker_claim("empty");
        metrics.worker_launch_request(false, Duration::from_millis(300));
        metrics.set_rate_limited(1);
        metrics.trigger_fired();
        metrics.team_run_advanced();
        metrics.inc_agents();
        let text = metrics.render(&[]);
        for expected in [
            "orbit_background_tick_duration_seconds_count{loop=\"reaper\"} 2\n",
            "orbit_background_tick_errors_total{loop=\"reaper\"} 1\n",
            "orbit_events_published_total{subject=\"orbit.execution.updated\"} 1\n",
            "orbit_events_publish_errors_total 1\n",
            "orbit_worker_heartbeats_total 1\n",
            "orbit_worker_claims_total{result=\"empty\"} 1\n",
            "orbit_worker_launch_requests_total{result=\"error\"} 1\n",
            "orbit_worker_launch_duration_seconds_count 1\n",
            "orbit_rate_limited_total 1\n",
            "orbit_scheduler_triggers_fired_total 1\n",
            "orbit_team_runs_advanced_total 1\n",
            "orbit_agents_created_total 1\n",
        ] {
            assert!(text.contains(expected), "missing {expected}");
        }
    }

    #[test]
    fn gauges_are_rendered_under_their_family() {
        let text = Metrics::new().render(&[
            GaugeSample::new("orbit_executions_in_flight", &[("status", "queued")], 3.0),
            GaugeSample::new("orbit_license_limit", &[("resource", "agents")], 25.0),
        ]);
        let type_at = text
            .find("# TYPE orbit_executions_in_flight gauge")
            .unwrap();
        let sample_at = text
            .find("orbit_executions_in_flight{status=\"queued\"} 3\n")
            .unwrap();
        assert!(sample_at > type_at);
        assert!(text.contains("orbit_license_limit{resource=\"agents\"} 25\n"));
    }
}
