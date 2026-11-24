"use client";

import { useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { ArrowRight, LayoutGrid, Plus, Sparkles, Trash2, X } from "lucide-react";

import { Button } from "@/components/ui/button";
import { CheckboxField } from "@/components/ui/checkbox";
import { Disclosure } from "@/components/ui/disclosure";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Segmented } from "@/components/ui/segmented";
import { Select } from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { brandName } from "@/lib/presentation";
import {
  DEFAULT_MAX_RUNS,
  MAX_RUNS_LIMIT,
  MAX_STEPS,
  WORKFLOW_EXAMPLES,
  WORKFLOW_START,
  addStep,
  conditionPhrase,
  connect,
  edgesFrom,
  isOutcomeLabel,
  isStartEdge,
  lastStep,
  loopSteps,
  patchEdge,
  patchStep,
  removeEdge,
  removeStep,
  setStartsRun,
  slugify,
  startsRun,
  suggestCondition,
  tidy,
  type Workflow,
  type WorkflowEdge,
  type WorkflowIssue,
  type WorkflowNode,
  type WorkflowWhen,
} from "@/lib/workflow";

import { WorkflowCanvas, type WorkflowSelection } from "./workflow-canvas";

export interface AgentOption {
  id: string;
  name: string;
  harness?: string | null;
}

/** "success" | "failure" | "always" | "outcome:<label>" for one select. */
function conditionValue(edge: WorkflowEdge): string {
  return edge.when === "outcome" ? `outcome:${edge.outcome ?? ""}` : (edge.when ?? "success");
}

function parseCondition(value: string): { when: WorkflowWhen; outcome: string | null } {
  if (value.startsWith("outcome:")) return { when: "outcome", outcome: value.slice(8) || null };
  return { when: value as WorkflowWhen, outcome: null };
}

function ConditionSelect({
  edge,
  outcomes,
  onChange,
  label,
  id,
}: {
  edge: WorkflowEdge;
  outcomes: string[];
  onChange: (patch: Partial<WorkflowEdge>) => void;
  label: string;
  id?: string;
}) {
  const value = conditionValue(edge);
  const known = edge.when !== "outcome" || (edge.outcome != null && outcomes.includes(edge.outcome));
  return (
    <Select id={id} aria-label={label} value={value} onChange={(event) => onChange(parseCondition(event.target.value))}>
      <option value="success">When it succeeds</option>
      <option value="failure">When it fails</option>
      <option value="always">Always</option>
      {outcomes.map((outcome) => (
        <option key={outcome} value={`outcome:${outcome}`}>
          When the outcome is {outcome}
        </option>
      ))}
      {!known ? <option value={value}>Choose an outcome…</option> : null}
    </Select>
  );
}

/** Outcome labels as removable chips with an input that adds on Enter or comma. */
function OutcomeInput({ value, onChange, id }: { value: string[]; onChange: (next: string[]) => void; id: string }) {
  const [draft, setDraft] = useState("");
  /** Add every label in `text` (separated by commas or line breaks). */
  const addAll = (text: string) => {
    const next = [...value];
    for (const part of text.split(/[,\n]/)) {
      const label = slugify(part, 32);
      if (label && isOutcomeLabel(label) && !next.includes(label)) next.push(label);
    }
    if (next.length !== value.length) onChange(next);
  };
  const add = () => {
    addAll(draft);
    setDraft("");
  };
  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Enter" || event.key === ",") {
      event.preventDefault();
      add();
    } else if (event.key === "Backspace" && !draft && value.length) {
      onChange(value.slice(0, -1));
    }
  };
  return (
    <div className="flex flex-wrap items-center gap-1.5">
      {value.map((outcome) => (
        <span key={outcome} className="flex h-8 items-center gap-1 rounded-full border bg-subtle pl-2.5 pr-1 text-[13px]">
          {outcome}
          <button
            type="button"
            className="flex h-6 w-6 items-center justify-center rounded-full text-muted-foreground hover:bg-muted hover:text-foreground"
            aria-label={`Remove outcome ${outcome}`}
            onClick={() => onChange(value.filter((item) => item !== outcome))}
          >
            <X className="h-3.5 w-3.5" />
          </button>
        </span>
      ))}
      <Input
        id={id}
        className="h-8 w-36 flex-1"
        value={draft}
        onChange={(event) => {
          // A pasted or dictated list: everything before the last comma is complete.
          const text = event.target.value;
          const cut = Math.max(text.lastIndexOf(","), text.lastIndexOf("\n"));
          if (cut < 0) return setDraft(text);
          addAll(text.slice(0, cut));
          setDraft(text.slice(cut + 1).trimStart());
        }}
        onKeyDown={onKeyDown}
        onBlur={add}
        placeholder={value.length ? "Add another" : "pass, fail…"}
      />
    </div>
  );
}

/** Everything about one step. Used beside the canvas and in the list view. */
function StepFields({
  workflow,
  node,
  agents,
  onChange,
  onSelect,
  inLoop,
}: {
  workflow: Workflow;
  node: WorkflowNode;
  agents: AgentOption[];
  onChange: (next: Workflow) => void;
  onSelect: (selection: WorkflowSelection) => void;
  inLoop: boolean;
}) {
  const patch = (change: Partial<WorkflowNode>) => onChange(patchStep(workflow, node.id, change));
  const lines = edgesFrom(workflow, node.id);
  const others = workflow.nodes.filter((item) => item.id !== node.id);
  const outcomes = node.outcomes ?? [];
  const known = agents.some((agent) => agent.id === node.agentId);
  const unused = outcomes.filter((outcome) => !lines.some((edge) => edge.when === "outcome" && edge.outcome === outcome));
  const key = `step-${node.id}`;

  return (
    <div className="space-y-4">
      <div className="grid gap-4 sm:grid-cols-2">
        <Field label="Step name" htmlFor={`${key}-name`}>
          <Input id={`${key}-name`} value={node.name} maxLength={60} onChange={(event) => patch({ name: event.target.value })} />
        </Field>
        <Field label="Agent" htmlFor={`${key}-agent`}>
          <Select id={`${key}-agent`} value={node.agentId} onChange={(event) => patch({ agentId: event.target.value })}>
            <option value="">Choose an agent…</option>
            {agents.map((agent) => (
              <option key={agent.id} value={agent.id}>
                {agent.name}
                {agent.harness ? ` (${brandName(agent.harness)})` : ""}
              </option>
            ))}
            {node.agentId && !known ? <option value={node.agentId}>Unavailable agent</option> : null}
          </Select>
        </Field>
      </div>
      <Field
        label="What this step does"
        htmlFor={`${key}-instructions`}
        optional
        hint="Given to the agent for this step only, on top of its own instructions and the team goal."
      >
        <Textarea
          id={`${key}-instructions`}
          className="min-h-20"
          value={node.instructions ?? ""}
          maxLength={4000}
          onChange={(event) => patch({ instructions: event.target.value })}
          placeholder="Run the test suite and report what failed."
        />
      </Field>
      <Field
        label="Outcomes"
        htmlFor={`${key}-outcomes`}
        optional
        hint="Results the agent chooses from when it finishes. A line can then follow one of them."
      >
        <OutcomeInput id={`${key}-outcomes`} value={outcomes} onChange={(next) => patch({ outcomes: next })} />
      </Field>

      <div className="space-y-2">
        <p className="text-sm font-medium">What happens next</p>
        {lines.length ? (
          <ul className="space-y-2">
            {lines.map((edge, index) => (
              <li key={edge.id} className="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-2 sm:grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)_auto]">
                <ConditionSelect
                  edge={edge}
                  outcomes={outcomes}
                  label={`Condition of line ${index + 1}`}
                  onChange={(change) => onChange(patchEdge(workflow, edge.id, change))}
                />
                <ArrowRight className="hidden h-4 w-4 text-muted-foreground sm:block" aria-hidden />
                <Select
                  className="col-start-1 sm:col-start-auto"
                  aria-label={`Next step of line ${index + 1}`}
                  value={edge.to}
                  onChange={(event) => onChange(patchEdge(workflow, edge.id, { to: event.target.value }))}
                >
                  {others.map((item) => (
                    <option key={item.id} value={item.id}>
                      {item.name || "Unnamed step"}
                    </option>
                  ))}
                  {edge.to === node.id ? <option value={node.id}>This step</option> : null}
                </Select>
                <Button
                  type="button"
                  size="icon-sm"
                  variant="ghost"
                  className="col-start-2 row-start-1 sm:col-start-auto sm:row-start-auto"
                  aria-label={`Remove line ${index + 1}`}
                  onClick={() => onChange(removeEdge(workflow, edge.id))}
                >
                  <X />
                </Button>
              </li>
            ))}
          </ul>
        ) : (
          <p className="text-[13px] text-muted-foreground">
            Nothing follows this step. The run ends when no step is left working.
          </p>
        )}
        {unused.length && lines.length ? (
          <p className="text-[13px] text-muted-foreground">
            No line follows {unused.length === 1 ? "the outcome" : "the outcomes"} {unused.join(", ")}: the run stops
            there.
          </p>
        ) : null}
        <Button
          type="button"
          size="sm"
          variant="outline"
          disabled={!others.length}
          onClick={() => {
            const suggestion = suggestCondition(workflow, node.id);
            const target = others.find((item) => !lines.some((edge) => edge.to === item.id)) ?? others[0];
            const result = connect(workflow, node.id, target.id, suggestion.when, suggestion.outcome);
            onChange(result.workflow);
          }}
        >
          <Plus /> Add a next step
        </Button>
      </div>

      <CheckboxField
        title="A run starts here"
        description="More than one step can start a run; they then work at the same time."
        checked={startsRun(workflow, node.id)}
        onChange={(event) => onChange(setStartsRun(workflow, node.id, event.target.checked))}
      />

      {inLoop ? (
        <Field
          label="Run at most"
          htmlFor={`${key}-max`}
          hint="This step is on a loop. When it would run more often than this, the run fails instead of going round again."
        >
          <div className="flex items-center gap-2">
            <Input
              id={`${key}-max`}
              type="number"
              className="w-24"
              min={1}
              max={MAX_RUNS_LIMIT}
              value={node.maxRuns ?? DEFAULT_MAX_RUNS}
              onChange={(event) => patch({ maxRuns: event.target.value ? Number(event.target.value) : null })}
            />
            <span className="text-[13px] text-muted-foreground">times in one run</span>
          </div>
        </Field>
      ) : null}

      <Disclosure
        title="Model override"
        summary={node.model?.model ? [node.model.provider, node.model.model].filter(Boolean).join("/") : "Uses the team's or the agent's model"}
      >
        <div className="grid gap-3 sm:grid-cols-2">
          <Field label="Provider" htmlFor={`${key}-provider`} optional>
            <Input
              id={`${key}-provider`}
              className="font-mono"
              value={node.model?.provider ?? ""}
              onChange={(event) => patch({ model: { ...node.model, provider: event.target.value } })}
              placeholder="openai"
            />
          </Field>
          <Field label="Model" htmlFor={`${key}-model`} optional>
            <Input
              id={`${key}-model`}
              className="font-mono"
              value={node.model?.model ?? ""}
              onChange={(event) => patch({ model: { ...node.model, model: event.target.value } })}
              placeholder="gpt-5"
            />
          </Field>
        </div>
      </Disclosure>

      <div className="flex justify-end border-t pt-3">
        <Button
          type="button"
          size="sm"
          variant="ghost"
          className="text-destructive hover:text-destructive"
          onClick={() => {
            onChange(removeStep(workflow, node.id));
            onSelect(null);
          }}
        >
          <Trash2 /> Remove step
        </Button>
      </div>
    </div>
  );
}

function LineFields({
  workflow,
  edge,
  onChange,
  onSelect,
}: {
  workflow: Workflow;
  edge: WorkflowEdge;
  onChange: (next: Workflow) => void;
  onSelect: (selection: WorkflowSelection) => void;
}) {
  const name = (id: string) => workflow.nodes.find((node) => node.id === id)?.name || "Unnamed step";
  const source = workflow.nodes.find((node) => node.id === edge.from);
  return (
    <div className="space-y-4">
      {isStartEdge(edge) ? (
        <p className="text-sm">
          A run starts with <strong className="font-medium">{name(edge.to)}</strong>.
        </p>
      ) : (
        <>
          <p className="text-sm">
            After <strong className="font-medium">{name(edge.from)}</strong>, continue with{" "}
            <strong className="font-medium">{name(edge.to)}</strong> {conditionPhrase(edge)}.
          </p>
          <Field
            label="Follow this line"
            htmlFor={`line-${edge.id}-when`}
            hint={
              source?.outcomes?.length
                ? undefined
                : `To branch on a result, give ${name(edge.from)} outcomes such as pass and fail first.`
            }
          >
            <ConditionSelect
              id={`line-${edge.id}-when`}
              edge={edge}
              outcomes={source?.outcomes ?? []}
              label="Condition"
              onChange={(change) => onChange(patchEdge(workflow, edge.id, change))}
            />
          </Field>
        </>
      )}
      <div className="flex flex-wrap justify-between gap-2 border-t pt-3">
        {isStartEdge(edge) ? <span /> : (
          <Button type="button" size="sm" variant="outline" onClick={() => onSelect({ kind: "node", id: edge.from })}>
            Edit {name(edge.from)}
          </Button>
        )}
        <Button
          type="button"
          size="sm"
          variant="ghost"
          className="text-destructive hover:text-destructive"
          onClick={() => {
            onChange(removeEdge(workflow, edge.id));
            onSelect(null);
          }}
        >
          <Trash2 /> Remove line
        </Button>
      </div>
    </div>
  );
}

/**
 * Build a workflow. The canvas shows the whole picture and takes drag gestures; the panel
 * next to it edits the selected step or line with ordinary form fields. The list view
 * offers the same fields without a canvas, for phones and for keyboard-only use.
 */
export function WorkflowEditor({
  value,
  onChange,
  agents,
  issues,
  revealProblem,
}: {
  value: Workflow;
  onChange: (next: Workflow) => void;
  agents: AgentOption[];
  issues: WorkflowIssue[];
  /** Changes when a save was refused: open the first step or line that has a problem. */
  revealProblem?: number;
}) {
  const [selection, setSelection] = useState<WorkflowSelection>(null);
  const [view, setView] = useState<"canvas" | "list">("canvas");
  const [fitKey, setFitKey] = useState(0);
  // Phones start with the list: dragging lines between cards needs room.
  useEffect(() => {
    if (window.matchMedia("(max-width: 639px)").matches) setView("list");
  }, []);

  const names = useMemo(() => new Map(agents.map((agent) => [agent.id, agent.name])), [agents]);
  const agentName = useCallback((id: string) => names.get(id) ?? "Unavailable agent", [names]);
  const loops = useMemo(() => loopSteps(value), [value]);
  const problems = useMemo(
    () => new Set(issues.flatMap((issue) => (issue.nodeId ? [issue.nodeId] : []))),
    [issues],
  );

  const selectedNode = selection?.kind === "node" ? value.nodes.find((node) => node.id === selection.id) : undefined;
  const selectedEdge = selection?.kind === "edge" ? value.edges.find((edge) => edge.id === selection.id) : undefined;

  // A new step continues from the selected step, or from the end of the line when the
  // workflow has one. Then its fields come into view, ready for a name.
  const [opening, setOpening] = useState<string | null>(null);
  const panel = useRef<HTMLDivElement>(null);
  const open = (id: string) => {
    setSelection({ kind: "node", id });
    setOpening(id);
  };
  const add = () => {
    const result = addStep(value, selectedNode?.id ?? lastStep(value));
    onChange(result.workflow);
    open(result.id);
  };
  const firstProblem = issues.find((issue) => issue.nodeId || issue.edgeId);
  const problemRef = useRef(firstProblem);
  problemRef.current = firstProblem;
  useEffect(() => {
    const problem = problemRef.current;
    if (!revealProblem || !problem) return;
    if (problem.nodeId) {
      setSelection({ kind: "node", id: problem.nodeId });
      setOpening(problem.nodeId);
    } else if (problem.edgeId) {
      setSelection({ kind: "edge", id: problem.edgeId });
    }
  }, [revealProblem]);
  useEffect(() => {
    if (!opening || selectedNode?.id !== opening) return;
    const input = document.getElementById(`step-${opening}-name`) as HTMLInputElement | null;
    if (!input) return;
    // Bring the first fields into view, but keep the canvas on screen above them.
    const top = panel.current?.getBoundingClientRect().top ?? 0;
    const wanted = window.innerHeight - 280;
    if (view === "canvas" && top > wanted) window.scrollBy({ top: top - wanted, behavior: "smooth" });
    input.focus({ preventScroll: true });
    if (view === "list") input.scrollIntoView({ block: "center", behavior: "smooth" });
    input.select();
    setOpening(null);
  }, [opening, selectedNode, view]);

  const toolbar = (
    <div className="flex flex-wrap items-center gap-2">
      <Segmented
        label="How to show the workflow"
        size="sm"
        value={view}
        onChange={setView}
        options={[
          { value: "canvas", label: "Canvas" },
          { value: "list", label: "List" },
        ]}
      />
      <div className="ml-auto flex flex-wrap items-center gap-2">
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button type="button" size="sm" variant="ghost">
              <Sparkles /> Examples
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-72">
            {WORKFLOW_EXAMPLES.map((example) => (
              <DropdownMenuItem
                key={example.id}
                className="flex-col items-start gap-0.5"
                onSelect={() => {
                  if (
                    value.nodes.length &&
                    !window.confirm("Replace the current workflow with this example? You choose the agents afterwards.")
                  )
                    return;
                  onChange(example.build());
                  setSelection(null);
                }}
              >
                <span className="font-medium">{example.title}</span>
                <span className="text-xs text-muted-foreground">{example.description}</span>
              </DropdownMenuItem>
            ))}
          </DropdownMenuContent>
        </DropdownMenu>
        {view === "canvas" ? (
          <Button type="button" size="sm" variant="ghost" onClick={() => {
              onChange(tidy(value));
              setFitKey((key) => key + 1);
            }} disabled={!value.nodes.length}>
            <LayoutGrid /> Tidy up
          </Button>
        ) : null}
        <Button type="button" size="sm" variant="outline" onClick={add} disabled={value.nodes.length >= MAX_STEPS}>
          <Plus /> Add step
        </Button>
      </div>
    </div>
  );

  if (view === "list") {
    return (
      <div className="space-y-3">
        {toolbar}
        {value.nodes.length ? (
          <ol className="space-y-2">
            {value.nodes.map((node, index) => {
              const next = edgesFrom(value, node.id);
              return (
                <li key={node.id}>
                  <Disclosure
                    title={
                      <span className="flex items-center gap-2">
                        <span className="tabular text-muted-foreground">{index + 1}.</span>
                        {node.name || "Unnamed step"}
                      </span>
                    }
                    summary={[
                      node.agentId ? agentName(node.agentId) : "No agent yet",
                      startsRun(value, node.id) ? "starts the run" : null,
                      next.length
                        ? `then ${next.map((edge) => value.nodes.find((item) => item.id === edge.to)?.name ?? "?").join(", ")}`
                        : "last step",
                    ]
                      .filter(Boolean)
                      .join(" · ")}
                    open={selection?.kind === "node" && selection.id === node.id}
                    onOpenChange={(open) => setSelection(open ? { kind: "node", id: node.id } : null)}
                  >
                    <StepFields
                      workflow={value}
                      node={node}
                      agents={agents}
                      onChange={onChange}
                      onSelect={setSelection}
                      inLoop={loops.has(node.id)}
                    />
                  </Disclosure>
                </li>
              );
            })}
          </ol>
        ) : (
          <p className="rounded-lg border border-dashed p-6 text-center text-sm text-muted-foreground">
            No steps yet. Add a step, or start from an example.
          </p>
        )}
      </div>
    );
  }

  return (
    <div className="space-y-3">
      {toolbar}
      <div className="grid gap-3 min-[1700px]:grid-cols-[minmax(0,1fr)_23rem] min-[1700px]:items-start">
        <div className="space-y-2">
          <WorkflowCanvas
            label="Workflow canvas"
            className="h-[24rem] min-[1700px]:h-[34rem]"
            workflow={value}
            agentName={agentName}
            selection={selection}
            onSelect={setSelection}
            onOpen={open}
            problems={problems}
            fitKey={fitKey}
            empty={
              <div className="space-y-3">
                <p className="text-sm font-medium">Start with the first step</p>
                <p className="text-[13px] text-muted-foreground">
                  A step runs one agent. Add steps and connect them, or begin with an example and change it.
                </p>
                <div className="flex flex-wrap justify-center gap-2">
                  <Button type="button" size="sm" onClick={add}>
                    <Plus /> Add step
                  </Button>
                  <Button type="button" size="sm" variant="outline" onClick={() => onChange(WORKFLOW_EXAMPLES[0].build())}>
                    <Sparkles /> Use &ldquo;{WORKFLOW_EXAMPLES[0].title}&rdquo;
                  </Button>
                </div>
              </div>
            }
            onMove={(id, position) => onChange(patchStep(value, id, { position }))}
            onConnect={(from, to) => {
              const suggestion = from === WORKFLOW_START ? { when: "always" as const } : suggestCondition(value, from);
              const result = connect(value, from, to, suggestion.when, suggestion.outcome);
              onChange(result.workflow);
              if (result.id) setSelection({ kind: "edge", id: result.id });
            }}
            onRemove={(removed) => {
              let next = removed.edges.reduce(removeEdge, value);
              next = removed.nodes.reduce(removeStep, next);
              onChange(next);
              setSelection(null);
            }}
          />
          <p className="text-xs text-muted-foreground">
            Drag from the dot on the right of a step to another step to connect them. Select a step or a line to edit
            it below, double-click a step to jump to its fields, and press Backspace to remove what is selected.
          </p>
        </div>
        <div ref={panel} className="scroll-mb-24 rounded-lg border bg-card p-4" aria-live="polite">
          {selectedNode ? (
            <>
              <h3 className="mb-3 text-sm font-semibold">Step</h3>
              <StepFields
                key={selectedNode.id}
                workflow={value}
                node={selectedNode}
                agents={agents}
                onChange={onChange}
                onSelect={setSelection}
                inLoop={loops.has(selectedNode.id)}
              />
            </>
          ) : selectedEdge ? (
            <>
              <h3 className="mb-3 text-sm font-semibold">Line</h3>
              <LineFields key={selectedEdge.id} workflow={value} edge={selectedEdge} onChange={onChange} onSelect={setSelection} />
            </>
          ) : (
            <div className="space-y-3 text-[13px] text-muted-foreground">
              <h3 className="text-sm font-semibold text-foreground">How a workflow runs</h3>
              <ul className="list-disc space-y-1.5 pl-4">
                <li>Each step runs one agent. When it finishes, every line whose condition matches hands the work on.</li>
                <li>Two lines that match at once start two steps that work at the same time.</li>
                <li>A step with several incoming lines waits until no earlier step is still working.</li>
                <li>A line back to an earlier step makes a loop. Each step has a limit on how often it runs.</li>
              </ul>
              <p>Select a step or a line to edit it.</p>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
