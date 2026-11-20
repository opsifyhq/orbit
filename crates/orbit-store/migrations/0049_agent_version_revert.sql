-- An agent may return to an earlier configuration: saving a spec identical to an older
-- version now appends it as the next version (versions stay immutable and append-only).
-- Only a save identical to the current version is refused; the API already treats that
-- as a metadata-only edit. Lookups use (agent_id, version), so no checksum index is kept.
ALTER TABLE agent_versions DROP CONSTRAINT IF EXISTS agent_versions_agent_id_checksum_key;
