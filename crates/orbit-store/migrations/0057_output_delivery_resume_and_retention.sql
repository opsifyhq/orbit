-- #311: a multi-part Telegram delivery resumes after the parts already accepted by
-- Telegram instead of reposting them on a retry.
ALTER TABLE output_deliveries
    ADD COLUMN IF NOT EXISTS parts_sent integer NOT NULL DEFAULT 0;

-- Delivery pruning removes terminal rows older than the retention window. The index keeps
-- that bounded delete from scanning the whole queue; pending/sending rows are never pruned.
CREATE INDEX IF NOT EXISTS output_deliveries_created_at_idx
    ON output_deliveries (created_at);
