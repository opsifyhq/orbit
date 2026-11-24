-- Keep the single-agent deletion contract after outputs moved to agent_ids: remove the
-- deleted agent from every target list and delete an output once it has no targets left.
CREATE OR REPLACE FUNCTION orbit_remove_agent_from_outputs() RETURNS trigger AS $$
BEGIN
    UPDATE outputs
    SET agent_ids = array_remove(agent_ids, OLD.id), updated_at = now()
    WHERE OLD.id = ANY(agent_ids);

    DELETE FROM outputs o
    WHERE o.org_id = OLD.org_id
      AND cardinality(o.agent_ids) = 0
      AND o.team_id IS NULL;
    RETURN OLD;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS agents_remove_output_targets ON agents;
CREATE TRIGGER agents_remove_output_targets
    AFTER DELETE ON agents
    FOR EACH ROW EXECUTE FUNCTION orbit_remove_agent_from_outputs();
