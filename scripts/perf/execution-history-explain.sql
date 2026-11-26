-- EXPLAIN (ANALYZE, BUFFERS) for the periodic and per-request queries of #209, against a
-- database seeded with execution-history-seed.sql. Writes run inside BEGIN/ROLLBACK.
-- The SQL mirrors crates/orbit-store/src (workers.rs, executions.rs, dashboard.rs) with
-- the binds inlined as a privileged caller would send them.
\pset pager off
\set org '''00000000-0000-0000-0000-000000000001'''
SELECT id AS agent FROM agents WHERE org_id = :org ORDER BY id LIMIT 1 \gset
SELECT id AS trig FROM triggers WHERE agent_id = :'agent' \gset
\echo '### 1 orphan reaper'
BEGIN;
EXPLAIN (ANALYZE, BUFFERS, COSTS OFF, SUMMARY ON)
UPDATE executions e SET status = 'failed', finished_at = now(), updated_at = now()
WHERE e.status IN ('dispatched', 'running', 'awaiting_approval')
  AND e.worker_id IS NOT NULL
  AND COALESCE(e.started_at, e.created_at) < now() - interval '15 seconds'
  AND (NULL::uuid IS NULL OR e.org_id = NULL::uuid)
  AND NOT EXISTS (SELECT 1 FROM worker_heartbeats h WHERE h.worker_id = e.worker_id AND h.last_heartbeat_at > now() - make_interval(secs => 60))
  AND (e.status <> 'dispatched' OR EXISTS (SELECT 1 FROM worker_heartbeats h WHERE h.worker_id = e.worker_id))
RETURNING e.id;
ROLLBACK;
\echo '### 2 timeout reaper (approvals sum)'
BEGIN;
EXPLAIN (ANALYZE, BUFFERS, COSTS OFF, SUMMARY ON)
UPDATE executions e SET status = 'timed_out', finished_at = now(), updated_at = now()
WHERE e.status = 'running'
  AND COALESCE(e.running_started_at, e.started_at) IS NOT NULL
  AND (NULL::uuid IS NULL OR e.org_id = NULL::uuid)
  AND COALESCE(e.running_started_at, e.started_at) + make_interval(secs => e.timeout_seconds::float8 + 30
      + COALESCE((SELECT sum(EXTRACT(EPOCH FROM (COALESCE(a.decided_at, now()) - a.created_at))) FROM approvals a WHERE a.execution_id = e.id), 0)::float8) < now()
RETURNING e.id;
ROLLBACK;
\echo '### 3 trigger delete (ON DELETE SET NULL on executions.trigger_id)'
BEGIN;
EXPLAIN (ANALYZE, BUFFERS, COSTS OFF, SUMMARY ON) DELETE FROM triggers WHERE id = :'trig';
ROLLBACK;
\echo '### 4 count_active_executions_for_agent'
EXPLAIN (ANALYZE, BUFFERS, COSTS OFF, SUMMARY ON)
SELECT COUNT(*) FROM executions WHERE org_id = :org AND agent_id = :'agent'
  AND status IN ('queued', 'dispatched', 'running', 'awaiting_approval');
\echo '### 5 dashboard executions_24h / failed_24h / running'
EXPLAIN (ANALYZE, BUFFERS, COSTS OFF, SUMMARY ON)
SELECT COUNT(*) FILTER (WHERE e.created_at > now() - interval '24 hours'),
       COUNT(*) FILTER (WHERE e.created_at > now() - interval '24 hours' AND e.status IN ('failed', 'timed_out')),
       COUNT(*) FILTER (WHERE e.status IN ('queued', 'dispatched', 'running'))
FROM executions e JOIN agents a ON a.id = e.agent_id
WHERE e.org_id = :org AND (true OR a.owner_user_id IS NULL)
  AND (e.created_at > now() - interval '24 hours' OR e.status IN ('queued', 'dispatched', 'running'));
\echo '### 6 dashboard pending approvals'
EXPLAIN (ANALYZE, BUFFERS, COSTS OFF, SUMMARY ON)
SELECT COUNT(*) FROM approvals p JOIN executions e ON e.id = p.execution_id JOIN agents a ON a.id = e.agent_id
WHERE p.org_id = :org AND p.status = 'pending' AND (true OR a.owner_user_id IS NULL);
\echo '### 7 list_executions total (default list: archived = false), as prepared by the store'
PREPARE list_total(uuid, text, uuid, boolean, bool, uuid, uuid[], bool) AS
SELECT COUNT(*) FROM executions e
JOIN agents a ON a.id = e.agent_id
WHERE e.org_id = $1
  AND ($2::text IS NULL OR e.status = ANY(string_to_array($2, ',')))
  AND ($3::uuid IS NULL OR e.agent_id = $3)
  AND ($4::boolean IS NULL OR ($4 = FALSE AND e.archived_at IS NULL) OR ($4 = TRUE AND e.archived_at IS NOT NULL))
  AND ($5::bool OR a.owner_user_id = $6
       OR CASE WHEN $8::bool AND a.owner_team_id IS NOT NULL
            THEN a.owner_team_id = ANY($7)
            ELSE a.visibility = 'org'
                 OR (a.visibility = 'team' AND a.owner_team_id = ANY($7)) END);
EXPLAIN (ANALYZE, BUFFERS, COSTS OFF, SUMMARY ON)
EXECUTE list_total(:org, NULL, NULL, false, true, NULL, '{}', false);
\echo '### 8 retention job archive step'
BEGIN;
EXPLAIN (ANALYZE, BUFFERS, COSTS OFF, SUMMARY ON)
UPDATE executions SET archived_at = now(), updated_at = now()
WHERE archived_at IS NULL AND finished_at IS NOT NULL AND finished_at < now() - interval '30 days'
  AND status IN ('succeeded', 'failed', 'timed_out', 'cancelled', 'rejected');
ROLLBACK;
\echo '### 9 unstarted reaper'
BEGIN;
EXPLAIN (ANALYZE, BUFFERS, COSTS OFF, SUMMARY ON)
UPDATE executions e SET status = 'failed', finished_at = now(), updated_at = now()
WHERE e.status = 'dispatched' AND (NULL::uuid IS NULL OR e.org_id = NULL::uuid)
  AND GREATEST(e.updated_at, COALESCE(e.started_at, e.created_at),
        COALESCE((SELECT max(ev.created_at) FROM execution_events ev WHERE ev.execution_id = e.id), e.created_at))
      < now() - make_interval(secs => CASE WHEN e.cancel_requested THEN 60 ELSE 600 END)
RETURNING e.id;
ROLLBACK;
\echo '### 10 lease reaper'
BEGIN;
EXPLAIN (ANALYZE, BUFFERS, COSTS OFF, SUMMARY ON)
UPDATE executions e SET status = 'failed', finished_at = now(), updated_at = now(), execution_lease_until = NULL
WHERE e.status IN ('dispatched', 'running', 'awaiting_approval') AND e.execution_lease_until IS NOT NULL
  AND e.execution_lease_until < now()
  AND EXISTS (SELECT 1 FROM worker_heartbeats h WHERE h.worker_id = e.worker_id AND h.last_heartbeat_at > now() - make_interval(secs => 60))
RETURNING e.id;
ROLLBACK;
