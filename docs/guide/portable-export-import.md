# Portable export and import

Every shareable resource can be exported as **secret-free code** (YAML or Markdown) and
imported again, into the same ORBIT or another one. Use it to back up definitions, move
them between environments, review them in Git, or hand them to a colleague.

This is about *definitions*. Who may see or change a resource is access control and lives
in the resource's **Access control** section ([access-teams.md](access-teams.md)); an
import never carries access settings, the importer becomes the owner like for any other create.

## What can be exported

| Resource | `kind` | File | Notes |
|---|---|---|---|
| Agent | `Agent` | `agent.yaml` | The agent definition (`spec` is the same as Agent Studio saves and `POST /api/v1/agents` accepts) |
| Skill | `Skill` | `SKILL.md`, or YAML with files | `SKILL.md` carries the instructions; scripts and references need the YAML form |
| Connection | `Connection` | YAML | Credentials are not included |
| Model provider | `Provider` | YAML | API key is not included |
| Worker profile | `WorkerProfile` | YAML | Environment variable *names* only |
| Agent team | `AgentTeam` | YAML | Agents, profile and connections by name |
| Trigger | `Trigger` | YAML | API only (no page lists triggers on their own) |
| Tool policy | `ToolPolicy` | YAML | API only |

In the web app, **Export as YAML** is in the menu of the agent page, the skill page, the
connection page, the agent team page, the provider card on **Model providers**, and the
worker profile card on **Workers**. **Import** is in the header of Agents, Skills,
Connectors, Model providers, Agent teams and Workers. The import dialog validates the file
first and shows what would be created or updated, conflicts and missing references before
anything is written.

```bash
# portable YAML (default JSON is the legacy wrapper: { resource, id, exportedAt, data })
curl -H "Authorization: Bearer $TOKEN" \
  "$ORBIT/api/v1/export/connections/$ID?format=yaml" -o connection.yaml
# a skill as SKILL.md (bundled files are not part of the Markdown form)
curl -H "Authorization: Bearer $TOKEN" \
  "$ORBIT/api/v1/export/skills/$ID?format=markdown" -o SKILL.md
```

`resource` is one of `agents`, `skills`, `connections`, `providers`, `worker_profiles`,
`agent_teams`, `triggers`, `tool_policies`. Export needs the read permission of the
resource kind and the right to see that particular resource. `GET /api/v1/agents/{id}/export`
returns the same document as `export/agents/{id}?format=yaml`.

## The envelope

Everything except `SKILL.md` uses one envelope:

```yaml
apiVersion: orbit.io/v1
kind: Connection
metadata:
  name: team-github          # identity of the resource within its kind
  description: ...           # optional
spec: { ... }                # kind specific, see below
credentials:                 # informational; ignored on import
  required: true
  note: Credentials are never exported. Set them on the imported resource after import.
```

Rules that hold for every kind:

- **Secrets never leave.** Export removes, recursively, every key that looks like a
  credential (`token`, `secret`, `password`, `apiKey`, `authorization`, `privateKey`,
  `webhookUrl`, `secretRef`, `authSecretName`, `credential`, ...) and every value that looks
  like one (`orbit_sk_...`, `ghp_...`, `glpat-...`, `xoxb-...`, PEM blocks, URLs with
  `user:password@`). Model parameters such as `maxTokens` are not credentials and are kept.
  Free text (agent instructions, skill content, team instructions) is not scanned, so read
  it before sharing.
- **References use names, never ids.** A skill is its `slug`, an agent its `name`, a
  connection `<connector>/<name>` (for example `github/team-github`), a worker profile or
  provider its `name`. Ids differ between installations. A referenced resource you cannot
  view is written by its id instead of its name (an agent binding, work source or
  repository keeps `connectionId`), so an export never reveals the names of resources
  hidden from you. Importing such a document reports the id as an unresolved reference.
- **`credentials.required`** tells the recipient that the source resource had a credential
  (connection, provider, worker profile with env vars or an auth secret, webhook trigger).
  After import, set it on the resource (Connectors, Model providers, Worker profiles).
- Unknown fields are rejected, so typos are reported instead of silently ignored.

### Skill

`SKILL.md` (frontmatter + instructions):

```markdown
---
name: Release notes
slug: release-notes
description: Drafts release notes from merged pull requests
---
# Release notes

Summarise merged PRs by area...
```

Skills with bundled files (`scripts/`, `references/`) use the YAML form:

```yaml
apiVersion: orbit.io/v1
kind: Skill
metadata:
  name: Release notes
  description: Drafts release notes from merged pull requests
spec:
  slug: release-notes
  content: |
    # Release notes
    ...
  files:
    scripts/lint.sh: |
      #!/bin/sh
      echo ok
    references/tone.md: Friendly.
```

Skill files are text (the platform stores them as strings), so the YAML form is a complete
bundle and **no zip dependency** is needed. If the Markdown export drops files, the response
has an `x-orbit-omitted-files` header with the count. The same limits as creating a skill
apply on import (script extensions, sizes, at most 50 files).

### Agent

The same `AgentDefinition` as Agent Studio and `POST /api/v1/agents`:

```yaml
apiVersion: orbit.io/v1
kind: Agent
metadata:
  name: Docs agent
  labels: { team: docs }
spec:
  model: { management: harness }
  instructions: { system: You are a docs agent. }
  connectors:
    - connector: github
      connection: github/team-github   # name instead of connectionId
      capabilities: [issues.list]
  skills: [release-notes]
  autonomy: { level: 2 }
```

`connection: <connector>/<name>` replaces `connectionId` on export and is resolved on
import. Webhook trigger secret references are removed.

### Connection

```yaml
apiVersion: orbit.io/v1
kind: Connection
metadata: { name: team-github }
spec:
  connector: github
  scopes: [repo:read]
  config: { owner: acme }
credentials: { required: true, note: ... }
```

### Provider

```yaml
apiVersion: orbit.io/v1
kind: Provider
metadata: { name: Gateway }
spec:
  kind: openai_compatible
  enabled: true
  config: { baseUrl: "http://gateway:9999/v1", model: qwen }
credentials: { required: true, note: ... }
```

### Worker profile

```yaml
apiVersion: orbit.io/v1
kind: WorkerProfile
metadata: { name: docs-worker }
spec:
  harnessKind: opencode
  target: docker
  executionDriver: process
  image: ghcr.io/acme/orbit-worker-opencode
  imageTag: ""
  agentScope: selected
  agents: [Docs agent]            # agent names
  skills: [release-notes]
  provider: Gateway
  model: qwen
  envMappings: [shared-github]    # names of reusable environment mappings
  env:
    GITHUB_TOKEN: <set-on-import> # names only; values never leave
    LOG_LEVEL: <set-on-import>
  resources: { ... }
  allowWebSearch: false
  allowWebFetch: false
  allowGlab: false
  allowGh: true
  nativeToolPermissions: { bash: ask }
```

Environment variables are imported **empty**: set their values afterwards. Updating an
existing profile keeps the values it already has for variables that are still listed, and
keeps its auth secret.

### Agent team

```yaml
apiVersion: orbit.io/v1
kind: AgentTeam
metadata: { name: Docs team, description: Pipeline }
spec:
  mode: pipeline                  # or native
  instructions: Ship the docs.
  workerProfile: docs-worker
  connections: [github/team-github]
  envMappings: []
  skills: [release-notes]
  workSource:                     # optional
    provider: github
    enabled: true
    connection: github/team-github
    repository: acme/docs
  repositories:
    - { connection: github/team-github, repository: acme/docs, primary: true }
  steps:
    - agent: Docs agent
```

A [workflow](agent-teams.md#workflows) team uses `mode: workflow` and a `workflow` block
whose steps name their agent; `steps` can be left out:

```yaml
spec:
  mode: workflow
  workflow:
    nodes:
      - { id: test, name: Test, agent: Tester, outcomes: [pass, fail], maxRuns: 3 }
      - { id: fix, name: Fix, agent: Docs agent }
    edges:
      - { id: line-1, from: start, to: test, when: always }
      - { id: line-2, from: test, to: fix, when: outcome, outcome: fail }
      - { id: line-3, from: fix, to: test, when: success }
```

### Trigger

Identified by agent, kind and config (a trigger has no name of its own):

```yaml
apiVersion: orbit.io/v1
kind: Trigger
metadata: { name: Docs agent schedule }   # informational
spec:
  agent: Docs agent
  kind: schedule                          # schedule | event | webhook | manual
  enabled: true
  config: { cron: "0 9 * * 1" }
```

Importing an identical trigger conflicts in `create` mode; in `update`/`upsert` mode it
only sets `enabled`. A webhook trigger needs its secret set after import. Only `schedule`
and `manual` triggers start runs today; `event` and `webhook` triggers are stored but never
fire ([agents](agents.md#triggers)).

### Tool policy

```yaml
apiVersion: orbit.io/v1
kind: ToolPolicy
metadata: { name: no-rm }
spec:
  agent: Docs agent        # omit for an organisation-wide rule
  priority: 10
  action: deny             # allow | deny | require_approval
  matcher: { tool: bash, commandRegex: "rm -rf" }
  reason: destructive
```

### Bundles

Several documents in one file (separated by `---`), or a JSON array of envelopes, form a
bundle. Documents are imported in dependency order (providers, skills, connections, agents,
worker profiles, teams, triggers, tool policies), and a document may reference another one
in the same bundle that is applied before it.

## Importing

`POST /api/v1/import` takes a `SKILL.md`, a YAML/JSON document or a bundle as the request
body (`Content-Type: application/yaml`, optional `X-Orbit-Filename`), or JSON
`{ "content": "...", "filename": "..." }`.

| Query | Meaning |
|---|---|
| `mode=create` (default) | Create only. A name that already exists is an error (**409**), nothing is overwritten. |
| `mode=update` | Update existing resources only; a missing one is reported. |
| `mode=upsert` | Create when missing, update when present. |
| `dryRun=true` | Validate and return the report. Nothing is written. |

```bash
# 1. see what would happen
curl -X POST -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/yaml" \
  --data-binary @bundle.yaml "$ORBIT/api/v1/import?mode=upsert&dryRun=true"
# 2. do it
curl -X POST -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/yaml" \
  --data-binary @bundle.yaml "$ORBIT/api/v1/import?mode=upsert"
```

The identity used for conflicts is: agent, provider, worker profile, team: `metadata.name`;
skill: `spec.slug`; connection: connector + name; tool policy: agent + name; trigger:
agent + kind + config.

### Validation and the report

Before anything is written every document is parsed, checked against the schema of its
`kind`, and every reference is resolved by name in your organisation (only resources you
may see count). The report lists, per document:

```json
{
  "dryRun": true, "mode": "create", "valid": false, "applied": false,
  "items": [{
    "kind": "Agent", "name": "Docs agent", "status": "unresolved_references",
    "action": null, "id": null,
    "unresolvedReferences": [
      { "kind": "skill", "reference": "release-notes", "path": "spec.skills[0]" }
    ],
    "issues": [], "warnings": [], "credentialsRequired": false
  }]
}
```

`status` is `ok`, `conflict`, `invalid` or `unresolved_references`. `warnings` lists
ignored credential fields and what to set after import.

- With `dryRun=true` the report is always returned with **200**, valid or not.
- A real import with any problem writes **nothing** and returns the error envelope:
  **409** `conflict` if something already exists, otherwise **422** `validation_error`.
  `error.details` has one entry per problem (all of them, not just the first).
- Success returns the report with `applied: true` and the ids: **201** if something was
  created, **200** if only updates.

Writes go through the same handlers as the UI, so limits, ownership, validation and the
normal `*.created` / `*.updated` audit entries are identical. Validation is complete before
the first write; if a database error still interrupts a multi-document bundle, the message
says how many documents were already imported and re-running with `mode=upsert` finishes
the job.

### Credentials are never imported

Credential-looking fields in a document are dropped with a warning (a hand-edited file
cannot reference another secret in the platform). Set credentials on the imported
resource afterwards. Updating through an import keeps existing credentials and, on
connections and providers, existing credential-like keys in the stored config.

### Permissions and audit

The importer needs the permission to create the resource (`agents:write` for agents and
skills, `connections:write`, `providers:write`, `triggers:write`, `settings:write` for worker
profiles, teams and tool policies), also for a dry run. Updating additionally requires
that you may manage the existing resource. Imported resources are owned by the importer
like any other create.

Audit log: `resource.imported` per document (kind, name, action, mode, file name),
`import.completed` for the whole request, `import.rejected` when a request is refused
because of conflicts or validation, and `import.failed` if applying stopped midway.

## Not covered

- Environment mappings, outputs, access teams, roles and platform settings.
- Access settings (visibility, owner team) of exported resources.
- Credentials of any kind, by design.
- OpenCode-style agent/skill Markdown is a separate importer
  (`POST /api/v1/import/opencode`; the "Import agent .md" and "Import SKILL.md" buttons on
  Agents and Skills). SKILL.md files with `name` frontmatter also work with
  `POST /api/v1/import`.
