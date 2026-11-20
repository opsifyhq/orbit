# ADR-0007: Immutable, validated agent specs

**Status:** Accepted

## Context
Agents must be reproducible: an execution must be explainable months later, and config
drift between edits must not mutate history.

## Decision
- `AgentSpec` v1 (`apiVersion: orbit.io/v1`, `kind: Agent`) with camelCase fields.
- Every configuration change creates a new immutable `agent_versions` row holding
  canonical JSON (`spec`), its SHA-256 (`checksum`), and the source YAML.
- `agents.current_version` points to the active version; executions reference the exact
  `agent_version_id`.
- Validation happens in `orbit-domain` (semantic rules) before persistence; the API maps
  failures to `validation_error` with field paths.
- Runtime-specific configuration lives under `spec.runtime.config` and is only validated
  by the selected adapter, keeping the core schema runtime-agnostic.

## Consequences
- Rollback = point `current_version` at an earlier version.
- Saving the current spec again creates no version (the API applies it as a metadata-only
  edit, and the store refuses an unchanged spec).

## Amendment (2026-11-20): returning to an earlier configuration
Originally every spec had to be unique per agent (`UNIQUE (agent_id, checksum)`), which
made undoing a change impossible: removing a skill that was just added, or switching a
setting back, produced a spec equal to an older version and the save was refused. Since
migration `0049_agent_version_revert` only the *current* version is compared: a spec equal
to an older version is appended as the next version (with the same checksum), so history
stays append-only and shows the return explicitly.
