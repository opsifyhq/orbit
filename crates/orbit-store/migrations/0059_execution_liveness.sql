-- #342: track per-execution liveness and the worker boot that owns each active run.
-- A new boot must not inherit an outcome from the boot it replaced, and a run whose
-- lease stops being renewed must be failed even while the worker itself is alive.
ALTER TABLE executions
    ADD COLUMN IF NOT EXISTS last_execution_heartbeat_at timestamptz,
    ADD COLUMN IF NOT EXISTS execution_lease_until timestamptz,
    ADD COLUMN IF NOT EXISTS worker_boot_id uuid,
    ADD COLUMN IF NOT EXISTS worker_boot_started_at timestamptz;

ALTER TABLE worker_heartbeats
    ADD COLUMN IF NOT EXISTS boot_id uuid,
    ADD COLUMN IF NOT EXISTS boot_started_at timestamptz;

-- Lease expiry and boot-owned runs only concern executions that can still finish.
CREATE INDEX IF NOT EXISTS executions_execution_lease_idx
    ON executions (execution_lease_until)
    WHERE status IN ('dispatched', 'running', 'awaiting_approval');

CREATE INDEX IF NOT EXISTS executions_worker_boot_idx
    ON executions (worker_id, worker_boot_id)
    WHERE status IN ('dispatched', 'running', 'awaiting_approval');
