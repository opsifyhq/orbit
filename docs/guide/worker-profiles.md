# Worker profiles

A worker profile is a launchable execution-plane worker: image, harness, placement,
agents, sizing and auth. Platform workers are normally launched from a profile by the
in-cluster launcher; the API mints a scoped token per launch and revokes it on stop.

## Fields

| Field | Purpose |
|---|---|
| `harnessKind` | Harness the worker claims (`mock`, `opencode`, `pi`, …) |
| `image` / `imageTag` | Execution-plane image. Empty tag defaults to `ORBIT_IMAGE_TAG`, which the chart sets to the release version (never `latest` in a release install) |
| `target` / `executionDriver` | `local`, `docker`, `kubernetes`; `process` or `docker` |
| `agentScope` | `all` (dynamic: new agents included) or `selected` |
| `agentIds` | Allowlist when scope is `selected` |
| `skillSlugs`, `agentIds` | What the worker may run |
| `provider` / `model` | Optional model selection for the worker |
| `authSecretName` | Existing Secret with harness auth (OpenCode/Pi). Must be allowed by the installer (`launcher.authSecrets`, default `orbit-opencode-auth*,orbit-pi-auth*`); see [Kubernetes launcher](../development/kubernetes-launcher.md) |
| `env` | Static env. Values are write-only: responses list names as `{ "NAME": { "hasValue": true } }` and never return a value |
| `envMappings` | Connection/secret → env mappings (inline). References only (`envVar` plus `connectionId`, `secretKey` or `secretRef`); other keys are rejected |
| `envMappingIds` | Reusable environment mappings shared across profiles |
| `resources` | CPU/memory requests + limits and workspace size |
| `enabled` | Only enabled profiles launch |

## Updating `env`

Send `env` as a map of name to value on create and update:

- a string sets or replaces the value (an empty string stores an empty value);
- `null` keeps the stored value (a `null` for a name with no stored value is a 422 on
  `env.<NAME>`);
- a name you leave out is removed.

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

## Reusable environment mappings

Environment mappings can be created once under **Library → Environments** and
attached to many worker profiles. Inline mappings override attached mappings for the same
env var. Changes to a mapping or to a profile require a worker relaunch; the Workers UI
asks whether to relaunch now or save only (running agents may be interrupted).

## Launch lifecycle

- Long-lived profile → a Kubernetes **Deployment** (recreated after drains/evictions)
- Single execution → a **Job** with TTL cleanup
- Stop deletes the workload; logs are read by the `orbit.launch-id` label
- OOMKilled/exit 137 is surfaced as a failed launch with the memory limit, and running
  executions on that worker fail with a `worker_oom` event

The profile can also generate copy-paste Docker/Compose/Kubernetes bootstrap output with a
scoped token. Generating bootstrap output rotates the profile token.

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
