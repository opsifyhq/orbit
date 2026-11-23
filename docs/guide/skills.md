# Skills

A skill is a reusable `SKILL.md` package stored in the database and materialized into a
worker workspace for an execution.

## Shape

- `slug` (unique per org), `name`, `description`, markdown `content`
- `files`: path → text, including optional `scripts/*`. At most 50 files; each path is
  relative, has no `..` or backslash and is at most 200 characters
- Monotonic `version`

## Materialization

At execution start the worker copies assigned skills (agent-level, execution-level and
worker-profile skills) into the workspace, makes validated script files executable, and
references them in the prompt. Script allowlisting rejects unsupported extensions and
path traversal. Skills never run with control-plane credentials.

A file counts as a script when its path starts with `scripts/` or contains `/scripts/`.
Scripts must end in `.sh`, `.bash`, `.py`, `.js`, `.ts`, `.rb`, `.pl` or `.go`, be at most
100 KB each, and total at most 1 MB per skill.

## Assignment

Attach skills to an agent (Agent Studio, "Connectors and skills"), an agent team, an
execution request (`skills` in `POST /agents/{id}/executions`) or a worker profile
("Agents and skills" in the worker editor). [Start work](start-work.md) does not offer a
skill choice. Skill content is trusted configuration; task input remains untrusted and is
kept separate from instructions.

## Screens

- **Skills** lists skills with search and an "In use" / "Unused" filter, selection with
  bulk **Delete** and a bulk access change, **Import** and **New skill**. Skills cannot be
  archived.
- A skill page (`/skills/<id>`) shows Instructions as formatted Markdown, Bundled files,
  "Used by" (agents, teams and worker profiles that use the skill) and Details. It warns
  when the stored SKILL.md does not match the name, slug and description.
- Skill Studio (`/skills/new`, `/skills/<id>/edit`) has the sections Identity (Name, Slug,
  Description), Instructions (with a Markdown preview and "Other frontmatter" for extra
  YAML keys), Bundled files and Access control. Name, slug and description are written
  into the SKILL.md frontmatter when you save. **Upload files** adds text files only. It
  asks before you leave with unsaved changes.

## API

`GET/POST /api/v1/skills`, `GET/PATCH/DELETE /api/v1/skills/{id}`,
`POST /api/v1/skills/bulk/delete`. Creating and changing skills needs `agents:write`.

A skill that an agent, an agent team or a worker profile still uses cannot be deleted
(409, naming up to five users): agents refer to skills by slug, so a freed slug could be
registered again by someone else. Remove the skill from those first. When a run starts,
the user who requested it (for a background run, the agent's owner) must be able to use
every skill the agent or the run names.
