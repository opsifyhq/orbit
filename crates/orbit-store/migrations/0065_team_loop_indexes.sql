-- #447: partial indexes for the team background loops, which run every 2 seconds on
-- every API replica and used to scan all of team_runs / team_queue_items (history is
-- never pruned). Each index only holds the few rows that are currently running.

-- list_running_team_runs: status = 'running' AND id > $1 ORDER BY id.
CREATE INDEX IF NOT EXISTS team_runs_running_idx
    ON team_runs (id)
    WHERE status = 'running';

-- list_team_runs_pending_outcome: running queue items joined to their run.
CREATE INDEX IF NOT EXISTS team_queue_items_running_run_idx
    ON team_queue_items (team_run_id)
    WHERE status = 'running';

-- release_stale_team_queue_claims: running items not yet linked to a run, by age.
CREATE INDEX IF NOT EXISTS team_queue_items_running_unlinked_idx
    ON team_queue_items (updated_at)
    WHERE status = 'running' AND team_run_id IS NULL;
