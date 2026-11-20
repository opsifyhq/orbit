-- The run timeout counts from the moment the harness reports `running`, not from
-- dispatch. `started_at` stays the claim/dispatch time (UI durations and the
-- claimed-silence rule read it); repository checkout and worker start-up no longer
-- consume the run's timeout.
ALTER TABLE executions ADD COLUMN IF NOT EXISTS running_started_at timestamptz;

-- Runs already running keep their current deadline.
UPDATE executions SET running_started_at = started_at
WHERE status IN ('running', 'awaiting_approval') AND running_started_at IS NULL;
