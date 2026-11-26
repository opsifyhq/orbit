import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import {
  WORKFLOW_EXAMPLES,
  WORKFLOW_START,
  addStep,
  agentLabel,
  apart,
  cleanWorkflow,
  conditionOfPort,
  connect,
  isArranged,
  isPlain,
  lastStep,
  layoutWorkflow,
  limitReached,
  liveEdges,
  loopSteps,
  nextInput,
  nextOutput,
  nodeRunStates,
  nodeHeight,
  patchStep,
  portOf,
  removeStep,
  slugify,
  stepPorts,
  suggestCondition,
  takenEdges,
  tidy,
  withPositions,
  workflowFromSteps,
  workflowIssues,
  type Workflow,
} from "./workflow.ts";

const withAgents = (workflow: Workflow): Workflow => ({
  ...workflow,
  nodes: workflow.nodes.map((node) => ({ ...node, agentId: "agent-1" })),
});

test("every example is a valid workflow once agents are chosen", () => {
  for (const example of WORKFLOW_EXAMPLES) {
    const built = example.build();
    assert.ok(workflowIssues(built).every((issue) => issue.message.includes("no agent yet")), example.id);
    assert.deepEqual(workflowIssues(withAgents(built)), [], example.id);
    // Examples are arranged for the user, not placed by hand.
    assert.ok(isArranged(built), example.id);
  }
});

test("a pipeline becomes a line of steps", () => {
  const workflow = workflowFromSteps([
    { agentId: "a", name: "Coder" },
    { agentId: "b", name: "Reviewer" },
    { agentId: "a", name: "Coder" },
  ]);
  assert.deepEqual(
    workflow.nodes.map((node) => node.name),
    ["Coder", "Reviewer", "Coder 3"],
  );
  assert.deepEqual(
    workflow.edges.map((edge) => `${edge.from}>${edge.to}`),
    [`${WORKFLOW_START}>coder`, "coder>reviewer", "reviewer>coder-3"],
  );
  assert.deepEqual(workflowIssues(workflow), []);
});

test("problems are reported in plain words", () => {
  const messages = (workflow: Workflow) => workflowIssues(workflow).map((issue) => issue.message);
  assert.deepEqual(messages({ nodes: [], edges: [] }), ["Add at least one step."]);

  const loose = withAgents({
    nodes: [
      { id: "a", name: "A", agentId: "" },
      { id: "b", name: "B", agentId: "" },
    ],
    edges: [{ id: "1", from: WORKFLOW_START, to: "a" }],
  });
  assert.deepEqual(messages(loose), ["B can never run: no line leads to it."]);

  const noStart = withAgents({ nodes: [{ id: "a", name: "A", agentId: "" }], edges: [] });
  assert.match(messages(noStart)[0], /where the run starts/);

  const outcome = withAgents({
    nodes: [
      { id: "a", name: "A", agentId: "" },
      { id: "b", name: "B", agentId: "" },
    ],
    edges: [
      { id: "1", from: WORKFLOW_START, to: "a" },
      { id: "2", from: "a", to: "b", when: "outcome", outcome: "pass" },
    ],
  });
  assert.deepEqual(messages(outcome), ['A has no outcome "pass".']);
});

test("editing keeps the graph consistent", () => {
  let workflow: Workflow = { nodes: [], edges: [] };
  const first = addStep(workflow);
  workflow = first.workflow;
  // The first step starts the run.
  assert.deepEqual(workflow.edges, [{ id: "line-1", from: WORKFLOW_START, to: first.id, when: "always" }]);
  assert.equal(lastStep(workflow), first.id);
  const second = addStep(workflow, first.id);
  workflow = second.workflow;
  assert.equal(workflow.edges.length, 2);
  assert.equal(lastStep(workflow), second.id);
  // A branch from the same step lands below its sibling, not on top of it.
  assert.ok(isArranged(workflow), "steps added to an arranged workflow stay arranged");
  const sibling = addStep(withPositions(workflow), first.id);
  const placed = sibling.workflow.nodes.map((node) => `${node.position?.x},${node.position?.y}`);
  assert.equal(new Set(placed).size, placed.length);
  assert.equal(lastStep(sibling.workflow), undefined);

  // Connecting the same pair with the same condition twice adds nothing.
  assert.equal(connect(workflow, first.id, second.id).workflow, workflow);
  assert.equal(connect(workflow, first.id, WORKFLOW_START).id, null);

  // Outcomes suggest the next free outcome for a new line.
  workflow = patchStep(workflow, first.id, { outcomes: ["pass", "fail"] });
  workflow = connect(workflow, first.id, second.id, "outcome", "pass").workflow;
  assert.deepEqual(suggestCondition(workflow, first.id), { when: "outcome", outcome: "fail" });

  // A step added after a step with outcomes follows the next free outcome.
  const branch = addStep(workflow, first.id);
  assert.deepEqual(
    branch.workflow.edges.filter((edge) => edge.to === branch.id).map((edge) => [edge.when, edge.outcome]),
    [["outcome", "fail"]],
  );

  // Removing an outcome keeps the line but asks for a new choice.
  workflow = patchStep(workflow, first.id, { outcomes: ["fail"] });
  assert.equal(workflow.edges.find((edge) => edge.when === "outcome")?.outcome, null);

  // Removing a step removes its lines.
  workflow = removeStep(workflow, second.id);
  assert.deepEqual(
    workflow.edges.map((edge) => edge.to),
    [first.id],
  );
});

test("a step lists its outputs, and a line knows which one it leaves", () => {
  const review = WORKFLOW_EXAMPLES.find((example) => example.id === "review-loop")!.build();
  const node = (id: string) => review.nodes.find((item) => item.id === id)!;
  // A step without outcomes has one output and a small card.
  assert.deepEqual(stepPorts(review, node("build")).map((port) => port.id), ["success"]);
  assert.ok(isPlain(stepPorts(review, node("build"))));
  // Outcomes come first, in the order they were given; unused extras are not listed.
  assert.deepEqual(stepPorts(review, node("review")).map((port) => port.label), ["approved", "changes"]);
  assert.deepEqual(
    stepPorts(review, node("review"), true).map((port) => port.label),
    ["approved", "changes", "done", "failed", "always"],
  );
  assert.ok(nodeHeight(review, node("review")) > nodeHeight(review, node("build")));
  // A line that follows a failure adds that output to the card.
  const handled = connect(review, "build", "fix", "failure").workflow;
  assert.deepEqual(stepPorts(handled, node("build")).map((port) => port.id), ["success", "failure"]);
  // An outcome the step no longer has still shows, so its line has somewhere to leave from.
  const stale = patchStep(review, "review", { outcomes: ["approved"] });
  assert.deepEqual(stepPorts(stale, stale.nodes[1]).map((port) => port.label), ["approved", "outcome?"]);
  for (const edge of review.edges) assert.deepEqual(conditionOfPort(portOf(edge)), { when: edge.when, outcome: edge.outcome ?? null });
});

test("a workflow stays arranged until a step is placed by hand", () => {
  const example = WORKFLOW_EXAMPLES[0].build();
  const added = addStep(example, "ship");
  assert.ok(isArranged(added.workflow));
  // Dropping a step at a place fixes every step where it is shown.
  const dropped = addStep(example, "ship", { at: { x: 900, y: 40 } });
  assert.ok(dropped.workflow.nodes.every((node) => node.position));
  assert.deepEqual(dropped.workflow.nodes.at(-1)?.position, { x: 900, y: 40 });
  assert.ok(isArranged(tidy(dropped.workflow)));
  // From an output: the new line follows it.
  const failed = addStep(example, "build", { port: "failure" });
  assert.equal(failed.workflow.edges.at(-1)?.when, "failure");
  const second = addStep(example, WORKFLOW_START);
  assert.deepEqual(second.workflow.edges.at(-1), { id: "line-6", from: WORKFLOW_START, to: second.id, when: "always" });
});

test("steps in a column never overlap, whatever their size", () => {
  const node = (id: string, outcomes?: string[]) => ({ id, name: id, agentId: "a", outcomes });
  const workflow: Workflow = {
    nodes: [node("split", ["a", "b", "c"]), node("one", ["x", "y", "z", "w"]), node("two"), node("three", ["p", "q"])],
    edges: [
      { id: "s", from: WORKFLOW_START, to: "split", when: "always" },
      { id: "1", from: "split", to: "one", when: "outcome", outcome: "a" },
      { id: "2", from: "split", to: "two", when: "outcome", outcome: "b" },
      { id: "3", from: "split", to: "three", when: "outcome", outcome: "c" },
    ],
  };
  const layout = layoutWorkflow(workflow);
  const column = ["one", "two", "three"].map((id) => ({ id, y: layout.get(id)!.y, h: nodeHeight(workflow, workflow.nodes.find((item) => item.id === id)!) }));
  // In the order of the outputs they follow, with room between them.
  for (let index = 1; index < column.length; index += 1) {
    assert.ok(column[index].y >= column[index - 1].y + column[index - 1].h + 20, column[index].id);
  }
});

test("a loop back into parallel steps keeps them side by side", () => {
  const node = (id: string, outcomes?: string[]) => ({ id, name: id, agentId: "a", outcomes });
  const line = (id: string, from: string, to: string, outcome?: string) => ({
    id,
    from,
    to,
    when: outcome ? ("outcome" as const) : ("success" as const),
    outcome,
  });
  const layout = layoutWorkflow({
    nodes: [node("plan"), node("frontend"), node("backend"), node("integrate", ["ok", "redo-ui", "redo-api"]), node("ship")],
    edges: [
      { id: "s", from: WORKFLOW_START, to: "plan", when: "always" },
      line("1", "plan", "frontend"),
      line("2", "plan", "backend"),
      line("3", "frontend", "integrate"),
      line("4", "backend", "integrate"),
      line("5", "integrate", "ship", "ok"),
      line("6", "integrate", "frontend", "redo-ui"),
      line("7", "integrate", "backend", "redo-api"),
    ],
  });
  assert.equal(layout.get("frontend")!.x, layout.get("backend")!.x);
  assert.ok(layout.get("backend")!.x < layout.get("integrate")!.x);
  assert.ok(layout.get("integrate")!.x < layout.get("ship")!.x);
});

test("steps that Start does not reach are still laid out as a chain", () => {
  const node = (id: string) => ({ id, name: id, agentId: "a" });
  const workflow: Workflow = {
    nodes: ["first", "loose", "after", "ring-a", "ring-b"].map(node),
    edges: [
      { id: "s", from: WORKFLOW_START, to: "first", when: "always" },
      { id: "1", from: "loose", to: "after", when: "success" },
      { id: "2", from: "ring-a", to: "ring-b", when: "success" },
      { id: "3", from: "ring-b", to: "ring-a", when: "success" },
    ],
  };
  const layout = layoutWorkflow(workflow);
  assert.ok(layout.get("loose")!.x < layout.get("after")!.x);
  assert.ok(layout.get("ring-a")!.x < layout.get("ring-b")!.x);
  // No two cards share a place.
  const boxes = workflow.nodes.map((item) => ({ ...layout.get(item.id)!, h: nodeHeight(workflow, item) }));
  for (const a of boxes) {
    for (const b of boxes) {
      if (a === b) continue;
      assert.ok(a.x !== b.x || a.y + a.h <= b.y || b.y + b.h <= a.y, JSON.stringify([a, b]));
    }
  }
});

test("steps placed by hand never lie on top of each other", () => {
  const node = (id: string, y: number, outcomes?: string[]) => ({ id, name: id, agentId: "a", outcomes, position: { x: 0, y } });
  const workflow: Workflow = {
    // "tall" has four outputs, so its card reaches well past where "under" was placed.
    nodes: [node("tall", 0, ["a", "b", "c", "d"]), node("under", 100), node("far", 600), { ...node("beside", 20), position: { x: 400, y: 20 } }],
    edges: [{ id: "s", from: WORKFLOW_START, to: "tall", when: "always" }],
  };
  const shown = withPositions(workflow);
  const at = (id: string) => shown.nodes.find((item) => item.id === id)!.position!;
  assert.ok(at("under").y >= nodeHeight(workflow, workflow.nodes[0]) + 12);
  // What did not overlap stays where it was put.
  assert.deepEqual(at("tall"), { x: 0, y: 0 });
  assert.deepEqual(at("far"), { x: 0, y: 600 });
  assert.deepEqual(at("beside"), { x: 400, y: 20 });
  assert.equal(apart(shown), shown);
});

test("loops are found and laid out left to right", () => {
  const loop = WORKFLOW_EXAMPLES.find((example) => example.id === "test-gate")!.build();
  assert.deepEqual([...loopSteps(loop)].sort(), ["fix", "test"]);
  const layout = layoutWorkflow(loop);
  assert.ok(layout.get("test")!.x < layout.get("fix")!.x);
  assert.ok(layout.get("test")!.x < layout.get("deploy")!.x);
  // The way forward is one level line; the step that only leads back hangs below it.
  assert.equal(layout.get("test")!.y, layout.get("deploy")!.y);
  assert.ok(layout.get("fix")!.y > layout.get("test")!.y);
});

test("parallel steps keep the order of the steps they come from", () => {
  const node = (id: string) => ({ id, name: id, agentId: "a" });
  const line = (id: string, from: string, to: string) => ({ id, from, to, when: "success" as const });
  const layout = layoutWorkflow({
    nodes: ["plan", "a", "b", "b2", "a2", "join"].map(node),
    edges: [
      { id: "s", from: WORKFLOW_START, to: "plan", when: "always" },
      line("1", "plan", "a"),
      line("2", "plan", "b"),
      line("3", "b", "b2"),
      line("4", "a", "a2"),
      line("5", "a2", "join"),
      line("6", "b2", "join"),
    ],
  });
  assert.ok(layout.get("a")!.y < layout.get("b")!.y);
  // b2 is listed first, but follows b, which is the lower branch.
  assert.ok(layout.get("a2")!.y < layout.get("b2")!.y);
  assert.equal(layout.get("join")!.y, layout.get("plan")!.y);
});

test("what is sent to the API has no empty extras", () => {
  const cleaned = cleanWorkflow({
    nodes: [{ id: "a", name: "  A ", agentId: "x", instructions: " ", outcomes: [], model: { provider: " ", model: "" }, position: { x: 1.4, y: 2.6 } }],
    edges: [{ id: "1", from: WORKFLOW_START, to: "a", when: "success", outcome: "stale" }],
  });
  assert.deepEqual(JSON.parse(JSON.stringify(cleaned)), {
    nodes: [{ id: "a", name: "A", agentId: "x", position: { x: 1, y: 3 } }],
    edges: [{ id: "1", from: WORKFLOW_START, to: "a", when: "always" }],
  });
});

test("a run is summarised per step and per line", () => {
  const steps = [
    { stepIndex: 0, nodeId: "test", status: "succeeded", outcome: "fail", activatedBy: [{ edge: "l1", step: null }] },
    { stepIndex: 1, nodeId: "fix", status: "succeeded", activatedBy: [{ edge: "l3", step: 0 }] },
    { stepIndex: 2, nodeId: "test", status: "running", activatedBy: [{ edge: "l4", step: 1 }] },
  ];
  const states = nodeRunStates(steps);
  assert.deepEqual(states.get("test"), { status: "running", runs: 2, outcome: undefined, stepIndex: 2 });
  assert.equal(nodeRunStates([{ stepIndex: 0, nodeId: "a", status: "cancelled" }]).get("a")?.status, "stopped");
  assert.deepEqual([...takenEdges(steps)], [["l1", 1], ["l3", 1], ["l4", 1]]);
  assert.deepEqual([...liveEdges(steps)], ["l4"]);
});

test("a run that ran out of tries points at the step that was due again", () => {
  const workflow: Workflow = {
    nodes: [
      { id: "test", name: "Test", agentId: "a", outcomes: ["pass", "fail"], maxRuns: 2 },
      { id: "fix", name: "Fix", agentId: "a" },
      { id: "ship", name: "Ship", agentId: "a" },
    ],
    edges: [
      { id: "l1", from: WORKFLOW_START, to: "test", when: "always" },
      { id: "l2", from: "test", to: "ship", when: "outcome", outcome: "pass" },
      { id: "l3", from: "test", to: "fix", when: "outcome", outcome: "fail" },
      { id: "l4", from: "fix", to: "test", when: "success" },
    ],
  };
  const steps = [
    { stepIndex: 0, nodeId: "test", status: "succeeded", outcome: "fail", activatedBy: [{ edge: "l1", step: null }] },
    { stepIndex: 1, nodeId: "fix", status: "succeeded", activatedBy: [{ edge: "l3", step: 0 }] },
    { stepIndex: 2, nodeId: "test", status: "succeeded", outcome: "fail", activatedBy: [{ edge: "l4", step: 1 }] },
    { stepIndex: 3, nodeId: "fix", status: "succeeded", activatedBy: [{ edge: "l3", step: 2 }] },
  ];
  assert.deepEqual([...limitReached(workflow, steps)], ["test"]);
  // One round earlier the second test run had taken the line back, so nothing is stuck.
  assert.deepEqual([...limitReached(workflow, steps.slice(0, 3))], []);
});

test("names become ids", () => {
  assert.equal(slugify("Run the tests!"), "run-the-tests");
  assert.equal(slugify("  "), "");
});

test("the editor agrees with the server on the shared validation cases", () => {
  // The same file is asserted by `validate` in crates/orbit-domain/src/workflow.rs.
  const fixture = JSON.parse(
    readFileSync(new URL("../../../crates/orbit-domain/tests/fixtures/workflow-validation.json", import.meta.url), "utf8"),
  ) as { cases: { name: string; valid: boolean; workflow: Workflow }[] };
  assert.ok(fixture.cases.length > 20);
  for (const { name, valid, workflow } of fixture.cases) {
    const issues = workflowIssues(workflow);
    if (valid) assert.deepEqual(issues, [], name);
    else assert.ok(issues.length > 0, `${name}: the editor accepts what the server refuses`);
  }
});

test("adding an input or output never returns the same workflow", () => {
  const two: Workflow = {
    nodes: [
      { id: "a", name: "A", agentId: "x" },
      { id: "b", name: "B", agentId: "x" },
    ],
    edges: [{ id: "s", from: WORKFLOW_START, to: "a", when: "always" }],
  };
  const withLine = nextOutput(two, "a");
  assert.ok(withLine);
  assert.deepEqual(withLine.edges.at(-1), { id: withLine.edges.at(-1)?.id, from: "a", to: "b", when: "success", outcome: null });
  // The suggested line now exists: nothing is left to add, in either direction.
  assert.equal(nextOutput(withLine, "a"), null);
  assert.equal(nextInput(withLine, "b"), null);
  // b can still feed a.
  assert.equal(nextInput(withLine, "a")?.edges.at(-1)?.from, "b");
  // A step on its own can repeat itself once.
  const single: Workflow = { nodes: [{ id: "a", name: "A", agentId: "x" }], edges: [] };
  const loop = nextOutput(single, "a");
  assert.equal(loop?.edges[0].to, "a");
  assert.equal(nextOutput(loop!, "a"), null);
  assert.equal(nextInput(single, "a"), null);
});

test("steps name archived agents and do not call a loading list unavailable", () => {
  const agents = [
    { id: "1", name: "Builder" },
    { id: "2", name: "Old reviewer", archived: true },
  ];
  assert.equal(agentLabel("1", agents), "Builder");
  assert.equal(agentLabel("2", agents), "Archived agent: Old reviewer");
  assert.equal(agentLabel("3", agents), "Unavailable agent");
  assert.equal(agentLabel("3", [], "loading"), "Loading agents…");
  assert.equal(agentLabel("3", [], "error"), "Agents could not be loaded");
});
