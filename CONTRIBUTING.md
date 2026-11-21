# Contributing to ORBIT

Thank you for helping. This page describes how a change gets from an idea to `main`.
[`AGENTS.md`](./AGENTS.md) holds the engineering rules themselves and applies to human
contributors and coding agents alike; read it before your first change.

## Before you start

- Look for an existing [issue](https://github.com/opsifyhq/orbit/issues) or open one that
  describes the problem. For anything larger than a bug fix, agree on the approach in the
  issue first.
- A decision that changes the architecture or replaces a technology needs a decision
  record in [`docs/decisions/`](./docs/decisions/README.md).
- Do not report a suspected vulnerability in a public issue. Contact the maintainers
  privately first.

## Set up

[Run ORBIT locally](./docs/development/local.md) covers the toolchain (Rust pinned in
`rust-toolchain.toml`, Node 20 or newer, the pnpm version pinned in `package.json`,
Docker with Compose v2) and how to run the API, a worker and the web app.

## Make the change

Work on a branch and keep one concern per pull request. The rules that most often decide
whether a change can be merged:

- **Contracts first.** Change `crates/orbit-contracts` and its documentation before an
  adapter or a handler.
- **No agent logic in the API.** `orbit-api` stores intent and publishes events; agent
  work runs on workers.
- **Permissions are enforced by the platform**, never by a prompt, and they fail closed.
- **Secrets are write-only.** They are never returned by the API, logged, put in an event
  or committed.
- **Migrations are append-only.** Add a new file in `crates/orbit-store/migrations`;
  never edit one that has been merged.
- **Errors use the envelope** `{ "error": { code, message, details, requestId } }`.
- **The web app works on a phone.** Every page must be usable at a width of 360 px and
  follow the [design system](./docs/development/design-system.md).
- **Be honest about what works.** A harness or connector is "available" only after it has
  been tested against the real thing.

## Test it

Which tests a change needs is listed under "Testing expectations for a change" in
`AGENTS.md`; how to run them is in [testing](./docs/development/testing.md). At minimum,
run what CI runs:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
ORBIT_TEST_DATABASE_URL=postgres://orbit:orbit@localhost:5432/orbit cargo test --workspace
pnpm --filter @orbit/web typecheck && pnpm --filter @orbit/web test && pnpm --filter @orbit/web build
pnpm api:generate --check
helm lint deploy/helm/orbit
```

After an API change, run `pnpm api:generate` and commit the regenerated
`packages/api-client/openapi.json` and `src/schema.d.ts`.

## Document it

- Add a line to the `Unreleased` section of [`CHANGELOG.md`](./CHANGELOG.md) for anything
  a user or operator would notice. Mark upgrade steps with **Upgrade:**.
- Update the page under [`docs/`](./docs/README.md) that describes the behaviour you
  changed. A new page must be listed in `docs/navigation.yaml`; a test fails otherwise.
- Keep the phase table in `AGENTS.md` and the status table in `README.md` accurate.

## Open the pull request

Describe what changed and why, how you tested it, and anything a reviewer should look at
first. Reference the issue. A pull request is merged when CI is green and a maintainer
has approved it.

By contributing you agree that your contribution is licensed under the
[Apache License 2.0](./LICENSE).
