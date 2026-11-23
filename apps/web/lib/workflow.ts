// Agent Team workflows: the graph a team with mode "workflow" runs.
//
// The API owns the rules (crates/orbit-domain/src/workflow.rs). This file holds what the
// editor needs before saving: the shape, friendly problem messages, a tidy layout and a
// few starting points. Keep the checks here in step with the server's; the server is
// still the one that decides.

export const WORKFLOW_START = "start";
export const DEFAULT_MAX_RUNS = 3;
export const MAX_RUNS_LIMIT = 10;
export const MAX_STEPS = 30;

export type WorkflowWhen = "always" | "success" | "failure" | "outcome";

export interface WorkflowModel {
  provider?: string | null;
  model?: string | null;
  management?: string | null;
}

export interface WorkflowNode {
  id: string;
  name: string;
  agentId: string;
  instructions?: string;
  outcomes?: string[];
  maxRuns?: number | null;
  model?: WorkflowModel | null;
  position?: { x: number; y: number } | null;
}

export interface WorkflowEdge {
  id: string;
  from: string;
  to: string;
  when?: WorkflowWhen;
  outcome?: string | null;
}

export interface Workflow {
  nodes: WorkflowNode[];
  edges: WorkflowEdge[];
}

/** How a step of a run went, for the run view. */
export interface WorkflowStepRun {
  stepIndex: number;
  nodeId?: string | null;
  status: string;
  attempt?: number;
  outcome?: string | null;
  activatedBy?: unknown;
}

const SLUG = /^[a-z0-9][a-z0-9_-]*$/;

export function isOutcomeLabel(value: string): boolean {
  return value.length <= 32 && SLUG.test(value);
}

/** "Run the tests!" -> "run-the-tests". Empty when nothing usable is left. */
export function slugify(value: string, max = 40): string {
  return value
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, max)
    .replace(/-+$/g, "");
}

/** A step id that is free in `workflow`, derived from `name` so exports read well. */
export function freeNodeId(workflow: Workflow, name: string): string {
  const base = slugify(name, 34) || "step";
  const taken = new Set([WORKFLOW_START, ...workflow.nodes.map((node) => node.id)]);
  if (!taken.has(base)) return base;
  for (let n = 2; ; n += 1) {
    if (!taken.has(`${base}-${n}`)) return `${base}-${n}`;
  }
}

export function freeEdgeId(workflow: Workflow): string {
  const taken = new Set(workflow.edges.map((edge) => edge.id));
  for (let n = workflow.edges.length + 1; ; n += 1) {
    if (!taken.has(`line-${n}`)) return `line-${n}`;
  }
}

export function freeStepName(workflow: Workflow, base = "Step"): string {
  const taken = new Set(workflow.nodes.map((node) => node.name.trim().toLowerCase()));
  for (let n = workflow.nodes.length + 1; ; n += 1) {
    const name = `${base} ${n}`;
    if (!taken.has(name.toLowerCase())) return name;
  }
}

export const isStartEdge = (edge: WorkflowEdge) => edge.from === WORKFLOW_START;

export function startsRun(workflow: Workflow, nodeId: string): boolean {
  return workflow.edges.some((edge) => isStartEdge(edge) && edge.to === nodeId);
}

export function edgesFrom(workflow: Workflow, nodeId: string): WorkflowEdge[] {
  return workflow.edges.filter((edge) => edge.from === nodeId);
}

/** What a line says, short enough for a label on the canvas. */
export function conditionLabel(edge: WorkflowEdge): string {
  if (isStartEdge(edge)) return "";
  switch (edge.when ?? "success") {
    case "always":
      return "always";
    case "failure":
      return "failed";
    case "outcome":
      return edge.outcome || "outcome?";
    default:
      return "done";
  }
}

/** The same, as a sentence fragment: "when it reports pass". */
export function conditionPhrase(edge: WorkflowEdge): string {
  switch (edge.when ?? "success") {
    case "always":
      return "whatever the result";
    case "failure":
      return "when it fails";
    case "outcome":
      return edge.outcome ? `when the outcome is ${edge.outcome}` : "when the outcome is…";
    default:
      return "when it succeeds";
  }
}

/** Steps reachable from each step by following one or more lines. */
function reachability(workflow: Workflow): Map<string, Set<string>> {
  const next = new Map<string, string[]>();
  for (const edge of workflow.edges) {
    if (isStartEdge(edge)) continue;
    next.set(edge.from, [...(next.get(edge.from) ?? []), edge.to]);
  }
  const reach = new Map<string, Set<string>>();
  for (const node of workflow.nodes) {
    const seen = new Set<string>();
    const stack = [...(next.get(node.id) ?? [])];
    while (stack.length) {
      const current = stack.pop() as string;
      if (seen.has(current)) continue;
      seen.add(current);
      stack.push(...(next.get(current) ?? []));
    }
    reach.set(node.id, seen);
  }
  return reach;
}

/** Steps that sit on a loop, so their run limit matters. */
export function loopSteps(workflow: Workflow): Set<string> {
  const reach = reachability(workflow);
  return new Set(workflow.nodes.filter((node) => reach.get(node.id)?.has(node.id)).map((node) => node.id));
}

export interface WorkflowIssue {
  message: string;
  /** The step or line to select so the user can fix it. */
  nodeId?: string;
  edgeId?: string;
}

/** Problems that would make the API refuse the workflow, in plain words. */
export function workflowIssues(workflow: Workflow): WorkflowIssue[] {
  const issues: WorkflowIssue[] = [];
  if (!workflow.nodes.length) return [{ message: "Add at least one step." }];
  if (workflow.nodes.length > MAX_STEPS) issues.push({ message: `A workflow can have at most ${MAX_STEPS} steps.` });
  const ids = new Set(workflow.nodes.map((node) => node.id));
  const names = new Map<string, number>();
  for (const node of workflow.nodes) {
    const name = node.name.trim();
    const label = name || "A step";
    if (!name) issues.push({ message: "Every step needs a name.", nodeId: node.id });
    else if (name.length > 60) issues.push({ message: `"${name.slice(0, 24)}…" is longer than 60 characters.`, nodeId: node.id });
    names.set(name.toLowerCase(), (names.get(name.toLowerCase()) ?? 0) + 1);
    if (!node.agentId) issues.push({ message: `${label} has no agent yet.`, nodeId: node.id });
    const outcomes = node.outcomes ?? [];
    if (outcomes.some((outcome) => !isOutcomeLabel(outcome)))
      issues.push({ message: `${label}: outcomes use lower-case letters, digits, "-" or "_".`, nodeId: node.id });
    if (new Set(outcomes).size !== outcomes.length)
      issues.push({ message: `${label} lists the same outcome twice.`, nodeId: node.id });
    if (outcomes.length > 8) issues.push({ message: `${label} has more than 8 outcomes.`, nodeId: node.id });
    if (node.maxRuns != null && !(node.maxRuns >= 1 && node.maxRuns <= MAX_RUNS_LIMIT))
      issues.push({ message: `${label}: the run limit is 1 to ${MAX_RUNS_LIMIT}.`, nodeId: node.id });
    // An outcome nobody follows is not an error, but an outcome line without a declared
    // outcome is.
  }
  for (const [name, count] of names) {
    if (name && count > 1) {
      const node = workflow.nodes.find((item) => item.name.trim().toLowerCase() === name);
      issues.push({ message: `Two steps are called "${node?.name.trim()}".`, nodeId: node?.id });
    }
  }
  const shapes = new Set<string>();
  for (const edge of workflow.edges) {
    if (!ids.has(edge.to) || (!isStartEdge(edge) && !ids.has(edge.from))) {
      issues.push({ message: "A line points at a step that no longer exists.", edgeId: edge.id });
      continue;
    }
    const source = workflow.nodes.find((node) => node.id === edge.from);
    if (!isStartEdge(edge) && edge.when === "outcome") {
      if (!edge.outcome)
        issues.push({ message: `Choose the outcome for the line from ${source?.name.trim() || "a step"}.`, edgeId: edge.id });
      else if (!(source?.outcomes ?? []).includes(edge.outcome))
        issues.push({
          message: `${source?.name.trim() || "A step"} has no outcome "${edge.outcome}".`,
          edgeId: edge.id,
        });
    }
    const shape = isStartEdge(edge)
      ? `start>${edge.to}`
      : `${edge.from}>${edge.to}:${edge.when ?? "success"}:${edge.when === "outcome" ? (edge.outcome ?? "") : ""}`;
    if (shapes.has(shape)) issues.push({ message: "The same line exists twice.", edgeId: edge.id });
    shapes.add(shape);
  }
  const entries = workflow.edges.filter(isStartEdge).map((edge) => edge.to);
  if (!entries.length) {
    issues.push({ message: "Choose where the run starts: connect Start to a step." });
  } else {
    const reach = reachability(workflow);
    const reachable = new Set(entries);
    for (const entry of entries) for (const id of reach.get(entry) ?? []) reachable.add(id);
    for (const node of workflow.nodes) {
      if (!reachable.has(node.id))
        issues.push({ message: `${node.name.trim() || "A step"} can never run: no line leads to it.`, nodeId: node.id });
    }
  }
  return issues;
}

// ---------------------------------------------------------------------------- layout

export const NODE_WIDTH = 204;
export const NODE_HEIGHT = 84;
const COLUMN_GAP = 76;
// Room for a card that grows a second row of chips, and for a label on the line between.
const ROW_GAP = 52;

/**
 * Place the steps left to right by how far they are from Start. Lines that point back
 * (loops) are ignored for the columns, and steps that only lead back sit below the main
 * line, so a loop reads as a detour that returns.
 */
export function layoutWorkflow(workflow: Workflow): Map<string, { x: number; y: number }> {
  const next = new Map<string, string[]>();
  for (const edge of workflow.edges) next.set(edge.from, [...(next.get(edge.from) ?? []), edge.to]);
  // Depth-first from Start; an edge to a step still on the stack points back.
  const column = new Map<string, number>();
  const visiting = new Set<string>();
  const forward = new Map<string, string[]>();
  const order: string[] = [];
  const visit = (id: string) => {
    visiting.add(id);
    for (const target of next.get(id) ?? []) {
      if (visiting.has(target)) continue;
      forward.set(id, [...(forward.get(id) ?? []), target]);
      if (!column.has(target)) {
        column.set(target, 0);
        visit(target);
      }
    }
    visiting.delete(id);
    order.unshift(id);
  };
  visit(WORKFLOW_START);
  // Longest path over the forward lines, in topological order.
  column.set(WORKFLOW_START, -1);
  for (const id of order) {
    for (const target of forward.get(id) ?? []) {
      column.set(target, Math.max(column.get(target) ?? 0, (column.get(id) ?? 0) + 1));
    }
  }
  // Steps nothing leads to go in a last column, so they stay visible.
  const last = Math.max(0, ...workflow.nodes.map((node) => column.get(node.id) ?? 0));
  const columnOf = (id: string) => column.get(id) ?? last + 1;
  // A step that only leads back (a fix that returns to its review) hangs below the main
  // line, so the way forward reads as one straight line and a loop as a detour under it.
  const returning = new Set<string>();
  for (const id of [...order].reverse()) {
    if (id === WORKFLOW_START || !(next.get(id) ?? []).length) continue;
    if ((forward.get(id) ?? []).every((target) => returning.has(target))) returning.add(id);
  }
  const columns = new Map<number, string[]>();
  for (const node of workflow.nodes) {
    if (returning.has(node.id)) continue;
    columns.set(columnOf(node.id), [...(columns.get(columnOf(node.id)) ?? []), node.id]);
  }
  const pitch = NODE_HEIGHT + ROW_GAP;
  const rows = new Map<string, number>([[WORKFLOW_START, 0]]);
  const sources = new Map<string, string[]>();
  for (const [from, targets] of forward) for (const to of targets) sources.set(to, [...(sources.get(to) ?? []), from]);
  // Left to right, each column ordered by where its steps come from: fewer crossing lines.
  const comesFrom = (id: string) => {
    const placed = (sources.get(id) ?? []).flatMap((from) => (rows.has(from) ? [rows.get(from) as number] : []));
    return placed.length ? placed.reduce((sum, y) => sum + y, 0) / placed.length : 0;
  };
  for (const at of [...columns.keys()].sort((a, b) => a - b)) {
    const ids = columns.get(at) ?? [];
    const from = new Map(ids.map((id) => [id, comesFrom(id)]));
    ids.sort((a, b) => (from.get(a) as number) - (from.get(b) as number));
    ids.forEach((id, row) => rows.set(id, (row - (ids.length - 1) / 2) * pitch));
  }
  // Returning steps go under everything between them and the step they return to.
  const lowest = (from: number, to: number) => {
    let y = -Infinity;
    for (const [id, row] of rows) {
      const at = id === WORKFLOW_START ? -1 : columnOf(id);
      if (at >= from && at <= to) y = Math.max(y, row);
    }
    return Number.isFinite(y) ? y : -pitch;
  };
  for (const node of workflow.nodes) {
    if (!returning.has(node.id)) continue;
    const at = columnOf(node.id);
    const back = Math.min(at, ...(next.get(node.id) ?? []).map(columnOf));
    rows.set(node.id, lowest(back, at) + pitch);
  }
  const positions = new Map<string, { x: number; y: number }>();
  for (const node of workflow.nodes) {
    positions.set(node.id, { x: columnOf(node.id) * (NODE_WIDTH + COLUMN_GAP), y: rows.get(node.id) ?? 0 });
  }
  return positions;
}

/** Every step with a position: its own, or one from the tidy layout. */
export function withPositions(workflow: Workflow): Workflow {
  if (workflow.nodes.every((node) => node.position)) return workflow;
  const layout = layoutWorkflow(workflow);
  return {
    ...workflow,
    nodes: workflow.nodes.map((node) => ({ ...node, position: node.position ?? layout.get(node.id) ?? { x: 0, y: 0 } })),
  };
}

export function tidy(workflow: Workflow): Workflow {
  const layout = layoutWorkflow(workflow);
  return { ...workflow, nodes: workflow.nodes.map((node) => ({ ...node, position: layout.get(node.id) ?? { x: 0, y: 0 } })) };
}

/** Where a new step goes: to the right of everything, level with Start. */
export function nextPosition(workflow: Workflow): { x: number; y: number } {
  const placed = workflow.nodes.flatMap((node) => (node.position ? [node.position] : []));
  if (!placed.length) return { x: 0, y: 0 };
  const right = placed.reduce((best, position) => (position.x > best.x ? position : best));
  const taken = new Set(placed.filter((position) => position.x === right.x).map((position) => position.y));
  let y = right.y;
  while (taken.has(y)) y += NODE_HEIGHT + ROW_GAP;
  return taken.size > 2 ? { x: right.x + NODE_WIDTH + COLUMN_GAP, y: 0 } : { x: right.x, y };
}

// ------------------------------------------------------------------------- editing

/** The only step nothing follows, if there is exactly one: where a line of steps continues. */
export function lastStep(workflow: Workflow): string | undefined {
  const leaves = workflow.nodes.filter((node) => !workflow.edges.some((edge) => edge.from === node.id));
  return leaves.length === 1 ? leaves[0].id : undefined;
}

/**
 * Add a step after `after` (connected with "when it succeeds"), or loose when `after` is
 * not given. The first step of a workflow starts the run.
 */
export function addStep(workflow: Workflow, after?: string): { workflow: Workflow; id: string } {
  const name = freeStepName(workflow);
  const id = freeNodeId(workflow, name);
  const anchor = workflow.nodes.find((node) => node.id === after)?.position;
  let position = anchor ? { x: anchor.x + NODE_WIDTH + COLUMN_GAP, y: anchor.y } : nextPosition(workflow);
  // Never drop a step on top of another one.
  const occupied = (spot: { x: number; y: number }) =>
    workflow.nodes.some(
      (node) => node.position && Math.abs(node.position.x - spot.x) < NODE_WIDTH && Math.abs(node.position.y - spot.y) < NODE_HEIGHT,
    );
  while (occupied(position)) position = { x: position.x, y: position.y + NODE_HEIGHT + ROW_GAP };
  const node: WorkflowNode = { id, name, agentId: "", position };
  const edges = [...workflow.edges];
  if (!workflow.nodes.length) edges.push({ id: "line-1", from: WORKFLOW_START, to: id, when: "always" });
  else if (after) {
    // A step with outcomes branches on them: the new line takes the first outcome that has
    // no line yet.
    const condition = suggestCondition(workflow, after);
    edges.push({ id: freeEdgeId(workflow), from: after, to: id, when: condition.when, outcome: condition.outcome ?? null });
  }
  return { workflow: { nodes: [...workflow.nodes, node], edges }, id };
}

export function removeStep(workflow: Workflow, id: string): Workflow {
  return {
    nodes: workflow.nodes.filter((node) => node.id !== id),
    edges: workflow.edges.filter((edge) => edge.from !== id && edge.to !== id),
  };
}

export function patchStep(workflow: Workflow, id: string, patch: Partial<WorkflowNode>): Workflow {
  let edges = workflow.edges;
  if (patch.outcomes) {
    // A line that follows an outcome the step no longer has would be refused; keep the
    // line and let the user pick again.
    const outcomes = patch.outcomes;
    edges = edges.map((edge) =>
      edge.from === id && edge.when === "outcome" && edge.outcome && !outcomes.includes(edge.outcome)
        ? { ...edge, outcome: null }
        : edge,
    );
  }
  return { nodes: workflow.nodes.map((node) => (node.id === id ? { ...node, ...patch } : node)), edges };
}

/** Connect two steps. Returns the workflow unchanged when that exact line exists. */
export function connect(
  workflow: Workflow,
  from: string,
  to: string,
  when: WorkflowWhen = "success",
  outcome?: string | null,
): { workflow: Workflow; id: string | null } {
  if (to === WORKFLOW_START || !workflow.nodes.some((node) => node.id === to)) return { workflow, id: null };
  const start = from === WORKFLOW_START;
  const existing = workflow.edges.find(
    (edge) =>
      edge.from === from &&
      edge.to === to &&
      (start || ((edge.when ?? "success") === when && (when !== "outcome" || (edge.outcome ?? null) === (outcome ?? null)))),
  );
  if (existing) return { workflow, id: existing.id };
  const id = freeEdgeId(workflow);
  const edge: WorkflowEdge = start
    ? { id, from, to, when: "always" }
    : { id, from, to, when, outcome: when === "outcome" ? (outcome ?? null) : null };
  return { workflow: { ...workflow, edges: [...workflow.edges, edge] }, id };
}

/**
 * The condition a new line from `from` most likely wants: the first outcome that has no
 * line yet when the step has outcomes, otherwise "succeeds".
 */
export function suggestCondition(workflow: Workflow, from: string): { when: WorkflowWhen; outcome?: string | null } {
  const node = workflow.nodes.find((item) => item.id === from);
  const outcomes = node?.outcomes ?? [];
  if (!outcomes.length) return { when: "success" };
  const used = new Set(edgesFrom(workflow, from).map((edge) => (edge.when === "outcome" ? edge.outcome : null)));
  return { when: "outcome", outcome: outcomes.find((outcome) => !used.has(outcome)) ?? outcomes[0] };
}

export function patchEdge(workflow: Workflow, id: string, patch: Partial<WorkflowEdge>): Workflow {
  return { ...workflow, edges: workflow.edges.map((edge) => (edge.id === id ? { ...edge, ...patch } : edge)) };
}

export function removeEdge(workflow: Workflow, id: string): Workflow {
  return { ...workflow, edges: workflow.edges.filter((edge) => edge.id !== id) };
}

export function setStartsRun(workflow: Workflow, nodeId: string, starts: boolean): Workflow {
  if (starts) return connect(workflow, WORKFLOW_START, nodeId).workflow;
  return { ...workflow, edges: workflow.edges.filter((edge) => !(isStartEdge(edge) && edge.to === nodeId)) };
}

/** What the API gets: trimmed, and without the fields that only carry defaults. */
export function cleanWorkflow(workflow: Workflow): Workflow {
  return {
    nodes: workflow.nodes.map((node) => {
      const provider = node.model?.provider?.trim() || null;
      const model = node.model?.model?.trim() || null;
      return {
        id: node.id,
        name: node.name.trim(),
        agentId: node.agentId,
        instructions: node.instructions?.trim() || undefined,
        outcomes: node.outcomes?.length ? node.outcomes : undefined,
        maxRuns: node.maxRuns ?? undefined,
        model: provider || model ? { ...node.model, provider, model } : undefined,
        position: node.position ? { x: Math.round(node.position.x), y: Math.round(node.position.y) } : undefined,
      };
    }),
    edges: workflow.edges.map((edge) => ({
      id: edge.id,
      from: edge.from,
      to: edge.to,
      when: isStartEdge(edge) ? "always" : (edge.when ?? "success"),
      outcome: !isStartEdge(edge) && edge.when === "outcome" ? (edge.outcome ?? undefined) : undefined,
    })),
  };
}

// ----------------------------------------------------------------------- starting points

/** The agents of a pipeline as a workflow: one step after another. */
export function workflowFromSteps(steps: Array<{ agentId: string; name?: string }>): Workflow {
  let workflow: Workflow = { nodes: [], edges: [] };
  let previous: string | undefined;
  for (const [index, step] of steps.entries()) {
    const name = step.name?.trim() || `Step ${index + 1}`;
    const unique = workflow.nodes.some((node) => node.name.toLowerCase() === name.toLowerCase()) ? `${name} ${index + 1}` : name;
    const id = freeNodeId(workflow, unique);
    workflow = {
      nodes: [...workflow.nodes, { id, name: unique, agentId: step.agentId }],
      edges: [
        ...workflow.edges,
        previous
          ? { id: `line-${index + 1}`, from: previous, to: id, when: "success" }
          : { id: "line-1", from: WORKFLOW_START, to: id, when: "always" },
      ],
    };
    previous = id;
  }
  return tidy(workflow);
}

export interface WorkflowExample {
  id: string;
  title: string;
  description: string;
  build: () => Workflow;
}

const step = (id: string, name: string, extra: Partial<WorkflowNode> = {}): WorkflowNode => ({ id, name, agentId: "", ...extra });
const line = (n: number, from: string, to: string, when: WorkflowWhen = "success", outcome?: string): WorkflowEdge => ({
  id: `line-${n}`,
  from,
  to,
  when,
  outcome: outcome ?? null,
});

export const WORKFLOW_EXAMPLES: WorkflowExample[] = [
  {
    id: "review-loop",
    title: "Build, review, fix",
    description: "A reviewer approves or sends the work back, up to three times.",
    build: () =>
      tidy({
        nodes: [
          step("build", "Build", { instructions: "Implement the task." }),
          step("review", "Review", {
            instructions: "Review the change. Report approved only when it is ready to merge.",
            outcomes: ["approved", "changes"],
            maxRuns: 3,
          }),
          step("fix", "Fix", { instructions: "Address the review comments you received." }),
          step("ship", "Ship", { instructions: "Open the pull request and summarise the change." }),
        ],
        edges: [
          line(1, WORKFLOW_START, "build", "always"),
          line(2, "build", "review"),
          line(3, "review", "ship", "outcome", "approved"),
          line(4, "review", "fix", "outcome", "changes"),
          line(5, "fix", "review"),
        ],
      }),
  },
  {
    id: "parallel",
    title: "Split the work, then combine",
    description: "Two agents work at the same time; a third waits for both.",
    build: () =>
      tidy({
        nodes: [
          step("plan", "Plan", { instructions: "Split the task into a frontend part and a backend part." }),
          step("frontend", "Frontend"),
          step("backend", "Backend"),
          step("integrate", "Integrate", { instructions: "Combine both parts and check they work together." }),
        ],
        edges: [
          line(1, WORKFLOW_START, "plan", "always"),
          line(2, "plan", "frontend"),
          line(3, "plan", "backend"),
          line(4, "frontend", "integrate"),
          line(5, "backend", "integrate"),
        ],
      }),
  },
  {
    id: "test-gate",
    title: "Test before deploy",
    description: "Deploy when the tests pass; otherwise fix and test again.",
    build: () =>
      tidy({
        nodes: [
          step("test", "Test", { instructions: "Run the test suite.", outcomes: ["pass", "fail"], maxRuns: 3 }),
          step("fix", "Fix", { instructions: "Fix the failing tests you were given." }),
          step("deploy", "Deploy"),
        ],
        edges: [
          line(1, WORKFLOW_START, "test", "always"),
          line(2, "test", "deploy", "outcome", "pass"),
          line(3, "test", "fix", "outcome", "fail"),
          line(4, "fix", "test"),
        ],
      }),
  },
];

// ------------------------------------------------------------------------------ runs

export interface NodeRunState {
  /** `stopped`: the run ended (cancelled or failed elsewhere) while this step was working. */
  status: "running" | "succeeded" | "failed" | "stopped";
  runs: number;
  outcome?: string | null;
  /** The latest step this workflow step produced. */
  stepIndex: number;
  /** The run failed because this step was due again after using all its runs. */
  limitReached?: boolean;
}

/** Per workflow step: how its latest run went and how often it ran. */
export function nodeRunStates(steps: WorkflowStepRun[]): Map<string, NodeRunState> {
  const states = new Map<string, NodeRunState>();
  for (const item of [...steps].sort((a, b) => a.stepIndex - b.stepIndex)) {
    if (!item.nodeId) continue;
    const status =
      item.status === "succeeded"
        ? "succeeded"
        : item.status === "running" || item.status === "pending"
          ? "running"
          : item.status === "cancelled"
            ? "stopped"
            : "failed";
    states.set(item.nodeId, {
      status,
      runs: (states.get(item.nodeId)?.runs ?? 0) + 1,
      outcome: item.outcome,
      stepIndex: item.stepIndex,
    });
  }
  return states;
}

/**
 * The steps a failed run stopped at: work arrived for them, but they had used all their
 * runs. The API names the step in the run's error; this finds it on the graph.
 */
export function limitReached(workflow: Workflow, steps: WorkflowStepRun[]): Set<string> {
  const used = new Set<string>();
  const runs = new Map<string, number>();
  for (const item of steps) {
    if (item.nodeId) runs.set(item.nodeId, (runs.get(item.nodeId) ?? 0) + 1);
    if (!Array.isArray(item.activatedBy)) continue;
    for (const token of item.activatedBy as Array<{ edge?: unknown; step?: unknown }>) {
      used.add(`${String(token?.edge)}:${String(token?.step ?? "start")}`);
    }
  }
  const stopped = new Set<string>();
  for (const edge of workflow.edges) {
    if (isStartEdge(edge)) continue;
    const limit = workflow.nodes.find((node) => node.id === edge.to)?.maxRuns ?? DEFAULT_MAX_RUNS;
    if ((runs.get(edge.to) ?? 0) < limit) continue;
    const waiting = steps.some((item) => {
      if (item.nodeId !== edge.from || used.has(`${edge.id}:${item.stepIndex}`)) return false;
      if (edge.when === "always") return item.status === "succeeded" || item.status === "failed";
      if (edge.when === "failure") return item.status === "failed";
      if (item.status !== "succeeded") return false;
      return edge.when === "success" || (Boolean(edge.outcome) && edge.outcome === item.outcome);
    });
    if (waiting) stopped.add(edge.to);
  }
  return stopped;
}

/** The lines that started the steps that are working right now. */
export function liveEdges(steps: WorkflowStepRun[]): Set<string> {
  return new Set(takenEdges(steps.filter((item) => item.status === "running" || item.status === "pending")).keys());
}

/** The lines a run followed, with how often. */
export function takenEdges(steps: WorkflowStepRun[]): Map<string, number> {
  const taken = new Map<string, number>();
  for (const item of steps) {
    if (!Array.isArray(item.activatedBy)) continue;
    for (const token of item.activatedBy as Array<{ edge?: unknown }>) {
      if (typeof token?.edge === "string") taken.set(token.edge, (taken.get(token.edge) ?? 0) + 1);
    }
  }
  return taken;
}
