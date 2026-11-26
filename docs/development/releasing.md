# Releasing ORBIT

A release is a version bump, a changelog entry, an annotated git tag, and a green CI run that
publishes images and binaries. Everything below is automated except the bump and tag.

## When to release

Whoever owns the change chooses the moment; releases are not tied one-to-one to issues.
Guidelines:

| Change | Next version | Release when |
|---|---|---|
| Bug fix, docs, dependency bump | patch (`0.9.1`) | batched weekly, or immediately for a blocking fix |
| Feature, chart default/behaviour change, API addition | minor (`0.10.0`) | when the feature is merged and verified, usually weekly |
| Breaking change (API, spec, chart values) | major (or a documented minor while pre-1.0) | announced in the changelog/ADR first |

- Release only from `main` with green CI; never from a branch.
- One tag ships **all six images** plus the chart and binaries. If any image is missing,
  the release is incomplete (this is why `launcher`/harness images must be in the tag run).
  CI enforces it: the chart and the GitHub release are published only after every image
  job succeeded, and pull requests already build the harness images.
- Do not retag a published version. If a release is broken, fix forward with a patch.
- A release is not a per-issue event: bundle related work, but do not hold a fix for an
  unrelated unfinished feature.

## Version alignment

`./scripts/bump-version.sh X.Y.Z` updates the Rust workspace, every Node package, the
Helm chart (`version` **and** `appVersion`) and `info.version` of
`packages/api-client/openapi.json`. `./scripts/check-versions.sh` fails CI when
they drift, and the chart's `appVersion` is the default image tag workers receive.

## 1. Prepare the version

Use the helper so every declaration moves together:

```bash
./scripts/bump-version.sh X.Y.Z
cargo check --workspace   # refresh Cargo.lock versions
./scripts/check-versions.sh
```

The script updates `Cargo.toml` `[workspace.package]`, the four `package.json` files,
`Chart.yaml` `version`/`appVersion`, `info.version` in `packages/api-client/openapi.json`
and the `ORBIT_VERSION=` line of the install example in `README.md`; `check-versions.sh`
compares all of them. `Chart.yaml` `appVersion` is the
default image tag, so it must equal the version CI publishes.

Update the changelog from the fragments that pull requests added to `changelog.d/`:

```bash
node scripts/changelog.mjs preview          # what the release section will say
node scripts/changelog.mjs collect X.Y.Z    # writes ## [X.Y.Z] - today, deletes the fragments
```

`collect` merges anything still written directly under `## [Unreleased]` with the
fragments, orders the sections as Keep a Changelog does (Added, Changed, Deprecated,
Removed, Fixed, Security; other headings such as Upgrade after them), leaves an empty
`## [Unreleased]` and removes the fragment files. Review the result and edit it if two
entries describe the same change. It refuses to run when the version already has a
section or there is nothing to release; `--date YYYY-MM-DD` overrides today's date. The
release notes are generated from git history, but the changelog is the human-facing
record.

## 2. Verify locally

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
ORBIT_TEST_DATABASE_URL=postgres://orbit:orbit@localhost:5432/orbit cargo test --workspace

pnpm --filter @orbit/web typecheck
pnpm --filter @orbit/web build

helm lint deploy/helm/orbit
helm template orbit deploy/helm/orbit -n orbit --set auth.acknowledgeInsecureDev=true >/dev/null
```

For the harness images, `scripts/build-images.sh` builds every image; the harness images
extend the base worker image, so build the base first.

## 3. Tag and push

```bash
git add -A
git commit -m "release: vX.Y.Z"
git tag -a vX.Y.Z -m "ORBIT vX.Y.Z"
git push origin main
git push origin vX.Y.Z
```

The tag triggers the **CI** workflow. On a `v*` tag it:

1. Runs rustfmt, clippy, tests (with a PostgreSQL service), `cargo audit`, `pnpm audit`
   and a TruffleHog secret scan.
2. Builds and pushes `orbit-api`, `orbit-worker`, `orbit-web` and `orbit-launcher` to
   GHCR for `linux/amd64`. A tag push `vX.Y.Z` produces the image tags `X.Y.Z`, `X.Y`,
   `latest` (added by `docker/metadata-action` for a release tag) and the full commit
   SHA. The `v` prefix is dropped: there is no `vX.Y.Z` image tag, and no branch-name or
   pull-request tag is produced on a tag run. A manual run with `push_images=true`
   instead pushes the branch name and the commit SHA.
3. Builds `orbit-worker-opencode` and `orbit-worker-pi` from the freshly pushed base
   worker image (using the commit SHA tag as `WORKER_BASE`), with the same tags.
4. Builds the `orbit-worker` binary for `x86_64` and attaches it to the GitHub release.
5. Publishes the Helm chart package (`orbit-X.Y.Z.tgz`) as a release asset.

## 4. Verify the release

```bash
gh release view vX.Y.Z
# images (requires a token with read:packages)
docker pull ghcr.io/opsifyhq/orbit/orbit-api:X.Y.Z
docker pull ghcr.io/opsifyhq/orbit/orbit-worker:X.Y.Z
docker pull ghcr.io/opsifyhq/orbit/orbit-web:X.Y.Z
docker pull ghcr.io/opsifyhq/orbit/orbit-worker-opencode:X.Y.Z
docker pull ghcr.io/opsifyhq/orbit/orbit-worker-pi:X.Y.Z
docker pull ghcr.io/opsifyhq/orbit/orbit-launcher:X.Y.Z
```

The chart is also installable straight from the release:

```bash
helm template orbit \
  https://github.com/opsifyhq/orbit/releases/download/vX.Y.Z/orbit-X.Y.Z.tgz \
  -n orbit --set auth.acknowledgeInsecureDev=true | grep 'image:'
```

Install the chart against a scratch namespace and confirm it renders the release tags:

```bash
helm template orbit deploy/helm/orbit -n orbit \
  --set imageRegistry=ghcr.io/opsifyhq/orbit \
  --set auth.acknowledgeInsecureDev=true | grep 'image:'
```

## 5. Deploy a private registry (optional)

Clusters that cannot reach GHCR can mirror the images and point the chart at the mirror:

```bash
REGISTRY=registry.example.com:5000 TAG=X.Y.Z ./scripts/build-images.sh
PUSH_REGISTRY=registry.example.com:5000 REGISTRY=registry.example.com:5000 \
  TAG=X.Y.Z ./scripts/push-images.sh

KUBE_CONTEXT=your-cluster NAMESPACE=orbit TAG=X.Y.Z \
  REGISTRY=registry.example.com:5000 INGRESS_HOST=orbit.example.com ./scripts/deploy.sh
```

`scripts/deploy.sh` is a helper for the maintainers' development cluster, not a
production installer. It always sets `platform.environment=development`,
`auth.acknowledgeInsecureDev=true` and hostpath storage classes, sets
`platform.corsOrigins` / `platform.publicBaseUrl` from the ingress host, and enables a
fixed worker only when a worker token already exists in the cluster secret. For a
production install on a mirrored registry, run `helm upgrade --install` with
`values-prod.example.yaml` and `--set imageRegistry=…` instead (see
[deployment.md](deployment.md)). The launcher token is generated by the chart and reused
across upgrades.

## Notes

- Migrations are append-only. A release never edits an applied migration; add a new one.
- `cargo audit` findings fail the build. If a finding has no fix, document the exception
  in the workflow instead of silently ignoring it.
- The chart refuses to render a fixed worker without `worker.token`; platform workers
  launched through the launcher receive a scoped, per-launch token from the API instead.

## Package retention and cleanup

GHCR does not impose a fixed version count, but old images, buildx cache artifacts and
release jobs accumulate. The scheduled **Cleanup packages** workflow runs weekly and uses
`scripts/cleanup-packages.sh`.

Policy:

- keep the newest **10** releases completely (`X.Y.Z` and `X.Y` tags)
- keep the newest **2 patches per minor** line
- protect `latest` and `main`
- delete non-protected versions older than **7 days**
- keep SHA-only versions for **30 days**
- delete Actions/build-cache artifacts older than **14 days**
- never delete a package version that still carries a protected tag

Manual dry-run:

```bash
GH_TOKEN=$(gh auth token) ORG=opsifyhq REPO=opsifyhq/orbit \
  DRY_RUN=true KEEP_RELEASES=10 ./scripts/cleanup-packages.sh
```

Run without `DRY_RUN=true` to delete. Every deletion is logged by the workflow. The
workflow can be started manually: `dry_run=true` (the default) previews, `dry_run=false`
deletes. The weekly schedule always deletes.
