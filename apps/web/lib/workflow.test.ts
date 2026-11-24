import assert from "node:assert/strict";
import { test } from "node:test";

import {
  WORKFLOW_EXAMPLES,
  WORKFLOW_START,
  addStep,
  cleanWorkflow,
  connect,
  lastStep,
  layoutWorkflow,
  limitReached,
  liveEdges,
  loopSteps,
  nodeRunStates,
  patchStep,
  removeStep,
  slugify,
  suggestCondition,
  takenEdges,
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
    assert.ok(built.nodes.every((node) => node.position), example.id);
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
  const sibling = addStep({ ...workflow, nodes: workflow.nodes.map((node) => ({ ...node, position: node.position ?? { x: 0, y: 0 } })) }, first.id);
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
