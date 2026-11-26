-- Execution-history seed for query-plan checks (#209). Run it on a scratch database with
-- every migration applied, never on a real one:
--
--   psql "$SCRATCH_URL" -v ON_ERROR_STOP=1 -f scripts/perf/execution-history-seed.sql
--   psql "$SCRATCH_URL" -f scripts/perf/execution-history-explain.sql
--
-- 5 organizations, 250 agents with one trigger each, 1M executions over 365 days
-- (900k in organization ...0001), 30% fired by a trigger, 60 in flight, about 200k
-- approvals. Rows older than 30 days are then archived, as the retention job does with
-- the default ORBIT_ARCHIVE_AFTER_DAYS, so the plans reflect a long-running install.
BEGIN;
INSERT INTO organizations (id, slug, name)
SELECT ('00000000-0000-0000-0000-00000000000' || g)::uuid, 'org' || g, 'Org ' || g FROM generate_series(1,5) g;
INSERT INTO agents (id, org_id, slug, name, status)
SELECT gen_random_uuid(), ('00000000-0000-0000-0000-00000000000' || (1 + (g % 5)))::uuid, 'agent-' || g, 'Agent ' || g, 'active'
FROM generate_series(1,250) g;
INSERT INTO agent_versions (id, org_id, agent_id, version, spec, checksum)
SELECT gen_random_uuid(), org_id, id, 1, '{}'::jsonb, 'x' FROM agents;
INSERT INTO triggers (id, org_id, agent_id, kind, config)
SELECT gen_random_uuid(), a.org_id, a.id, 'schedule', '{}'::jsonb FROM agents a;
CREATE TEMP TABLE ag AS
SELECT a.id, a.org_id, v.id AS vid, t.id AS tid
FROM agents a JOIN agent_versions v ON v.agent_id = a.id JOIN triggers t ON t.agent_id = a.id;
-- Organization ...0001 gets 900k executions over its 50 agents; the rest share 100k.
CREATE TEMP TABLE ag1 AS SELECT row_number() OVER (ORDER BY id) - 1 AS n, * FROM ag WHERE org_id = '00000000-0000-0000-0000-000000000001';
CREATE TEMP TABLE agx AS SELECT row_number() OVER (ORDER BY id) - 1 AS n, * FROM ag WHERE org_id <> '00000000-0000-0000-0000-000000000001';
INSERT INTO executions (id, org_id, agent_id, agent_version_id, agent_name, harness_kind, trigger_id, trigger_source,
                        status, created_at, started_at, finished_at, updated_at, worker_id)
SELECT gen_random_uuid(), a.org_id, a.id, a.vid, 'agent', 'mock',
       CASE WHEN g % 10 < 3 THEN a.tid END,
       CASE WHEN g % 10 < 3 THEN 'schedule' ELSE 'manual' END,
       CASE WHEN g % 7 = 0 THEN 'failed' ELSE 'succeeded' END,
       ts, ts + interval '5 s', ts + interval '60 s', ts + interval '60 s', 'worker-' || (g % 20)
FROM generate_series(1, 900000) g
JOIN ag1 a ON a.n = g % 50
CROSS JOIN LATERAL (SELECT now() - (g::float8 / 900000) * interval '365 days' AS ts) t;
INSERT INTO executions (id, org_id, agent_id, agent_version_id, agent_name, harness_kind, trigger_id, trigger_source,
                        status, created_at, started_at, finished_at, updated_at, worker_id)
SELECT gen_random_uuid(), a.org_id, a.id, a.vid, 'agent', 'mock', NULL, 'manual', 'succeeded',
       ts, ts, ts + interval '60 s', ts + interval '60 s', 'worker-x'
FROM generate_series(1, 100000) g
JOIN agx a ON a.n = g % 200
CROSS JOIN LATERAL (SELECT now() - (g::float8 / 100000) * interval '365 days' AS ts) t;
-- in flight: 60 recent rows in org 1
UPDATE executions SET status = (ARRAY['dispatched','running','awaiting_approval'])[1 + (abs(hashtext(id::text)) % 3)],
       finished_at = NULL, started_at = now() - interval '2 min', running_started_at = now() - interval '2 min'
WHERE id IN (SELECT id FROM executions WHERE org_id = '00000000-0000-0000-0000-000000000001' ORDER BY created_at DESC LIMIT 60);
INSERT INTO approvals (id, org_id, execution_id, status, created_at, decided_at)
SELECT gen_random_uuid(), org_id, id, 'approved', created_at, created_at + interval '10 s'
FROM executions WHERE abs(hashtext(id::text)) % 5 = 0;
-- Steady state: the retention job has archived everything older than 30 days.
UPDATE executions SET archived_at = now()
WHERE finished_at < now() - interval '30 days'
  AND status IN ('succeeded', 'failed', 'timed_out', 'cancelled', 'rejected');
COMMIT;
VACUUM ANALYZE;
SELECT (SELECT count(*) FROM executions) AS executions,
       (SELECT count(*) FROM executions WHERE archived_at IS NULL) AS unarchived,
       (SELECT count(*) FROM approvals) AS approvals,
       (SELECT count(*) FROM executions WHERE status IN ('dispatched', 'running', 'awaiting_approval')) AS in_flight;
