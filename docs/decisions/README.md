# Architecture decision records

Decisions that shape ORBIT, in the order they were made. Add a new numbered file for each
new decision; do not edit an accepted one, supersede it.

- [0001 — Monorepo with a modular monolith control plane](0001-monorepo-modular-monolith.md)
- [0002 — Runtime adapters behind a trait, executed only in workers](0002-runtime-boundary.md)
- [0003 — Explicit, kind-tagged model provider configuration](0003-provider-configuration.md)
- [0004 — Pluggable secret store, encrypted at rest](0004-secrets.md)
- [0005 — Event bus abstraction; JetStream in production, memory in dev](0005-events.md)
- [0006 — Token-based auth and SSE for live updates](0006-auth-and-streaming.md)
- [0007 — Immutable, validated agent specs](0007-agent-spec-versioning.md)
- [0008 — Harness / Provider / Model / Worker terminology](0008-harness-provider-worker-terms.md)
- [0009 — Agent teams, execution-level skills and connector capabilities](0009-agent-teams-and-connector-capabilities.md)
- [0010 — Offline licensing and entitlements](0010-offline-licensing.md)
- [0011 — Multi-team access model](0011-multi-team-access-model.md)
- [0012 — Decouple agents from harnesses](0012-decouple-agent-and-harness.md)
- [0013 — Bulk lifecycle: agents archive, skills delete only](0013-bulk-lifecycle.md)
- [0014 — Prometheus metrics from the API only](0014-prometheus-metrics.md)
