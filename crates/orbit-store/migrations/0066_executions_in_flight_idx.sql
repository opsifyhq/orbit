-- /metrics counts in-flight executions across all organizations on every snapshot
-- (metrics_snapshot). executions_status_idx is keyed (org_id, status) and covers
-- every row ever written, so that count read the whole index and grew with history.
-- This partial index holds only in-flight rows; the query names the same literal
-- statuses so the planner can match the predicate (a bound array cannot).
-- Plain CREATE INDEX (see 0051): on a very large install, create it by hand with
-- CONCURRENTLY before upgrading.
CREATE INDEX IF NOT EXISTS executions_in_flight_status_idx
    ON executions (status)
    WHERE status IN ('queued', 'dispatched', 'running', 'awaiting_approval');
