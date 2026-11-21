# ADR-0002: Runtime adapters behind a trait, executed only in workers

**Status:** Accepted

## Context
Agent runtimes (OpenCode, Claude Code, Pi, custom) have incompatible APIs, lifecycles
and isolation requirements. Coupling the control plane to any of them would break the
platform's core value.

## Decision
- `RuntimeAdapter` is defined in `orbit-contracts` with capabilities, lifecycle,
  execute/resume, streaming events, cancellation and a typed error model.
- Adapters live in `orbit-runtime` and are instantiated by `orbit-worker` only.
- The API persists execution intent and publishes events; it never runs agent logic.
- Adapters are addressed by `RuntimeKind`; new kinds require no domain or API change
  beyond registering a descriptor.
- Production execution is container/Job-isolated; the in-process mock adapter is for
  development and contract tests only.

## Consequences
- Runtime-specific configuration remains opaque JSON, validated by the adapter.
- The worker is the trust boundary between control plane and untrusted agent tasks.
- A runtime can be unavailable without breaking agent management.

## Terminology note (2026-11-21)
ADR 0008 renamed "runtime" to "harness". Read `RuntimeAdapter` as `HarnessAdapter`,
`RuntimeKind` as `HarnessKind` and `orbit-runtime` as `crates/orbit-harness`. The boundary
itself is unchanged: adapters run only in the worker, and `orbit-api` never executes agent
logic.
