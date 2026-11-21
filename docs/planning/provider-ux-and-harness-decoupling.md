# Provider UX and Harness Decoupling — Plan

Status: partly shipped. Shipped: the harness can be chosen per execution and per team run
(`harness`, with `harnessOverride` as a legacy alias), the Work wizard passes it, and the
Agent Studio model field offers one provider list that sets the management mode. Not
shipped as planned: the plan below stores the harness on the agent, but
[ADR 0012](../decisions/0012-decouple-agent-and-harness.md) decouples it, and the
`spec.harness` field is deprecated. Not checked against the UI: the "no providers" hint
wording. The custom worker guide at the end matches the worker token API and environment
variables.
Scope: Agent Studio provider selection and execution harness selection

## Problem 1: Provider management mode confusion

*Shipped: the Agent Studio form lists all providers in one control (see
`apps/web/components/agents/agent-form.tsx`). The grouping by kind is not verified.*

### Current state

- "Harness managed" model management uses `harness_managed` providers (opencode-go, etc.).
  Authentication is handled by the worker/harness (e.g. `opencode auth export`).
- "Platform" model management uses `openai_compatible` providers (vLLM, OpenAI, etc.).
  Authentication is handled by ORBIT (API key stored encrypted, injected into harness config).

### User confusion

Users create a provider (e.g. an opencode provider with an API key) and expect to
use it in "platform" mode. But the provider is `harness_managed`, so it does not
appear in the platform dropdown. The message "No OpenAI-compatible providers
configured yet" is confusing when harness-managed providers exist.

### Plan options

| Option | Description | Effort |
|---|---|---|
| A. Keep two modes, improve labels | Rename to "Harness-authenticated" vs "ORBIT-managed", add explanatory text | Low |
| B. Unified provider dropdown | Show all providers in one dropdown, auto-set management mode based on kind | Medium |
| C. Allow opencode as platform provider | Create an `openai_compatible` provider with opencode's base URL + API key | Already supported — needs documentation |

### Recommendation

Option B + C. Show all providers in a single dropdown grouped by kind. Selecting a
provider auto-sets the management mode. Add a note explaining that
"harness-managed" means the worker handles auth.

### Implementation steps

1. Modify `AgentForm` to fetch all providers (not just openai_compatible).
2. Replace the two separate inputs (dropdown for platform, text for harness) with a
   single dropdown grouped by kind.
3. On selection, set `model.management` and `model.provider` based on the provider's kind.
4. Update the "no providers" hint to guide users to the Providers page (the page is titled "Providers & models", ADR 0008).
5. Document that opencode can be used as a platform provider by creating an
   `openai_compatible` provider with opencode's base URL and API key.

---

## Problem 2: Harness coupled to agent

*Shipped, and superseded: option B was implemented, then ADR 0012 removed the harness from
the agent. The "Current state" below describes the code before that change.*

### Current state

The agent stores `spec.harness.kind`. When you run an agent, it uses that harness.
The Work wizard lets you select a harness, but it only filters agents — it does not
override the agent's stored harness.

### User concern

If you select "opencode" in Work but pick an agent configured for "mock", the mock
harness is used instead of opencode.

### Plan options

| Option | Description | Effort |
|---|---|---|
| A. Filter agents by harness (done) | Already implemented — only matching agents show | Done |
| B. Override harness at execution time | Pass harness kind through the execution API, override agent's stored harness | Medium |
| C. Decouple harness from agent | Agent = instructions + model + connectors; harness = execution environment chosen at run time | High — touches domain model, API, worker, UI |

### Recommendation

Option B. Add an optional `harnessOverride` field to execution and team-run requests.
When set, the worker uses that harness instead of the agent's stored harness. This
keeps the agent's default harness for normal runs while allowing Work to override.

### Implementation steps

1. Add `harness_override: Option<HarnessKind>` to `CreateExecutionRequest` and
   `CreateTeamRunRequest`.
2. In `execute_agent` handler, if `harness_override` is set, use it instead of
   `spec.harness.kind` when creating the execution.
3. In `start_step` (team runs), pass `harness_override` through the payload.
4. In the worker, use `execution.harness_override` if present, else `spec.harness.kind`.
5. In the Work wizard, pass the selected harness as `harnessOverride`.
6. Update the UI to show which harness will be used.

---

## Proposed implementation order

1. **Provider dropdown unification** (Option B for Problem 1) — ~1 hour
2. **Harness override** (Option B for Problem 2) — ~2 hours
3. **UI polish** — clearer labels, grouping, help text — ~1 hour

---

## Customer bringing their own worker with OpenCode harness and auth

A customer can build and run their own worker image with OpenCode. The architecture
is designed for this.

### What the customer needs

1. **Build a worker image** using the minimal Dockerfile + OpenCode CLI:

```dockerfile
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates git curl \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 orbit \
    && useradd --uid 10001 --gid 10001 --no-create-home --shell /usr/sbin/nologin orbit
COPY orbit-worker /usr/local/bin/orbit-worker
RUN chmod 0755 /usr/local/bin/orbit-worker
RUN curl -fsSL https://github.com/anomalyco/opencode/releases/download/v2.3.0/opencode-linux-x64.tar.gz \
    | tar -xz -C /usr/local/bin opencode && chmod 0755 /usr/local/bin/opencode
USER 10001:10001
ENV ORBIT_WORKSPACE_ROOT=/tmp/orbit-workspaces HOME=/home/orbit
ENTRYPOINT ["/usr/local/bin/orbit-worker"]
```

2. **Issue a worker token** from ORBIT:

```bash
curl -X POST http://orbit-api:8080/api/v1/worker-tokens \
  -H "Authorization: Bearer <api-token>" \
  -d '{"name":"customer-opencode","harnesses":["opencode"]}'
```

3. **Mount OpenCode auth** (the customer's own credentials):

```bash
opencode auth export > auth-export.json
# Mount as a Kubernetes Secret or Docker volume
```

4. **Run the worker** with environment variables:

```
ORBIT_API_URL=https://orbit-api.example.com
ORBIT_WORKER_TOKEN=orbit_wk_...
ORBIT_WORKER_HARNESSES=opencode
ORBIT_WORKER_TARGET=kubernetes
ORBIT_WORKSPACE_ROOT=/tmp/orbit-workspaces
```

### Key points

- The worker is **token-only** — no database access, no master key needed.
- Auth credentials are **mounted externally** — never baked into the image.
- The worker **self-registers** via heartbeat — appears in `GET /api/v1/workers`.
- Multiple workers can run simultaneously — they compete for executions via the claim mechanism.
- The customer's worker can use **their own OpenCode auth** (their own API keys, their own provider config).
- ORBIT doesn't need to know about the customer's providers — OpenCode manages them via auth export.

### What ORBIT provides vs what the customer provides

| ORBIT provides | Customer provides |
|---|---|
| `orbit-worker` binary | Worker image (with harness CLI) |
| Worker token (scoped to harnesses) | Harness auth (opencode auth export) |
| Execution queue + scheduling | Compute (K8s cluster, Docker, etc.) |
| Tool broker (MCP) | Network access to ORBIT API |
| Connector credentials (encrypted) | Their own provider API keys (via harness auth) |
