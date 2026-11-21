#!/usr/bin/env bash
# Bump the ORBIT version everywhere it is declared.
# Usage: ./scripts/bump-version.sh 0.9.1
set -euo pipefail

VERSION="${1:-}"
if [[ -z "${VERSION}" ]]; then
  echo "usage: $0 <version>" >&2
  exit 1
fi

cd "$(dirname "$0")/.."

python3 - "$VERSION" <<'PY'
import json
import pathlib
import re
import sys

version = sys.argv[1]

# Rust workspace version
cargo = pathlib.Path("Cargo.toml")
text = cargo.read_text()
text = re.sub(
    r'(?m)^(\[workspace\.package\][\s\S]*?^version\s*=\s*)"[^"]+"',
    lambda match: f'{match.group(1)}"{version}"',
    text,
    count=1,
)
cargo.write_text(text)

# Node package versions (first "version" field only)
for name in [
    "package.json",
    "apps/web/package.json",
    "packages/api-client/package.json",
    "packages/contracts/package.json",
]:
    path = pathlib.Path(name)
    if not path.exists():
        continue
    data = json.loads(path.read_text())
    data["version"] = version
    path.write_text(json.dumps(data, indent=2) + "\n")

# Helm chart version + appVersion
chart = pathlib.Path("deploy/helm/orbit/Chart.yaml")
text = chart.read_text()
text = re.sub(r"(?m)^version:\s*.*$", f"version: {version}", text, count=1)
text = re.sub(r'(?m)^appVersion:\s*.*$', f'appVersion: "{version}"', text, count=1)
chart.write_text(text)

# README install example (`ORBIT_VERSION=X.Y.Z`)
readme = pathlib.Path("README.md")
text = readme.read_text()
text, count = re.subn(r"(?m)^ORBIT_VERSION=\S+$", f"ORBIT_VERSION={version}", text)
if count != 1:
    sys.exit("README.md: expected exactly one ORBIT_VERSION=<version> line")
readme.write_text(text)

print(f"bumped to {version}")
PY

echo "Run: cargo check --workspace   # refresh Cargo.lock"
