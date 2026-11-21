-- no-transaction
-- #208: audit retention deletes the oldest rows in bounded batches ordered by
-- (created_at, id). Without this index each batch needs a sequential scan. CONCURRENTLY
-- keeps audit writes unblocked during the upgrade, which requires running outside a
-- transaction (sqlx's `-- no-transaction` directive above).
CREATE INDEX CONCURRENTLY IF NOT EXISTS audit_logs_created_at_id_idx
    ON audit_logs (created_at, id);
