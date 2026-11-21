-- Track when an execution actually entered `running` separately from dispatch/claim.
-- The timeout reaper must count from this moment, not from `started_at`, otherwise
-- checkout and worker startup consume the execution timeout.
ALTER TABLE executions ADD COLUMN IF NOT EXISTS running_started_at TIMESTAMPTZ;

UPDATE executions
SET running_started_at = started_at
WHERE status = 'running'
  AND running_started_at IS NULL
  AND started_at IS NOT NULL;
