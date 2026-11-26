# Worker profiles

A worker profile is a launchable execution-plane worker: image, harness, placement,
agents, sizing and auth. Platform workers are normally launched from a profile by the
in-cluster launcher; the API mints a scoped token per launch and revokes it on stop.

## Fields

| Field | Purpose |
|---|---|
| `harnessKind` | Harness the worker claims (`mock`, `opencode`, `pi`, …) |
| `image` / `imageTag` | Execution-plane image. Empty tag defaults to `ORBIT_IMAGE_TAG`, which the chart sets to the release version (never `latest` in a release install) |
| `target` / `executionDriver` | `local`, `docker`, `kubernetes` (default `docker`); `process` or `docker` (default `process`) |
| `agentScope` | `all` (dynamic: new agents included) or `selected` |
| `agentIds` | Allowlist when scope is `selected` |
| `skillSlugs`, `agentIds` | What the worker may run |
| `provider` / `model` | Optional model selection for the worker |
| `authSecretName` | Existing Secret with harness auth (OpenCode/Pi). Must be allowed by the installer (`launcher.authSecrets`, default `orbit-opencode-auth*,orbit-pi-auth*`). Binding a new Secret takes an Owner or Admin; other profile editors can keep, clear or reuse a Secret already bound to a profile they manage. See [Kubernetes launcher](../development/kubernetes-launcher.md) |
| `env` | Static env. Values are write-only: responses list names as `{ "NAME": { "hasValue": true } }` and never return a value |
| `envMappings` | Connection/secret → env mappings (inline). References only (`envVar` plus `connectionId`, `secretKey` or `secretRef`); other keys are rejected |
| `envMappingIds` | Reusable environment mappings shared across profiles |
| `resources` | CPU/memory requests + limits and workspace size |
| `allowWebSearch` / `allowWebFetch` | Allow OpenCode's built-in web search and web fetch tools. Both are off by default for headless runs |
| `allowGlab` / `allowGh` | Inject GitLab or GitHub CLI tokens; see [CLI tools](cli-tools.md) |
| `nativeToolPermissions` | OpenCode per-tool `allow`/`ask`/`deny` map; see [below](#native-opencode-tool-permissions) |
| `enabled` | Only enabled profiles launch |

## Updating `env`

Send `env` as a map of name to value on create and update:

- a string sets or replaces the value (an empty string stores an empty value);
- `null` keeps the stored value (a `null` for a name with no stored value is a 422 on
  `env.<NAME>`);
- a name you leave out is removed.

The Workers editor cannot add static variables: its Environment section only offers
mappings, and saving keeps the variables a profile already has. Set static values through
the API or an import.

Values are stored unencrypted in the `worker_profiles.env` column (they are not in the
encrypted secret store); the API only keeps them from being read back. Prefer an env
mapping to a secret or connection for real credentials.

## Resources

```json
{
  "requests": { "cpu": "100m", "memory": "256Mi" },
  "limits": { "cpu": "1", "memory": "1Gi" },
  "workspace": { "sizeLimit": "1Gi" }
}
```

Validated as Kubernetes quantities, request ≤ limit; defaults are applied when omitted.
The launcher sets container resources on the Deployment/Job and `--cpus/--memory` for the
Docker driver. **Worker CPU/memory belong here, not in the agent definition.**

## Environments

Reusable environment mappings are sets of environment variables for worker processes. Each
value comes from a connection's credential or a stored secret, so no value is typed in
plain text. Manage them under **Environments** in the Infrastructure group of the sidebar
(`/environments`; the old `/environment-mappings` address redirects there).

The list works like the other registries: search by name, variable or connection, a filter
**All / In use / Unused / Needs attention** with counts, which worker profiles use each one,
and row checkboxes for **Change access**. **Needs attention** means a variable cannot be
resolved because its connection was removed or has no credential. A run whose worker uses
such an environment fails before the harness starts, with the variable name in the error;
it never starts without the variable.

**New environment** opens Environment Studio, one section at a time: **Details** (name and
optional description), **Variables** and **Access control**. For each variable choose **Add
variable**, enter the **Variable name** (for example `GITLAB_TOKEN`) and a source: either
**Connection** (pick a connection; the name is suggested from it) or **Secret** (the key of
a stored secret). An environment needs at least one variable, and names must be unique.

Each environment has a page with its variables and sources, the worker profiles that use
it with **Attach to a worker**, **Edit**, and **Duplicate** and **Delete** under ⋯.

Attach a mapping to many worker profiles in the worker editor's Environment section, or to
an agent team ([agent teams](agent-teams.md)). Inline mappings on a profile override
attached mappings for the same env var. Changes to a mapping or to a profile require a
worker relaunch; the Workers UI asks whether to relaunch now or save only (running agents
may be interrupted). API: `GET/POST /api/v1/env-mappings`,
`PATCH/DELETE /api/v1/env-mappings/{id}`. Changing them needs `settings:write`.

## Workers page

**Workers** in the Infrastructure group has three parts:

- **Profiles**: one card per profile, with **Launch** (or **Relaunch**), a menu with
  "Bootstrap by hand", Export as YAML, enable or disable, and delete (a profile must be
  disabled first). **New profile** opens the worker editor; **Import** loads a profile from
  YAML. The editor has the sections Runtime, Model and auth, Agents and skills,
  Permissions, Environment, Resources and Access control, and asks before you leave with
  unsaved changes.
- **Launches**: worker instances started from a profile by the launcher, with an
  "Active" / "History" switch. Each launch shows its status and, when it failed, the
  error. **Stop** ends a running launch, **Launch again** starts a fresh one, and **Logs**
  shows its container log. A launch marked "Relaunch required" runs an older version of
  its profile.
- **Live workers**: workers that have reported a heartbeat, marked Live or Stale. A
  worker is stale when its heartbeat stops.

Launches that are old enough are archived automatically (see
[Executions](executions.md#statuses-and-retention)); the UI has no archive button, but
`POST /api/v1/worker-launches/{id}/archive` exists. Creating, changing, launching and
stopping profiles needs `settings:write`.

## Launch lifecycle

- Long-lived profile → a Kubernetes **Deployment** (recreated after drains/evictions)
- Single execution → a **Job** with TTL cleanup
- Stop deletes the workload; logs are read by the `orbit.launch-id` label
- A worker stopped by the runtime is surfaced as a failed launch with a structured reason:
  `worker_oom` (OOMKilled, with the memory limit), `worker_evicted` (node pressure or drain),
  `worker_exited` (non-zero exit), `worker_lost` (node/pod gone) or `worker_did_not_start`
  (image pull or container config error before the container ever ran; its waiting runs
  keep their retry). Exit code 137 alone is
  SIGKILL, not OOM. Every in-flight execution on that worker fails with the same reason,
  pod name, restart count, exit code and termination time; team runs are not retried
  automatically. The API polls live launches every 15 seconds, so this happens even when
  nobody has the Workers page open

The profile can also generate copy-paste Docker/Compose/Kubernetes bootstrap output with a
scoped token. Generating bootstrap output (an explicit **Generate** button in the dialog)
issues one new token for the profile; workers that are already running keep theirs.

## Tokens

Worker tokens are separate from user API tokens (`orbit_wk_`), scoped and revocable. CLI
deployments set `ORBIT_API_URL` + `ORBIT_WORKER_TOKEN`; workers never receive database,
NATS or master-key credentials.

## Team scoping

A worker profile has an owning access team (`ownerTeamId`, default **Default**). Claim is
restricted to queued executions whose agent is `org`-visible or owned by that team; a
profile can never claim another team's private agent. **Profile visibility is not claim
scope:** making the profile organization-visible does not let it claim agents from other
teams. Execution context/running/events/completion/cancel endpoints re-check the same rule,
so knowing an execution ID is not enough. Agent and skill references on create/update must
be visible to the caller.

If a run stays queued while its worker is online, check the agent's `visibility` and
`ownerTeamId` against the profile's owning team. Move the agent, change its visibility, add
it to a `selected` profile, or remove the profile's owning team (if that is safe).

## Native OpenCode tool permissions

OpenCode can enforce per-tool `allow | ask | deny` permissions. Set a JSON map on the
worker profile (`nativeToolPermissions`):

```json
{
  "bash": "deny",
  "shell": "allow",
  "edit": "ask",
  "read": "allow"
}
```

ORBIT writes this into `opencode.json` under `permission`. Tool names are OpenCode's
native tool names (`bash`, `edit`, `read`, `webfetch`, `websearch`, …); `shell` only
applies when the harness exposes that name. These permissions are enforced by OpenCode,
not by the ORBIT broker. Connector tools still use ORBIT grants/policy/approvals.
