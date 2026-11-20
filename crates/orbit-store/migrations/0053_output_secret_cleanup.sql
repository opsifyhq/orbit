-- Output webhook URLs are stored as encrypted secrets referenced by `outputs.secret_ref`.
-- Remove the secret whenever the reference disappears (output delete, agent/team/org
-- cascade) or is replaced, so no encrypted URL outlives its output.
CREATE OR REPLACE FUNCTION outputs_cleanup_secret() RETURNS trigger AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        DELETE FROM secrets WHERE reference = OLD.secret_ref;
    ELSIF OLD.secret_ref IS DISTINCT FROM NEW.secret_ref THEN
        DELETE FROM secrets WHERE reference = OLD.secret_ref;
    END IF;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS outputs_cleanup_secret_trg ON outputs;
CREATE TRIGGER outputs_cleanup_secret_trg
    AFTER DELETE OR UPDATE OF secret_ref ON outputs
    FOR EACH ROW EXECUTE FUNCTION outputs_cleanup_secret();

-- Secrets already orphaned by earlier deletes.
DELETE FROM secrets s
WHERE s.reference LIKE 'output/%'
  AND NOT EXISTS (SELECT 1 FROM outputs o WHERE o.secret_ref = s.reference);
