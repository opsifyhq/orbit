-- #9: Agent Team workflows. A workflow run creates steps from a graph, so a step records
-- which workflow step it ran (`node_id`), which run of that step it is (`attempt`), the
-- outcome label the agent reported, and the deliveries it took (`activated_by`, a JSON
-- array of {"edge": <line id>, "step": <source step_index or null for start>}).
-- Pipeline and native runs leave these at their defaults.
ALTER TABLE team_steps
    ADD COLUMN IF NOT EXISTS node_id text,
    ADD COLUMN IF NOT EXISTS attempt integer NOT NULL DEFAULT 1,
    ADD COLUMN IF NOT EXISTS outcome text,
    ADD COLUMN IF NOT EXISTS activated_by jsonb NOT NULL DEFAULT '[]'::jsonb;

-- A platform-ended execution looks up its step directly; workflow runs have several
-- steps running at once, so "the latest step" is no longer enough.
CREATE INDEX IF NOT EXISTS team_steps_execution_idx
    ON team_steps (execution_id)
    WHERE execution_id IS NOT NULL;
