-- The `anthropic` provider kind has existed in the contract, the API validation, the
-- worker's provider resolution and the Models page since it was added, but this check
-- still listed only the two older kinds, so saving an Anthropic provider failed with a
-- constraint violation (HTTP 500).
ALTER TABLE providers DROP CONSTRAINT IF EXISTS providers_kind_check;

ALTER TABLE providers
    ADD CONSTRAINT providers_kind_check
    CHECK (kind IN ('openai_compatible', 'anthropic', 'harness_managed'));
