# 0013 — Bulk lifecycle: agents archive, skills delete only

Date: 2026-11-20

## Context

Issue #249 extends the existing bulk selection bar on Agents and Skills with Archive and
Delete. Agents have a persisted `status` column (`draft → active ⇄ paused → archived`), so
archive is reversible and keeps the definition, versions and execution history. Skills have
no archived state: `skills` has no status column, no migration adding one, and no archive
or restore endpoint. Adding one would require a schema change, list filters, restore flows
and a decision about what an archived skill means for agents that reference it by slug.

## Decision

- Agents get bulk **Archive** and bulk **Delete**. Archive sets `status = archived`; normal
  lists hide archived agents, the Archived filter still shows them, and a single activate
  restores them. Delete is permanent and uses the same FK-enforced SQL as single delete.
- Skills get bulk **Delete only**. Bulk archive is deliberately out of scope: there is no
  archived state to set and inventing a soft-delete flag just for the bulk bar would split
  the behaviour from the single-resource API. `POST /api/v1/skills/bulk/archive` does not
  exist.
- Both resources expose `dryRun` on the bulk endpoints so the bar can show what will happen
  before the confirmation ("3 archived, 1 already archived, 2 cannot be deleted"). The
  preview uses the same access checks and database constraints as the real request.
- Partial success is explicit: only ids reported as changed or already in the target state
  are removed from the selection; failed ids stay selected.

## Consequences

- No migration and no new skill status are needed for this issue. Restoring an archived
  skill remains impossible because skills are never archived.
- The bulk preview performs a transactional delete that is rolled back, so FK violations
  are reported from the real constraints rather than a duplicated rule list.
- Adding skill archive later needs a new ADR, a migration and matching single-resource
  endpoints; the bulk bar can then gain an Archive button without changing its shape.
