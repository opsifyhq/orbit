-- An output can target several agents. Keep the legacy agent_id column for existing
-- direct inserts, but stop it from cascading a multi-target output away when one agent is
-- deleted; agent_ids is the source of truth.
ALTER TABLE outputs
    ADD COLUMN IF NOT EXISTS agent_ids uuid[] NOT NULL DEFAULT '{}';

UPDATE outputs
SET agent_ids = ARRAY[agent_id]
WHERE agent_id IS NOT NULL AND cardinality(agent_ids) = 0;

ALTER TABLE outputs DROP CONSTRAINT IF EXISTS outputs_agent_id_fkey;
UPDATE outputs SET agent_id = NULL;

CREATE INDEX IF NOT EXISTS outputs_agent_ids_idx ON outputs USING GIN (agent_ids);
