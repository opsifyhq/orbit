# Dependency and harness updates

`versions.json` is the source of truth for pins that ecosystem tools cannot update:
Rust, Node, PostgreSQL, Helm, OpenCode, Pi, `gh` and `glab`. Dependabot handles Cargo,
npm, Docker and GitHub Actions through `.github/dependabot.yml`.

## Automated review

The weekly **Dependency watch** workflow (`dependency-watch.yml`) runs
`scripts/check-updates.mjs`, which checks upstream releases and npm. It only proposes
updates within the pinned major; it never edits files. Manual runs default to dry-run.
When enabled, it opens or comments on an issue labelled `dependency-update` with the
version list and the checks that are required.

## Applying an update

1. Edit `versions.json`.
2. Run `node scripts/sync-external-versions.mjs` for the generated Dockerfile/toolchain
   pins. Node/Postgres image digests are checked but intentionally not guessed.
3. Run `node scripts/check-external-versions.mjs` and `./scripts/check-versions.sh`.
4. Run the matching E2E before changing an `ImplementationStatus` or “tested version”
   note: `cargo test -p orbit-harness --test opencode_e2e` or `pi_e2e` (opt-in, paid
   credentials). Update the docs only after the test passes.
5. Run the normal CI: cargo fmt/clippy/test, pnpm typecheck/test/build, helm lint/template,
   `pnpm docs:check`, cargo audit.
6. Never auto-merge. A harness bump is not “available” until the real harness passed.

OpenCode/Pi releases can change protocol behaviour; keep the tested versions in
`docs/architecture/runtime-contract.md` and the adapter descriptors aligned.

## Accepted warnings

`cargo audit` reports `chrono` and `chrono-tz` as unmaintained informational warnings.
The decision and review date are in
[ADR 0016](../decisions/0016-chrono-dependencies.md). The audit output is copied to the CI
job summary; an actual vulnerability still fails the job.
