-- Delivery history per output, newest first (output page and the latest delivery per
-- output in the list). The unique (output_id, execution_id) index does not cover the order.
CREATE INDEX IF NOT EXISTS output_deliveries_output_recent_idx
    ON output_deliveries (output_id, created_at DESC);
