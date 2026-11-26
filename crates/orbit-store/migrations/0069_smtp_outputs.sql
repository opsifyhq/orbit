-- SMTP execution outputs. Non-secret settings (host, port, TLS mode, from/to/subject) live
-- in `config`; the username/password are stored as an encrypted secret_ref like S3.
ALTER TABLE outputs DROP CONSTRAINT IF EXISTS outputs_kind_check;
ALTER TABLE outputs ADD CONSTRAINT outputs_kind_check
    CHECK (kind IN ('slack', 'mattermost', 'webhook', 'telegram', 's3', 'smtp'));
