//! Installation-wide counts read when Prometheus scrapes `/metrics`.
//!
//! These are current-state gauges that only the database knows (several API replicas
//! and the SQL reapers all change them), so they are counted here rather than tracked
//! in process memory. Every query is a grouped count over a small set of live rows.

use chrono::{Duration, Utc};

use crate::{Store, StoreResult};

/// Statuses in which an execution still occupies a queue slot or a worker.
pub const IN_FLIGHT_STATUSES: [&str; 4] = ["queued", "dispatched", "running", "awaiting_approval"];
/// Statuses in which a launched worker is still (coming) up.
pub const LIVE_LAUNCH_STATUSES: [&str; 3] = ["pending", "starting", "running"];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MetricsSnapshot {
    /// `(status, count)` for every status in [`IN_FLIGHT_STATUSES`], zero included.
    pub executions_in_flight: Vec<(String, i64)>,
    /// Workers whose heartbeat is newer than the staleness window.
    pub workers_online: i64,
    /// `(status, count)` for every status in [`LIVE_LAUNCH_STATUSES`], zero included.
    pub worker_launches: Vec<(String, i64)>,
    /// What the license limits count: `seats`, `agents`, `workers`, `executions_per_day`.
    pub license_usage: Vec<(&'static str, i64)>,
}

impl Store {
    /// Count current state across all organizations. `heartbeat_stale_seconds` is the
    /// window after which a silent worker no longer counts as online.
    pub async fn metrics_snapshot(
        &self,
        heartbeat_stale_seconds: i64,
    ) -> StoreResult<MetricsSnapshot> {
        let executions: Vec<(String, i64)> = sqlx::query_as(
            "SELECT status, COUNT(*) FROM executions WHERE status = ANY($1) GROUP BY status",
        )
        .bind(IN_FLIGHT_STATUSES.map(String::from).to_vec())
        .fetch_all(&self.pool)
        .await?;
        let launches: Vec<(String, i64)> = sqlx::query_as(
            r#"
            SELECT status, COUNT(*) FROM worker_launches
            WHERE archived_at IS NULL AND status = ANY($1)
            GROUP BY status
            "#,
        )
        .bind(LIVE_LAUNCH_STATUSES.map(String::from).to_vec())
        .fetch_all(&self.pool)
        .await?;
        let workers_online: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*) FROM worker_heartbeats
            WHERE last_heartbeat_at > now() - make_interval(secs => $1)
            "#,
        )
        .bind(heartbeat_stale_seconds as f64)
        .fetch_one(&self.pool)
        .await?;

        let since = Utc::now() - Duration::hours(24);
        let license_usage = vec![
            ("seats", self.count_active_users_installation().await?),
            ("agents", self.count_agents_installation().await?),
            (
                "workers",
                self.count_active_worker_launches_installation().await?,
            ),
            (
                "executions_per_day",
                self.count_all_executions_since_installation(since).await?,
            ),
        ];

        Ok(MetricsSnapshot {
            executions_in_flight: with_zeroes(&IN_FLIGHT_STATUSES, executions),
            workers_online,
            worker_launches: with_zeroes(&LIVE_LAUNCH_STATUSES, launches),
            license_usage,
        })
    }
}

/// Report every known status, so a series does not disappear when its count is zero.
fn with_zeroes(known: &[&str], counted: Vec<(String, i64)>) -> Vec<(String, i64)> {
    known
        .iter()
        .map(|status| {
            let count = counted
                .iter()
                .find(|(name, _)| name == status)
                .map(|(_, count)| *count)
                .unwrap_or(0);
            (status.to_string(), count)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::with_zeroes;

    #[test]
    fn missing_statuses_are_reported_as_zero() {
        let rows = with_zeroes(&["queued", "running"], vec![("running".to_string(), 3)]);
        assert_eq!(
            rows,
            vec![("queued".to_string(), 0), ("running".to_string(), 3)]
        );
    }
}
