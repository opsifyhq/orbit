-- Indexes for periodic and per-request queries on tables that grow with every run.
-- Plain CREATE INDEX: sqlx runs each migration in a transaction and no earlier
-- migration uses CONCURRENTLY / `-- no-transaction`. These tables are locked against
-- writes while an index builds; on a very large install, create them by hand with
-- CONCURRENTLY before upgrading (IF NOT EXISTS makes this migration a no-op then).

-- Orphan reaper (every 15s per replica): status includes awaiting_approval, which the
-- older executions_pending_idx predicate does not cover.
CREATE INDEX IF NOT EXISTS executions_active_worker_idx
    ON executions (worker_id)
    WHERE status IN ('dispatched', 'running', 'awaiting_approval');

-- count_active_executions_for_agent runs on every run and trigger.
CREATE INDEX IF NOT EXISTS executions_agent_active_idx
    ON executions (agent_id)
    WHERE status IN ('queued', 'dispatched', 'running', 'awaiting_approval');

-- Foreign keys without a usable index (deleting a trigger or agent version scans
-- executions; the timeout reaper sums approvals per running execution).
CREATE INDEX IF NOT EXISTS executions_trigger_idx
    ON executions (trigger_id) WHERE trigger_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS executions_agent_version_idx
    ON executions (agent_version_id);
CREATE INDEX IF NOT EXISTS approvals_execution_idx
    ON approvals (execution_id);
CREATE INDEX IF NOT EXISTS output_deliveries_execution_idx
    ON output_deliveries (execution_id) WHERE execution_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS output_deliveries_team_run_idx
    ON output_deliveries (team_run_id) WHERE team_run_id IS NOT NULL;

-- Audit retention deletes by age across all organizations.
CREATE INDEX IF NOT EXISTS audit_logs_created_idx ON audit_logs (created_at);
