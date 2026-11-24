# 0016 — Keep chrono and chrono-tz, with a migration plan

- Status: Accepted
- Date: 2026-11-23
- Review by: 2027-05-23

## Context

`cargo audit` reports RUSTSEC-2026-0543 (`chrono`) and RUSTSEC-2026-0555
(`chrono-tz`) as informational warnings because the crates are unmaintained. There is no
known exploitable advisory, and no fixed release is available. Both crates are used across
the workspace for timestamps, API/DB serialization and schedule time zones.

## Decision

Keep `chrono` 0.4 and `chrono-tz` 0.10 pinned for now. The risk is accepted because:

- there is no vulnerability advisory or fixed upgrade;
- replacing them with `jiff`/`time` touches the domain model, SQLx mappings, OpenAPI,
  scheduling, templates, audit logs and every test fixture;
- the work can be done as one planned migration without mixing it into feature changes.

CI no longer hides the warnings: the cargo-audit output is copied to the job summary. The
review date is recorded above.

## Consequences

- Schedule time-zone handling stays on `chrono-tz` until the migration.
- Any new vulnerability (not just "unmaintained") still fails CI.
- The YAML stack is separate: `serde_yaml` was replaced by the maintained `serde_norway`
  fork (same dependency name) and imports reject anchors, aliases and oversized documents.
