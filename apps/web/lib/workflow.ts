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
export const MAX_LINES = 120;
export const MAX_INSTRUCTIONS = 4000;

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
  if (workflow.edges.length > MAX_LINES) issues.push({ message: `A workflow can have at most ${MAX_LINES} lines.` });
  const stepIds = new Set<string>();
  for (const node of workflow.nodes) {
    if (node.id === WORKFLOW_START || node.id.length > 40 || !SLUG.test(node.id))
      issues.push({ message: `${node.name.trim() || "A step"} has an id the server does not accept.`, nodeId: node.id });
    else if (stepIds.has(node.id)) issues.push({ message: `Two steps share the id "${node.id}".`, nodeId: node.id });
    stepIds.add(node.id);
  }
  const lineIds = new Set<string>();
  for (const edge of workflow.edges) {
    if (!edge.id.trim() || lineIds.has(edge.id)) issues.push({ message: "Every line needs its own id.", edgeId: edge.id });
    lineIds.add(edge.id);
  }
  const ids = new Set(workflow.nodes.map((node) => node.id));
  const names = new Map<string, number>();
  for (const node of workflow.nodes) {
    const name = node.name.trim();
    const label = name || "A step";
    if (!name) issues.push({ message: "Every step needs a name.", nodeId: node.id });
    else if (name.length > 60) issues.push({ message: `"${name.slice(0, 24)}…" is longer than 60 characters.`, nodeId: node.id });
    names.set(name.toLowerCase(), (names.get(name.toLowerCase()) ?? 0) + 1);
    if (!node.agentId) issues.push({ message: `${label} has no agent yet.`, nodeId: node.id });
    if ((node.instructions ?? "").length > MAX_INSTRUCTIONS)
      issues.push({ message: `${label}: instructions are longer than ${MAX_INSTRUCTIONS} characters.`, nodeId: node.id });
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
    if (!isStartEdge(edge) && edge.when !== "outcome" && edge.outcome)
      issues.push({
        message: `The line from ${source?.name.trim() || "a step"} names an outcome but does not follow one.`,
        edgeId: edge.id,
      });
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

// --------------------------------------------------------------------------- outputs

/** One way out of a step: a result a line can follow. */
export interface WorkflowPort {
  /** "success", "failure", "always" or "outcome:<label>"; the handle id on the canvas. */
  id: string;
  when: WorkflowWhen;
  outcome?: string | null;
  label: string;
  /** Place among the step's outcomes, which picks its colour. */
  index?: number;
}

/** The output a line leaves its step from. */
export function portOf(edge: Pick<WorkflowEdge, "when" | "outcome">): string {
  const when = edge.when ?? "success";
  return when === "outcome" ? `outcome:${edge.outcome ?? ""}` : when;
}

/** The condition a line gets when it is drawn from the output `port`. */
export function conditionOfPort(port: string | null | undefined): { when: WorkflowWhen; outcome: string | null } {
  if (port?.startsWith("outcome:")) return { when: "outcome", outcome: port.slice(8) || null };
  return { when: port === "failure" || port === "always" ? port : "success", outcome: null };
}

/**
 * The outputs of a step, top to bottom: its outcomes, then "done", "failed" and "always".
 * Without `all` only the ones that matter are listed: the outcomes, the ones a line uses,
 * and "done" for a step that has no outcomes.
 */
export function stepPorts(workflow: Workflow, node: WorkflowNode, all = false): WorkflowPort[] {
  const used = new Set(edgesFrom(workflow, node.id).map(portOf));
  const outcomes = node.outcomes ?? [];
  const ports: WorkflowPort[] = outcomes.map((outcome, index) => ({
    id: `outcome:${outcome}`,
    when: "outcome",
    outcome,
    label: outcome,
    index,
  }));
  // A line that follows an outcome the step does not have (yet) still needs a place to leave from.
  for (const id of used) {
    if (!id.startsWith("outcome:") || ports.some((port) => port.id === id)) continue;
    ports.push({ id, when: "outcome", outcome: id.slice(8) || null, label: id.slice(8) || "outcome?", index: ports.length });
  }
  if (all || !outcomes.length || used.has("success")) ports.push({ id: "success", when: "success", label: "done" });
  if (all || used.has("failure")) ports.push({ id: "failure", when: "failure", label: "failed" });
  if (all || used.has("always")) ports.push({ id: "always", when: "always", label: "always" });
  return ports;
}

/** A step whose only output is "done" shows no list of outputs: its card stays small. */
export function isPlain(ports: WorkflowPort[]): boolean {
  return ports.length === 1 && ports[0].id === "success";
}

// ---------------------------------------------------------------------------- layout

export const NODE_WIDTH = 196;
/** The top of a step card: its name and agent. Lines arrive at the middle of it. */
export const NODE_HEIGHT = 60;
export const HANDLE_Y = NODE_HEIGHT / 2;
/** One output in the list under the name. */
export const PORT_ROW = 22;
const PORTS_PAD = 9;
const COLUMN_GAP = 64;
const ROW_GAP = 40;

/** Rows of outputs a card shows for `ports`. */
export const portRows = (ports: WorkflowPort[]) => (isPlain(ports) ? 0 : ports.length);

export const stepHeight = (rows: number) => NODE_HEIGHT + (rows ? rows * PORT_ROW + PORTS_PAD : 0);

/** Where output `index` leaves a card with `rows` rows, measured from the card's top. */
export const portY = (rows: number, index: number) =>
  rows ? NODE_HEIGHT + 5 + Math.max(0, index) * PORT_ROW + PORT_ROW / 2 : HANDLE_Y;

/** How tall the card of `node` is drawn. */
export function nodeHeight(workflow: Workflow, node: WorkflowNode): number {
  return stepHeight(portRows(stepPorts(workflow, node)));
}

/**
 * Place the steps left to right by how far they are from Start. Lines that point back
 * (loops) are ignored for the columns, and steps that only lead back sit below the main
 * line, so a loop reads as a detour that returns. Within a column the steps are ordered
 * to keep lines from crossing, and each step sits level with the steps it follows.
 */
export function layoutWorkflow(workflow: Workflow): Map<string, { x: number; y: number }> {
  const next = new Map<string, string[]>();
  for (const edge of workflow.edges) {
    // A step that repeats itself does not move anything.
    if (edge.from !== edge.to) next.set(edge.from, [...(next.get(edge.from) ?? []), edge.to]);
  }
  // How many lines away from Start each step is, by the shortest way.
  const distance = new Map<string, number>([[WORKFLOW_START, 0]]);
  for (const queue = [WORKFLOW_START]; queue.length; ) {
    const id = queue.shift() as string;
    for (const target of next.get(id) ?? []) {
      if (distance.has(target)) continue;
      distance.set(target, (distance.get(id) as number) + 1);
      queue.push(target);
    }
  }
  const reach = reachability(workflow);
  // Depth-first from Start; an edge to a step still on the stack points back. So does a
  // line on a loop that leads to a step nearer to Start: of the two lines between
  // "integrate" and "backend", the one that returns is the one towards Start.
  const column = new Map<string, number>();
  const visiting = new Set<string>();
  const forward = new Map<string, string[]>();
  const order: string[] = [];
  const visit = (id: string) => {
    visiting.add(id);
    for (const target of next.get(id) ?? []) {
      if (visiting.has(target)) continue;
      if (
        !column.has(target) &&
        (distance.get(target) ?? Infinity) < (distance.get(id) ?? Infinity) &&
        reach.get(target)?.has(id)
      )
        continue;
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
  // Steps that Start does not reach (loose ones, while a workflow is being built) are
  // laid out the same way, from the ones nothing loose leads to.
  const loose = () => workflow.nodes.filter((node) => !column.has(node.id));
  for (const node of loose()) {
    const led = workflow.edges.some((edge) => edge.to === node.id && edge.from !== node.id && !column.has(edge.from));
    if (led || column.has(node.id)) continue;
    column.set(node.id, 0);
    visit(node.id);
  }
  // What is left only leads round in circles.
  for (const node of loose()) {
    if (column.has(node.id)) continue;
    column.set(node.id, 0);
    visit(node.id);
  }
  // Longest path over the forward lines, in topological order.
  column.set(WORKFLOW_START, -1);
  for (const id of order) {
    for (const target of forward.get(id) ?? []) {
      column.set(target, Math.max(column.get(target) ?? 0, (column.get(id) ?? 0) + 1));
    }
  }
  const columnOf = (id: string) => column.get(id) ?? 0;
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
  const keys = [...columns.keys()].sort((a, b) => a - b);

  const ports = new Map(workflow.nodes.map((node) => [node.id, stepPorts(workflow, node)]));
  const heightOf = (id: string) => stepHeight(portRows(ports.get(id) ?? []));
  /** How far down its step a line leaves, as a share of one row: orders the targets of one step. */
  const leaves = (from: string, to: string) => {
    const list = ports.get(from) ?? [];
    const at = workflow.edges
      .filter((edge) => edge.from === from && edge.to === to)
      .map((edge) => list.findIndex((port) => port.id === portOf(edge)));
    return at.length ? Math.max(0, Math.min(...at)) / 100 : 0;
  };
  const before = new Map<string, string[]>();
  const after = new Map<string, string[]>();
  for (const [from, targets] of forward) {
    for (const to of targets) {
      if (returning.has(to) || returning.has(from)) continue;
      before.set(to, [...(before.get(to) ?? []), from]);
      after.set(from, [...(after.get(from) ?? []), to]);
    }
  }
  // Order each column by where its lines come from, then by where they go, a few times
  // over: fewer crossing lines.
  const place = new Map<string, number>([[WORKFLOW_START, 0]]);
  for (const at of keys) (columns.get(at) ?? []).forEach((id, index) => place.set(id, index));
  for (let pass = 0; pass < 6; pass += 1) {
    const down = pass % 2 === 0;
    for (const at of down ? keys : [...keys].reverse()) {
      const ids = columns.get(at) ?? [];
      const weight = new Map(
        ids.map((id) => {
          const around = (down ? before.get(id) : after.get(id)) ?? [];
          if (!around.length) return [id, place.get(id) ?? 0] as const;
          const sum = around.reduce(
            (total, other) => total + (place.get(other) ?? 0) + (down ? leaves(other, id) : 0),
            0,
          );
          return [id, sum / around.length] as const;
        }),
      );
      const sorted = [...ids].sort(
        (a, b) => (weight.get(a) as number) - (weight.get(b) as number) || (place.get(a) ?? 0) - (place.get(b) ?? 0),
      );
      columns.set(at, sorted);
      sorted.forEach((id, index) => place.set(id, index));
    }
  }
  // Each step level with the steps it follows; steps that would overlap are spread out
  // around that spot.
  const tops = new Map<string, number>([[WORKFLOW_START, 0]]);
  for (const at of keys) {
    const ids = columns.get(at) ?? [];
    const wanted = ids.map((id) => {
      const from = (before.get(id) ?? []).flatMap((other) => (tops.has(other) ? [tops.get(other) as number] : []));
      return from.length ? from.reduce((sum, y) => sum + y, 0) / from.length : 0;
    });
    let cursor = -Infinity;
    const placed = ids.map((id, index) => {
      const y = Math.max(wanted[index], cursor);
      cursor = y + heightOf(id) + ROW_GAP;
      return y;
    });
    const pushed = placed.reduce((sum, y, index) => sum + y - wanted[index], 0) / Math.max(1, ids.length);
    ids.forEach((id, index) => tops.set(id, Math.round(placed[index] - pushed)));
  }
  // Returning steps go under everything between them and the step they return to.
  const lowest = (from: number, to: number) => {
    let y = -Infinity;
    for (const [id, top] of tops) {
      const at = id === WORKFLOW_START ? -1 : columnOf(id);
      if (at >= from && at <= to) y = Math.max(y, top + (id === WORKFLOW_START ? NODE_HEIGHT : heightOf(id)));
    }
    return Number.isFinite(y) ? y : 0;
  };
  for (const node of workflow.nodes) {
    if (!returning.has(node.id)) continue;
    const at = columnOf(node.id);
    const back = Math.min(at, ...(next.get(node.id) ?? []).map(columnOf));
    tops.set(node.id, lowest(back, at) + ROW_GAP + 8);
  }
  const positions = new Map<string, { x: number; y: number }>();
  for (const node of workflow.nodes) {
    positions.set(node.id, { x: columnOf(node.id) * (NODE_WIDTH + COLUMN_GAP), y: tops.get(node.id) ?? 0 });
  }
  return positions;
}

/** No step has a place of its own: the workflow is arranged for the user after every change. */
export function isArranged(workflow: Workflow): boolean {
  return workflow.nodes.every((node) => !node.position);
}

/**
 * Steps the user placed, moved apart where their cards would lie on top of each other:
 * the lower one goes down. A card grows with its outputs, so two steps that were placed
 * close together can meet later.
 */
export function apart(workflow: Workflow): Workflow {
  const boxes = workflow.nodes
    .flatMap((node) => (node.position ? [{ id: node.id, x: node.position.x, y: node.position.y, h: nodeHeight(workflow, node) }] : []))
    .sort((a, b) => a.y - b.y || a.x - b.x);
  const moved = new Map<string, number>();
  for (let index = 1; index < boxes.length; index += 1) {
    const box = boxes[index];
    for (let again = true; again; ) {
      again = false;
      for (const other of boxes) {
        if (other === box || Math.abs(other.x - box.x) >= NODE_WIDTH + 8) continue;
        // Only the upper of two cards stays; `other` is upper when it starts higher, or
        // was settled earlier at the same height.
        const upper = other.y < box.y || (other.y === box.y && boxes.indexOf(other) < index);
        if (!upper || box.y >= other.y + other.h + 4) continue;
        box.y = other.y + other.h + 16;
        moved.set(box.id, box.y);
        again = true;
      }
    }
  }
  if (!moved.size) return workflow;
  return {
    ...workflow,
    nodes: workflow.nodes.map((node) =>
      moved.has(node.id) && node.position ? { ...node, position: { x: node.position.x, y: moved.get(node.id) as number } } : node,
    ),
  };
}

/** Every step with a position: its own, or one from the tidy layout. */
export function withPositions(workflow: Workflow): Workflow {
  if (isArranged(workflow)) {
    const layout = layoutWorkflow(workflow);
    return { ...workflow, nodes: workflow.nodes.map((node) => ({ ...node, position: layout.get(node.id) ?? { x: 0, y: 0 } })) };
  }
  if (workflow.nodes.every((node) => node.position)) return apart(workflow);
  const layout = layoutWorkflow(workflow);
  return apart({
    ...workflow,
    nodes: workflow.nodes.map((node) => ({ ...node, position: node.position ?? layout.get(node.id) ?? { x: 0, y: 0 } })),
  });
}

/** Hand the arrangement back: forget where the steps were dragged to. */
export function tidy(workflow: Workflow): Workflow {
  return { ...workflow, nodes: workflow.nodes.map((node) => ({ ...node, position: null })) };
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
 * Add a step after `after` (from its output `port`, or the first one without a line), or
 * loose when `after` is not given. The first step of a workflow starts the run. A
 * workflow that is arranged for the user stays that way unless the step is dropped `at`
 * a place.
 */
export function addStep(
  workflow: Workflow,
  after?: string,
  options: { port?: string; at?: { x: number; y: number } } = {},
): { workflow: Workflow; id: string } {
  const name = freeStepName(workflow);
  const id = freeNodeId(workflow, name);
  let base = workflow;
  let position: { x: number; y: number } | null = null;
  if (options.at) {
    base = withPositions(workflow);
    position = options.at;
  } else if (!isArranged(workflow)) {
    base = withPositions(workflow);
    const anchor = base.nodes.find((node) => node.id === after)?.position;
    let spot = anchor ? { x: anchor.x + NODE_WIDTH + COLUMN_GAP, y: anchor.y } : nextPosition(base);
    // Never drop a step on top of another one.
    const occupied = (at: { x: number; y: number }) =>
      base.nodes.some(
        (node) =>
          node.position &&
          Math.abs(node.position.x - at.x) < NODE_WIDTH &&
          at.y < node.position.y + nodeHeight(base, node) + 8 &&
          at.y + NODE_HEIGHT + 8 > node.position.y,
      );
    while (occupied(spot)) spot = { x: spot.x, y: spot.y + NODE_HEIGHT + ROW_GAP };
    position = spot;
  }
  const node: WorkflowNode = { id, name, agentId: "", position };
  const edges = [...base.edges];
  if (!base.nodes.length) edges.push({ id: "line-1", from: WORKFLOW_START, to: id, when: "always" });
  else if (after === WORKFLOW_START) edges.push({ id: freeEdgeId(base), from: WORKFLOW_START, to: id, when: "always" });
  else if (after) {
    // A step with outcomes branches on them: the new line takes the first outcome that has
    // no line yet.
    const condition = options.port ? conditionOfPort(options.port) : suggestCondition(base, after);
    edges.push({ id: freeEdgeId(base), from: after, to: id, when: condition.when, outcome: condition.outcome ?? null });
  }
  return { workflow: { nodes: [...base.nodes, node], edges }, id };
}

/** The lines that lead into a step, Start's included. */
export function edgesInto(workflow: Workflow, nodeId: string): WorkflowEdge[] {
  return workflow.edges.filter((edge) => edge.to === nodeId);
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

/**
 * What "Add an input" adds to `nodeId`: a line from the first other step that does not
 * lead here yet in the condition it most likely wants. Null when every such line exists,
 * so the button can say there is nothing to add instead of doing nothing.
 */
export function nextInput(workflow: Workflow, nodeId: string): Workflow | null {
  for (const source of workflow.nodes) {
    if (source.id === nodeId) continue;
    const suggestion = suggestCondition(workflow, source.id);
    const result = connect(workflow, source.id, nodeId, suggestion.when, suggestion.outcome);
    if (result.workflow !== workflow) return result.workflow;
  }
  return null;
}

/**
 * What "Add an output" adds to `nodeId`: a line in the suggested condition to the first
 * step it does not lead to yet, or back to itself when there is no other step. Null when
 * every such line exists.
 */
export function nextOutput(workflow: Workflow, nodeId: string): Workflow | null {
  const suggestion = suggestCondition(workflow, nodeId);
  const others = workflow.nodes.filter((item) => item.id !== nodeId);
  const targets = others.length ? others : workflow.nodes.filter((item) => item.id === nodeId);
  for (const target of targets) {
    const result = connect(workflow, nodeId, target.id, suggestion.when, suggestion.outcome);
    if (result.workflow !== workflow) return result.workflow;
  }
  return null;
}

/** An agent a step can name. Archived agents are listed so their steps can say so. */
export interface WorkflowAgent {
  id: string;
  name: string;
  archived?: boolean;
}

/** Whether the list of agents has arrived. */
export type AgentListState = "loading" | "error" | "ready";

/** How a step names its agent: archived ones say so, and a list still loading is not "unavailable". */
export function agentLabel(id: string, agents: WorkflowAgent[], state: AgentListState = "ready"): string {
  const agent = agents.find((item) => item.id === id);
  if (agent) return agent.archived ? `Archived agent: ${agent.name}` : agent.name;
  if (state === "loading") return "Loading agents…";
  if (state === "error") return "Agents could not be loaded";
  return "Unavailable agent";
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
  return workflow;
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
    build: () => ({
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
    build: () => ({
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
    build: () => ({
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
