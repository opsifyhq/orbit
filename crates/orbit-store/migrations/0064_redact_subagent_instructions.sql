-- Native-team runs stored every subagent's system prompt in the primary step's input,
-- which anyone who can view the team or the primary agent can read (#429). The worker
-- now receives the prompt when it claims the step; remove the stored copies.
UPDATE executions
SET input = jsonb_set(
    input,
    '{team,subagents}',
    (SELECT jsonb_agg(subagent - 'instructions')
     FROM jsonb_array_elements(input -> 'team' -> 'subagents') AS subagent)
)
WHERE jsonb_typeof(input -> 'team' -> 'subagents') = 'array'
  AND jsonb_array_length(input -> 'team' -> 'subagents') > 0;

UPDATE team_steps
SET input = jsonb_set(
    input,
    '{team,subagents}',
    (SELECT jsonb_agg(subagent - 'instructions')
     FROM jsonb_array_elements(input -> 'team' -> 'subagents') AS subagent)
)
WHERE jsonb_typeof(input -> 'team' -> 'subagents') = 'array'
  AND jsonb_array_length(input -> 'team' -> 'subagents') > 0;
