#!/usr/bin/env bash
# Fail when the Rust, Node and Helm versions are not aligned.
set -euo pipefail
cd "$(dirname "$0")/.."

python3 - <<'PY'
import json
import pathlib
import re
import sys

versions = {}

cargo = pathlib.Path("Cargo.toml").read_text()
match = re.search(r'(?m)^\[workspace\.package\][\s\S]*?^version\s*=\s*"([^"]+)"', cargo)
versions["Cargo.toml"] = match.group(1) if match else "missing"

for name in [
    "package.json",
    "apps/web/package.json",
    "packages/api-client/package.json",
    "packages/contracts/package.json",
]:
    path = pathlib.Path(name)
    if path.exists():
        versions[name] = json.loads(path.read_text()).get("version", "missing")

chart = pathlib.Path("deploy/helm/orbit/Chart.yaml").read_text()
version = re.search(r"(?m)^version:\s*(\S+)", chart)
app_version = re.search(r'(?m)^appVersion:\s*"?([^"\s]+)"?', chart)
versions["Chart.yaml version"] = version.group(1) if version else "missing"
versions["Chart.yaml appVersion"] = app_version.group(1) if app_version else "missing"

readme = re.search(r"(?m)^ORBIT_VERSION=(\S+)$", pathlib.Path("README.md").read_text())
versions["README.md install example"] = readme.group(1) if readme else "missing"

unique = set(versions.values())
for name, value in versions.items():
    print(f"{name}: {value}")
if len(unique) != 1:
    print("ERROR: versions are not aligned", file=sys.stderr)
    sys.exit(1)
PY

# Shipped values files must not pin an image tag that differs from Chart.appVersion
# (an empty tag follows appVersion). Plain grep/awk, no extra dependencies.
app_version="$(awk -F'"' '/^appVersion:/ {print $2; exit}' deploy/helm/orbit/Chart.yaml)"
status=0
for values in deploy/helm/orbit/values*.yaml; do
  while IFS= read -r hit; do
    line="${hit%%:*}"
    tag="$(printf '%s' "${hit#*:}" | sed -E "s/^[[:space:]]*tag:[[:space:]]*//; s/[[:space:]]*(#.*)?\$//; s/^[\"'](.*)[\"']\$/\\1/")"
    if [[ -n "${tag}" && "${tag}" != "${app_version}" ]]; then
      echo "ERROR: ${values}:${line} pins image tag '${tag}' (appVersion is ${app_version}); leave tag empty" >&2
      status=1
    fi
  done < <(grep -nE '^[[:space:]]*tag:' "${values}" || true)
done
exit "${status}"
