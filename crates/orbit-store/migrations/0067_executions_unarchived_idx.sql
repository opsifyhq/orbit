-- Queries over the unarchived part of `executions`, which the retention job keeps to
-- ORBIT_ARCHIVE_AFTER_DAYS (30 by default) while archived history keeps growing.
-- Both indexes hold only unarchived rows, so their size and the cost of the queries
-- below follow the retention window instead of total history (#209).
--
-- Plain CREATE INDEX (see 0051): on a very large install, create them by hand with
-- CONCURRENTLY before upgrading (IF NOT EXISTS makes this migration a no-op then).

-- The default executions list (archived = false) counts its total on every page.
-- executions_org_archived_idx cannot answer that without visiting the heap for
-- agent_id (the list joins agents for access), so the planner scanned the whole table.
-- INCLUDE lets the count, and a status-filtered count, run as an index-only scan.
CREATE INDEX IF NOT EXISTS executions_unarchived_idx
    ON executions (org_id, created_at DESC, id DESC)
    INCLUDE (agent_id, status)
    WHERE archived_at IS NULL;

-- The retention job (every 6 hours, all organizations) archives finished runs older
-- than the cutoff; without an index on finished_at it scanned the whole table.
CREATE INDEX IF NOT EXISTS executions_archive_due_idx
    ON executions (finished_at)
    WHERE archived_at IS NULL AND finished_at IS NOT NULL;
