-- #433: a worker retries an event post when the response is lost, so the API needs a
-- key to recognise the repeat. The worker sends one id per event; a second post with the
-- same id for the same execution returns the stored row instead of adding another.
-- Older workers send no key; their events stay NULL and are never deduplicated.
ALTER TABLE execution_events ADD COLUMN IF NOT EXISTS event_key uuid;
-- Plain CREATE INDEX matches the rest of the migrations (sqlx runs each in a
-- transaction); on a very large install, create it by hand with CONCURRENTLY before
-- upgrading -- IF NOT EXISTS makes this migration a no-op then.
CREATE UNIQUE INDEX IF NOT EXISTS execution_events_event_key_idx
    ON execution_events (execution_id, event_key)
    WHERE event_key IS NOT NULL;
