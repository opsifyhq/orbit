-- S3 output configuration (bucket, endpoint, region, prefix, path-style). Other output
-- kinds keep an empty object.
ALTER TABLE outputs
    ADD COLUMN IF NOT EXISTS config jsonb NOT NULL DEFAULT '{}'::jsonb;

ALTER TABLE outputs DROP CONSTRAINT IF EXISTS outputs_kind_check;
ALTER TABLE outputs ADD CONSTRAINT outputs_kind_check
    CHECK (kind IN ('slack', 'mattermost', 'webhook', 'telegram', 's3'));
