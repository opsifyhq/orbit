#!/usr/bin/env bash
# Run the host-side dispatcher that starts a **per-execution Docker container** for
# OpenCode agents. The container runs the worker in single-execution mode, including its
# own tool broker and MCP bridge.
#
# Prerequisites:
#   1. PostgreSQL/NATS/API/web are running (docker compose -f deploy/compose/compose.yaml up -d)
#   2. `opencode auth export > .orbit-auth-export.json` (gitignored, contains secrets)
#   3. OpenCode worker image built for your host architecture:
#        make compose-build-opencode
set -euo pipefail
cd "$(dirname "$0")/.."

AUTH_EXPORT="${ORBIT_OPENCODE_AUTH_EXPORT:-$PWD/.orbit-auth-export.json}"
if [ ! -f "$AUTH_EXPORT" ]; then
  echo "Missing $AUTH_EXPORT." >&2
  echo "Create it with: opencode auth export > .orbit-auth-export.json" >&2
  exit 1
fi

export ORBIT_ENV="${ORBIT_ENV:-development}"
export ORBIT_DATABASE_URL="${ORBIT_DATABASE_URL:-postgres://orbit:orbit@localhost:5432/orbit}"
export ORBIT_NATS_URL="${ORBIT_NATS_URL:-nats://localhost:4222}"
export ORBIT_MASTER_KEY="${ORBIT_MASTER_KEY:-b3JiaXQtZGV2LW1hc3Rlci1rZXktMzItYnl0ZXMhISE=}"
export ORBIT_WORKER_HARNESSES="${ORBIT_WORKER_HARNESSES:-opencode}"
export ORBIT_WORKER_POLL_MS="${ORBIT_WORKER_POLL_MS:-300}"
export ORBIT_WORKER_CONCURRENCY="${ORBIT_WORKER_CONCURRENCY:-2}"
export ORBIT_WORKSPACE_ROOT="${ORBIT_WORKSPACE_ROOT:-/tmp/orbit-workspaces}"
export ORBIT_MCP_BROKER_BIN="${ORBIT_MCP_BROKER_BIN:-$PWD/target/debug/orbit-worker}"
export ORBIT_WORKER_TARGET="${ORBIT_WORKER_TARGET:-docker}"
export ORBIT_EXECUTION_DRIVER=docker
export ORBIT_EXECUTION_IMAGE="${ORBIT_EXECUTION_IMAGE:-orbit-worker-opencode:dev}"
export ORBIT_OPENCODE_AUTH_EXPORT="$AUTH_EXPORT"

cargo build -p orbit-worker
exec cargo run -p orbit-worker
