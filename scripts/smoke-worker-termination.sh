#!/usr/bin/env bash
# Cluster end-to-end check for worker termination reasons (#343).
#
# Launches a single-execution worker whose memory limit is far below the worker image's
# startup RSS, so the runtime OOM-kills it. Verifies that:
#   - the in-flight execution fails with `worker_oom` plus pod name, exit code, restart
#     count and termination time, and is marked not retryable,
#   - the launch ends `failed`,
#   - the team step and team run are terminal and the step was not rerun,
#   - a `worker.oom_killed` audit entry exists.
#
# Requires a deployment with a configured launcher and a Kubernetes worker target
# (kind, microk8s or the mkd.local cluster). It creates one agent, one worker profile and
# one team and leaves them behind; delete them afterwards on a shared cluster.
#
# Usage:
#   API_URL=http://orbit.mdk.local ORBIT_API_TOKEN=... ./scripts/smoke-worker-termination.sh
#   MEMORY_LIMIT=8Mi ./scripts/smoke-worker-termination.sh
#
# MEMORY_LIMIT must be low enough that the worker is OOM-killed. If the run ends with
# `worker_exited` or `worker_lost` instead, lower it and try again.
set -euo pipefail

API_URL="${API_URL:-http://localhost:8080}"
TOKEN="${ORBIT_API_TOKEN:-orbit-dev-token}"
MEMORY_LIMIT="${MEMORY_LIMIT:-16Mi}"
WAIT_SECONDS="${WAIT_SECONDS:-180}"

api() { curl -sS -H "Authorization: Bearer ${TOKEN}" -H "content-type: application/json" "$@"; }
fail() { echo "FAIL: $*" >&2; exit 1; }
# field a.b.0.c — dotted path into the JSON on stdin; prints None for a missing key.
field() {
  python3 -c '
import json, sys
value = json.load(sys.stdin)
for key in sys.argv[1].split("."):
    if value is None:
        break
    value = value[int(key)] if isinstance(value, list) else value.get(key)
print(value if isinstance(value, (str, int, float, bool)) or value is None else json.dumps(value))
' "$1"
}

echo "==> Health"
api "${API_URL}/healthz" >/dev/null || fail "health endpoint"
api "${API_URL}/readyz" | grep -q '"status":"ready"' || fail "not ready"

echo "==> Create mock agent"
AGENT_JSON=$(cat <<'JSON'
{
  "apiVersion": "orbit.io/v1",
  "kind": "Agent",
  "metadata": { "name": "Termination Smoke Agent", "description": "created by scripts/smoke-worker-termination.sh" },
  "spec": {
    "harness": { "type": "mock", "executionMode": "on_demand", "config": {} },
    "model": { "management": "harness" },
    "instructions": { "system": "You are a smoke test agent." },
    "connectors": [],
    "triggers": [],
    "autonomy": { "level": 2 },
    "resources": { "timeoutSeconds": 120, "maxConcurrentExecutions": 1 }
  }
}
JSON
)
AGENT=$(api -X POST "${API_URL}/api/v1/agents" -d "${AGENT_JSON}")
AGENT_ID=$(echo "${AGENT}" | field id)
[ -n "${AGENT_ID}" ] && [ "${AGENT_ID}" != "None" ] || fail "create agent: ${AGENT}"
api -X POST "${API_URL}/api/v1/agents/${AGENT_ID}/activate" >/dev/null
echo "    agent ${AGENT_ID}"

echo "==> Create worker profile (memory limit ${MEMORY_LIMIT})"
PROFILE=$(api -X POST "${API_URL}/api/v1/worker-profiles" -d "$(cat <<JSON
{
  "name": "termination-smoke-$(date +%s)",
  "harnessKind": "mock",
  "target": "kubernetes",
  "executionDriver": "process",
  "agentScope": "all",
  "resources": {
    "requests": { "cpu": "50m", "memory": "${MEMORY_LIMIT}" },
    "limits": { "cpu": "200m", "memory": "${MEMORY_LIMIT}" }
  }
}
JSON
)")
PROFILE_ID=$(echo "${PROFILE}" | field id)
[ -n "${PROFILE_ID}" ] && [ "${PROFILE_ID}" != "None" ] || fail "create profile: ${PROFILE}"
api -X POST "${API_URL}/api/v1/worker-profiles/${PROFILE_ID}/enable" >/dev/null
echo "    profile ${PROFILE_ID}"

echo "==> Create auto-launch team"
TEAM=$(api -X POST "${API_URL}/api/v1/agent-teams" -d "$(cat <<JSON
{
  "name": "termination-smoke",
  "mode": "pipeline",
  "instructions": "Reply with one line.",
  "workerProfileId": "${PROFILE_ID}",
  "autoLaunch": true,
  "steps": [{ "agentId": "${AGENT_ID}" }]
}
JSON
)")
TEAM_ID=$(echo "${TEAM}" | field id)
[ -n "${TEAM_ID}" ] && [ "${TEAM_ID}" != "None" ] || fail "create team: ${TEAM}"

echo "==> Start run (launches a single-execution worker)"
RUN=$(api -X POST "${API_URL}/api/v1/agent-teams/${TEAM_ID}/runs" -d '{"task":"smoke"}')
RUN_ID=$(echo "${RUN}" | field run.id)
EXECUTION_ID=$(echo "${RUN}" | field steps.0.executionId)
[ -n "${EXECUTION_ID}" ] && [ "${EXECUTION_ID}" != "None" ] || fail "start run: ${RUN}"
echo "    run ${RUN_ID}, execution ${EXECUTION_ID}"

echo "==> Wait for the execution to end (up to ${WAIT_SECONDS}s)"
STATUS=""
for _ in $(seq 1 "${WAIT_SECONDS}"); do
  sleep 1
  EXECUTION=$(api "${API_URL}/api/v1/executions/${EXECUTION_ID}")
  STATUS=$(echo "${EXECUTION}" | field status)
  case "${STATUS}" in
    succeeded|failed|timed_out|cancelled|rejected) break ;;
  esac
done
[ "${STATUS}" = "failed" ] || fail "expected a failed execution, got '${STATUS}'"

CODE=$(echo "${EXECUTION}" | field error.code)
[ "${CODE}" = "worker_oom" ] || fail "expected error.code worker_oom, got '${CODE}': $(echo "${EXECUTION}" | field error)"
for KEY in workerName exitCode restartCount terminatedAt; do
  VALUE=$(echo "${EXECUTION}" | field "error.${KEY}")
  [ "${VALUE}" != "None" ] || fail "execution error is missing ${KEY}"
done
RETRYABLE=$(echo "${EXECUTION}" | field error.retryable)
[ "${RETRYABLE}" = "False" ] || fail "a runtime kill must not be marked retryable"
echo "    execution failed: $(echo "${EXECUTION}" | field error.message)"

echo "==> Verify the launch ended failed"
LAUNCH_STATUS=$(api "${API_URL}/api/v1/worker-launches?includeInactive=true" | python3 -c '
import json, sys
items = json.load(sys.stdin)
match = [i for i in items if i["profileId"] == sys.argv[1]]
print(match[0]["status"] if match else "missing")
' "${PROFILE_ID}")
[ "${LAUNCH_STATUS}" = "failed" ] || fail "expected a failed launch, got '${LAUNCH_STATUS}'"

echo "==> Verify the team run is terminal and was not retried"
RUN_DETAIL=$(api "${API_URL}/api/v1/team-runs/${RUN_ID}")
RUN_STATUS=$(echo "${RUN_DETAIL}" | field run.status)
[ "${RUN_STATUS}" = "failed" ] || fail "expected a failed team run, got '${RUN_STATUS}'"
STEP_STATUS=$(echo "${RUN_DETAIL}" | field steps.0.status)
[ "${STEP_STATUS}" = "failed" ] || fail "expected a failed team step, got '${STEP_STATUS}'"
STEP_COUNT=$(echo "${RUN_DETAIL}" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["steps"]))')
[ "${STEP_COUNT}" = "1" ] || fail "the step was rerun (${STEP_COUNT} steps)"

echo "==> Verify the audit entry"
FOUND=$(api "${API_URL}/api/v1/audit-logs?action=worker.oom_killed&limit=50" | python3 -c '
import json, sys
items = json.load(sys.stdin)["items"]
print(any(i["targetId"] == sys.argv[1] for i in items))
' "${EXECUTION_ID}")
[ "${FOUND}" = "True" ] || fail "no worker.oom_killed audit entry for ${EXECUTION_ID}"

echo "PASS: OOM-killed worker -> worker_oom on the execution, launch, team run and audit log"
