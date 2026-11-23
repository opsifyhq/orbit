-- The installation-wide license count selects executions by created_at only. The
-- existing partial indexes do not cover all statuses, so add a plain index for it.
-- Plain CREATE INDEX matches the rest of the migrations (sqlx runs each in a
-- transaction); on a very large install, create it by hand with CONCURRENTLY before
-- upgrading -- IF NOT EXISTS makes this migration a no-op then.
CREATE INDEX IF NOT EXISTS executions_created_at_idx ON executions (created_at);
