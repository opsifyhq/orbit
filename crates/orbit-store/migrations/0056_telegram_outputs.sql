-- Telegram execution outputs (#268). The bot token continues to live in the encrypted
-- secret store (`secret_ref`); only the non-secret destination is stored here.
ALTER TABLE outputs DROP CONSTRAINT IF EXISTS outputs_kind_check;
ALTER TABLE outputs ADD CONSTRAINT outputs_kind_check
    CHECK (kind IN ('slack', 'mattermost', 'webhook', 'telegram'));

ALTER TABLE outputs ADD COLUMN IF NOT EXISTS telegram_chat_id text;
ALTER TABLE outputs ADD COLUMN IF NOT EXISTS telegram_message_thread_id text;

-- A Telegram output without a numeric chat id cannot deliver anything.
ALTER TABLE outputs DROP CONSTRAINT IF EXISTS outputs_telegram_target_check;
ALTER TABLE outputs ADD CONSTRAINT outputs_telegram_target_check
    CHECK (kind <> 'telegram' OR telegram_chat_id IS NOT NULL);
