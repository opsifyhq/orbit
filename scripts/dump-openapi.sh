#!/usr/bin/env bash
# Dump the OpenAPI document straight from ApiDoc, with no running API or database.
# Usage: ./scripts/dump-openapi.sh [output.json]   (default: target/orbit-openapi.json)
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
spec="${1:-"$root/target/orbit-openapi.json"}"

mkdir -p "$(dirname "$spec")"
cd "$root"
ORBIT_OPENAPI_DUMP="$spec" cargo test -p orbit-api --locked --test openapi_parity
echo "OpenAPI document written to $spec" >&2
