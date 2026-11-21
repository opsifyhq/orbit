# ADR-0001: Monorepo with a modular monolith control plane

**Status:** Accepted (Phase 0)

## Context
ORBIT needs a fast-moving MVP spanning a web app, a Rust API, runtime adapters and
deployment artifacts, plus a future mobile app that must share contracts.

## Decision
- Single repository with pnpm workspaces (`apps/*`, `packages/*`) and a Cargo workspace
  (`crates/*`, `services/*`).
- The control plane is one deployable binary (`orbit-api`) plus one worker binary
  (`orbit-worker`); domains are separated by crates and module boundaries, not by
  network hops.
- Shared contracts live in Rust (`orbit-contracts`) and TypeScript (`packages/*`),
  generated from a single OpenAPI document for HTTP types.

## Consequences
- One build/test pipeline; contract changes are atomic across frontend and backend.
- Microservices can be extracted later along existing crate boundaries.
- The worker is independently scalable and is the only place runtime adapters run.

## Amendment (2026-11-21): additional binaries
The workspace now builds more than the two binaries named above: `orbit-launcher`
(`services/launcher`, starts workers), `orbit-mcp-broker` (`services/mcp-broker`, an MCP
stdio bridge) and `orbit-license-cli` (`services/license-cli`, offline license signing). The
control plane is still one deployable (`orbit-api`) and the worker the only place harness
adapters run. See [Execution Plane](../architecture/execution-plane.md).
