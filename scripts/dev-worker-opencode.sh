#!/usr/bin/env bash
# Run an OpenCode-capable worker on the host against the Docker Compose stack.
#
# The compose stack provides PostgreSQL, NATS, API and web; this worker uses the host's
# OpenCode CLI/auth and the local worker's MCP bridge subcommand. Keep the containerized mock worker
# running (it handles mock executions).
set -euo pipefail
cd "$(dirname "$0")/.."

export ORBIT_ENV="${ORBIT_ENV:-development}"
export ORBIT_DATABASE_URL="${ORBIT_DATABASE_URL:-postgres://orbit:orbit@localhost:5432/orbit}"
export ORBIT_NATS_URL="${ORBIT_NATS_URL:-nats://localhost:4222}"
export ORBIT_MASTER_KEY="${ORBIT_MASTER_KEY:-b3JiaXQtZGV2LW1hc3Rlci1rZXktMzItYnl0ZXMhISE=}"
export ORBIT_WORKER_HARNESSES="${ORBIT_WORKER_HARNESSES:-opencode}"
export ORBIT_WORKER_POLL_MS="${ORBIT_WORKER_POLL_MS:-300}"
export ORBIT_WORKER_CONCURRENCY="${ORBIT_WORKER_CONCURRENCY:-2}"
export ORBIT_WORKSPACE_ROOT="${ORBIT_WORKSPACE_ROOT:-/tmp/orbit-workspaces}"
export ORBIT_WORKER_TARGET="${ORBIT_WORKER_TARGET:-local}"
export ORBIT_MCP_BROKER_BIN="${ORBIT_MCP_BROKER_BIN:-$PWD/target/debug/orbit-worker}"

cargo build -p orbit-worker
exec cargo run -p orbit-worker
