# Agent Spawning for Large-Scale Simulations — Discussion

Status: proposal, not implemented. There is no batch execution endpoint, no `batchId`
on executions and no worker autoscaling manifest in the Helm chart. Creating one
execution per agent run (Option 1) works today.
Scope: Execution plane scalability for high-volume, low-complexity agent runs

## Problem

A customer wants to run 300 agents evaluating an article. The current architecture
creates one execution per agent run, each claimed by a worker that invokes a harness
subprocess. For 300 simple evaluation tasks, this is heavy: 300 execution records,
300 harness invocations, and potentially many worker pods.

## Current architecture constraints

- `orbit-api` never executes agent tasks (control plane only).
- `orbit-worker` claims executions and runs harnesses (execution plane).
- Each execution = one agent run with one harness invocation.
- Concurrency is controlled by `ORBIT_WORKER_CONCURRENCY` (default 4).
- The mock harness is available for testing (no LLM cost, instant results).

## Options

### Option 1: Many executions, scaled workers (recommended baseline)

Create 300 executions (one per agent) and let workers claim them:

```
for each agent:
  POST /api/v1/agents/{id}/executions { task: "evaluate article", payload: { article: "..." } }
```

Scale workers horizontally via Kubernetes:
- `kubectl scale deploy orbit-worker --replicas=10` (10 workers × 4 concurrent = 40 parallel)
- Mock harness: extremely fast, zero LLM cost — perfect for simulation testing
- Real LLM: use OpenCode/Pi workers with higher concurrency

**Pros:** Uses existing architecture, scales horizontally, tracked in executions table
**Cons:** 300 execution records, needs worker capacity, one API call per execution

### Option 2: Dedicated simulation worker (new feature)

A lightweight worker that:
- Skips harness invocation entirely
- Runs agent logic in-process (no subprocess, no MCP broker)
- Handles 100+ concurrent "evaluations" per instance
- Only works for read-only analysis (no tool calls, no writes)

**Pros:** Much faster, lower overhead, no container per execution
**Cons:** New code path, only works for simple evaluation tasks, bypasses the harness contract

### Option 3: Batch execution API (new feature)

A single API endpoint that creates N executions from one template:

```
POST /api/v1/executions/batch
{
  "agentId": "...",
  "count": 300,
  "task": "evaluate article",
  "payload": { "article": "..." },
  "variations": [{ "persona": "skeptic" }, { "persona": "optimist" }, ...]
}
```

**Pros:** One API call, tracked as a batch
**Cons:** Still needs workers to execute

## Recommendation

**Option 1 + Option 3 combined:**

1. Add a **batch execution endpoint** (Option 3) to create N executions from a template
2. Use the **mock harness** for pure simulations (no LLM cost, instant results)
3. For real LLM evaluation, scale workers horizontally (K8s handles this natively)
4. Consider a **simulation mode** flag on executions that skips the harness and runs a
   lightweight in-process evaluator

The mock harness is actually perfect for this use case — it's designed for testing and
can handle hundreds of concurrent executions with zero LLM cost. You'd just need to
scale the worker's concurrency or add more worker pods.

## What would need to be built

| Feature | Effort | Priority |
|---|---|---|
| Batch execution API | Medium | High |
| Worker concurrency tuning (higher limits) | Low | High |
| Simulation mode (skip harness, in-process eval) | High | Medium |
| Horizontal worker scaling (K8s HPA) | Low (config) | Medium |

## Implementation steps (if approved)

1. Add `POST /api/v1/executions/batch` endpoint to create N executions from a template
2. Add `batchId` field to `ExecutionRecord` for grouping
3. Add `GET /api/v1/executions?batchId=...` filter
4. Add HPA manifest to Helm chart for worker scaling
5. Document the simulation workflow

## Customer bringing their own worker

A customer can build a custom worker image with any harness CLI. See
[provider-ux-and-harness-decoupling.md](provider-ux-and-harness-decoupling.md) for the custom worker guide.
The worker is token-only — no database access needed. Multiple workers compete for
executions via the claim mechanism, so scaling is just adding more pods.
