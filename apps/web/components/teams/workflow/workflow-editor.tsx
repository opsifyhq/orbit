"use client";

import { useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { ArrowRight, LayoutGrid, Maximize2, Minimize2, Plus, Redo2, Sparkles, Trash2, Undo2, X } from "lucide-react";

import { Button } from "@/components/ui/button";
import { CheckboxField } from "@/components/ui/checkbox";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";
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
import { useConfirm } from "@/components/ui/confirm";
import { scrollBehavior } from "@/lib/motion";
import { brandName } from "@/lib/presentation";
import {
  DEFAULT_MAX_RUNS,
  MAX_RUNS_LIMIT,
  MAX_STEPS,
  WORKFLOW_EXAMPLES,
  WORKFLOW_START,
  addStep,
  apart,
  conditionOfPort,
  conditionPhrase,
  connect,
  edgesFrom,
  edgesInto,
  isArranged,
  isOutcomeLabel,
  isStartEdge,
  lastStep,
  loopSteps,
  portOf,
  patchEdge,
  patchStep,
  removeEdge,
  removeStep,
  setStartsRun,
  slugify,
  startsRun,
  stepPorts,
  suggestCondition,
  tidy,
  withPositions,
  type Workflow,
  type WorkflowEdge,
  type WorkflowIssue,
  type WorkflowNode,
  type WorkflowWhen,
} from "@/lib/workflow";

import { WorkflowCanvas, WorkflowLegend, portColor, type WorkflowSelection } from "./workflow-canvas";

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

/**
 * "Run at most": the box can be emptied and retyped (#442). While it has focus it shows what
 * was typed and saves each value in range; on blur an empty box means the default and
 * anything else is clamped to 1-MAX_RUNS_LIMIT.
 */
function RunLimitInput({
  id,
  value,
  onChange,
}: {
  id: string;
  value: number | null | undefined;
  onChange: (next: number | null) => void;
}) {
  const [draft, setDraft] = useState<string | null>(null);
  const inRange = (n: number) => Number.isInteger(n) && n >= 1 && n <= MAX_RUNS_LIMIT;
  return (
    <Input
      id={id}
      type="number"
      inputMode="numeric"
      className="w-24"
      min={1}
      max={MAX_RUNS_LIMIT}
      step={1}
      placeholder={String(DEFAULT_MAX_RUNS)}
      value={draft ?? String(value ?? DEFAULT_MAX_RUNS)}
      onChange={(event) => {
        const text = event.target.value;
        setDraft(text);
        const n = Number(text);
        if (text.trim() && inRange(n) && n !== value) onChange(n);
      }}
      onBlur={() => {
        if (draft === null) return;
        const n = Math.round(Number(draft));
        const next = !draft.trim() || Number.isNaN(n) ? null : Math.min(MAX_RUNS_LIMIT, Math.max(1, n));
        if (next !== (value ?? null)) onChange(next);
        setDraft(null);
      }}
    />
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
  const incoming = edgesInto(workflow, node.id).filter((edge) => !isStartEdge(edge));
  const starts = startsRun(workflow, node.id);
  const others = workflow.nodes.filter((item) => item.id !== node.id);
  const outcomes = node.outcomes ?? [];
  const known = agents.some((agent) => agent.id === node.agentId);
  const unused = outcomes.filter((outcome) => !lines.some((edge) => edge.when === "outcome" && edge.outcome === outcome));
  const key = `step-${node.id}`;
  const ports = stepPorts(workflow, node, true);
  const colorOf = (edge: WorkflowEdge, from: WorkflowNode = node) => {
    const port = (from === node ? ports : stepPorts(workflow, from, true)).find((item) => item.id === portOf(edge));
    return port ? portColor(port) : "hsl(var(--muted-foreground))";
  };
  const stepOf = (id: string) => workflow.nodes.find((item) => item.id === id);

  return (
    // The fields follow the width of the panel they are in, not of the window.
    <div className="space-y-4 [container-type:inline-size]">
      <div className="grid gap-4 [@container(min-width:30rem)]:grid-cols-2">
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

      <section className="space-y-2 rounded-lg border bg-subtle/60 p-3" aria-label="Inputs">
        <h4 className="flex items-baseline gap-2 text-sm font-medium">
          Inputs <span className="text-[13px] font-normal text-muted-foreground">what starts this step</span>
        </h4>
        <CheckboxField
          title="The start of a run"
          description="More than one step can start a run; they then work at the same time."
          checked={starts}
          onChange={(event) => onChange(setStartsRun(workflow, node.id, event.target.checked))}
        />
        {incoming.length ? (
          <ul className="space-y-2">
            {incoming.map((edge, index) => {
              const source = stepOf(edge.from);
              return (
                <li
                  key={edge.id}
                  className="grid grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-2 [@container(min-width:30rem)]:grid-cols-[auto_minmax(0,1fr)_minmax(0,1fr)_auto]"
                >
                  <span
                    className="h-2.5 w-2.5 rounded-full"
                    style={{ background: source ? colorOf(edge, source) : undefined }}
                    aria-hidden
                  />
                  <Select
                    aria-label={`Step before, input ${index + 1}`}
                    value={edge.from}
                    onChange={(event) => {
                      const from = stepOf(event.target.value);
                      const keeps = edge.when !== "outcome" || Boolean(edge.outcome && from?.outcomes?.includes(edge.outcome));
                      onChange(
                        patchEdge(workflow, edge.id, keeps ? { from: event.target.value } : { from: event.target.value, ...suggestCondition(workflow, event.target.value) }),
                      );
                    }}
                  >
                    {others.map((item) => (
                      <option key={item.id} value={item.id}>
                        After {item.name || "Unnamed step"}
                      </option>
                    ))}
                    {edge.from === node.id ? <option value={node.id}>After this step</option> : null}
                  </Select>
                  <div className="col-span-2 col-start-2 [@container(min-width:30rem)]:col-span-1 [@container(min-width:30rem)]:col-start-auto">
                    <ConditionSelect
                      edge={edge}
                      outcomes={source?.outcomes ?? []}
                      label={`Condition of input ${index + 1}`}
                      onChange={(change) => onChange(patchEdge(workflow, edge.id, change))}
                    />
                  </div>
                  <Button
                    type="button"
                    size="icon-sm"
                    variant="ghost"
                    className="col-start-3 row-start-1 [@container(min-width:30rem)]:col-start-auto [@container(min-width:30rem)]:row-start-auto"
                    aria-label={`Remove input ${index + 1}`}
                    onClick={() => onChange(removeEdge(workflow, edge.id))}
                  >
                    <X />
                  </Button>
                </li>
              );
            })}
          </ul>
        ) : !starts ? (
          <p className="text-[13px] text-warning">Nothing leads to this step yet, so it can never run.</p>
        ) : null}
        {incoming.length + (starts ? 1 : 0) > 1 ? (
          <p className="text-[13px] text-muted-foreground">
            With several inputs this step waits until no earlier step is still working, then gets what each of them
            produced.
          </p>
        ) : null}
        <Button
          type="button"
          size="sm"
          variant="outline"
          disabled={!others.length}
          onClick={() => {
            const source = others.find((item) => !incoming.some((edge) => edge.from === item.id)) ?? others[0];
            const suggestion = suggestCondition(workflow, source.id);
            onChange(connect(workflow, source.id, node.id, suggestion.when, suggestion.outcome).workflow);
          }}
        >
          <Plus /> Add an input
        </Button>
      </section>

      <section className="space-y-3 rounded-lg border bg-subtle/60 p-3" aria-label="Outputs">
        <h4 className="flex items-baseline gap-2 text-sm font-medium">
          Outputs <span className="text-[13px] font-normal text-muted-foreground">what happens next</span>
        </h4>
        <Field
          label="Outcomes"
          htmlFor={`${key}-outcomes`}
          optional
          hint="Results the agent chooses from when it finishes, such as pass and fail. Each one becomes an output a line can follow."
        >
          <OutcomeInput id={`${key}-outcomes`} value={outcomes} onChange={(next) => patch({ outcomes: next })} />
        </Field>
        {lines.length ? (
          <ul className="space-y-2">
            {lines.map((edge, index) => (
              <li
                key={edge.id}
                className="grid grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-2 [@container(min-width:30rem)]:grid-cols-[auto_minmax(0,1fr)_auto_minmax(0,1fr)_auto]"
              >
                <span className="h-2.5 w-2.5 rounded-full" style={{ background: colorOf(edge) }} aria-hidden />
                <ConditionSelect
                  edge={edge}
                  outcomes={outcomes}
                  label={`Condition of output ${index + 1}`}
                  onChange={(change) => onChange(patchEdge(workflow, edge.id, change))}
                />
                <ArrowRight className="hidden h-4 w-4 text-muted-foreground [@container(min-width:30rem)]:block" aria-hidden />
                <Select
                  className="col-start-2 [@container(min-width:30rem)]:col-start-auto"
                  aria-label={`Next step of output ${index + 1}`}
                  value={edge.to}
                  onChange={(event) => onChange(patchEdge(workflow, edge.id, { to: event.target.value }))}
                >
                  {others.map((item) => (
                    <option key={item.id} value={item.id}>
                      {item.name || "Unnamed step"}
                    </option>
                  ))}
                  <option value={node.id}>This step again</option>
                </Select>
                <Button
                  type="button"
                  size="icon-sm"
                  variant="ghost"
                  className="col-start-3 row-start-1 [@container(min-width:30rem)]:col-start-auto [@container(min-width:30rem)]:row-start-auto"
                  aria-label={`Remove output ${index + 1}`}
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
          onClick={() => {
            const suggestion = suggestCondition(workflow, node.id);
            // With no other step to go to, the step can still repeat itself.
            const target = others.find((item) => !lines.some((edge) => edge.to === item.id)) ?? others[0] ?? node;
            const result = connect(workflow, node.id, target.id, suggestion.when, suggestion.outcome);
            onChange(result.workflow);
          }}
        >
          <Plus /> Add an output
        </Button>
      </section>

      {inLoop ? (
        <Field
          label="Run at most"
          htmlFor={`${key}-max`}
          hint="This step is on a loop. When it would run more often than this, the run fails instead of going round again."
        >
          <div className="flex items-center gap-2">
            <RunLimitInput id={`${key}-max`} value={node.maxRuns} onChange={(maxRuns) => patch({ maxRuns })} />
            <span className="text-[13px] text-muted-foreground">times in one run</span>
          </div>
        </Field>
      ) : null}

      <Disclosure
        title="Model override"
        summary={node.model?.model ? [node.model.provider, node.model.model].filter(Boolean).join("/") : "Uses the team's or the agent's model"}
      >
        <div className="grid gap-3 [@container(min-width:30rem)]:grid-cols-2">
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
  const port = source ? stepPorts(workflow, source, true).find((item) => item.id === portOf(edge)) : undefined;
  const target = (
    <Field label={isStartEdge(edge) ? "Start with" : "Then run"} htmlFor={`line-${edge.id}-to`}>
      <Select
        id={`line-${edge.id}-to`}
        value={edge.to}
        onChange={(event) => onChange(patchEdge(workflow, edge.id, { to: event.target.value }))}
      >
        {workflow.nodes.map((node) => (
          <option key={node.id} value={node.id}>
            {node.id === edge.from ? "This step again" : node.name || "Unnamed step"}
          </option>
        ))}
      </Select>
    </Field>
  );
  return (
    <div className="space-y-4">
      {isStartEdge(edge) ? (
        <p className="text-sm">
          A run starts with <strong className="font-medium">{name(edge.to)}</strong>.
        </p>
      ) : (
        <>
          <p className="flex items-baseline gap-2 text-sm">
            <span
              className="h-2.5 w-2.5 shrink-0 rounded-full"
              style={{ background: port ? portColor(port) : undefined }}
              aria-hidden
            />
            <span>
              After <strong className="font-medium">{name(edge.from)}</strong>, continue with{" "}
              <strong className="font-medium">{name(edge.to)}</strong> {conditionPhrase(edge)}.
            </span>
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
      {target}
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

const isTyping = (target: EventTarget | null) =>
  target instanceof HTMLElement && (target.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName));

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
  const [full, setFull] = useState(false);
  // Phones start with the list: dragging lines between cards needs room.
  useEffect(() => {
    if (window.matchMedia("(max-width: 639px)").matches) setView("list");
  }, []);

  // Undo and redo. Changes that follow each other quickly (typing a name) are one step.
  const history = useRef<{ past: Workflow[]; future: Workflow[]; at: number }>({ past: [], future: [], at: 0 });
  const current = useRef(value);
  current.current = value;
  const [, redraw] = useState(0);
  const change = useCallback(
    (next: Workflow) => {
      const log = history.current;
      const now = Date.now();
      if (now - log.at > 700) {
        log.past.push(current.current);
        if (log.past.length > 80) log.past.shift();
      }
      log.at = now;
      log.future = [];
      onChange(next);
      redraw((count) => count + 1);
    },
    [onChange],
  );
  const step = useCallback(
    (from: "past" | "future") => {
      const log = history.current;
      const next = log[from].pop();
      if (!next) return;
      log[from === "past" ? "future" : "past"].push(current.current);
      log.at = 0;
      onChange(next);
      setFitKey((key) => key + 1);
      redraw((count) => count + 1);
    },
    [onChange],
  );
  // On the document: after a step or line is removed, nothing inside the editor has the
  // focus any more, and that is exactly when Undo is wanted.
  useEffect(() => {
    const onKeyDown = (event: globalThis.KeyboardEvent) => {
      if (!(event.metaKey || event.ctrlKey) || event.altKey || isTyping(event.target)) return;
      const key = event.key.toLowerCase();
      if (key !== "z" && key !== "y") return;
      event.preventDefault();
      step(key === "y" || event.shiftKey ? "future" : "past");
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [step]);

  const names = useMemo(() => new Map(agents.map((agent) => [agent.id, agent.name])), [agents]);
  const agentName = useCallback((id: string) => names.get(id) ?? "Unavailable agent", [names]);
  const loops = useMemo(() => loopSteps(value), [value]);
  const problems = useMemo(
    () => new Set(issues.flatMap((issue) => (issue.nodeId ? [issue.nodeId] : []))),
    [issues],
  );
  const arranged = isArranged(value);

  const selectedNode = selection?.kind === "node" ? value.nodes.find((node) => node.id === selection.id) : undefined;
  const selectedEdge = selection?.kind === "edge" ? value.edges.find((edge) => edge.id === selection.id) : undefined;
  const confirm = useConfirm();
  const stepName = (id: string) =>
    id === WORKFLOW_START ? "Start" : value.nodes.find((node) => node.id === id)?.name || "an unnamed step";
  const announcement = selectedNode
    ? `Step ${selectedNode.name || "without a name"} selected.`
    : selectedEdge
      ? `Line from ${stepName(selectedEdge.from)} to ${stepName(selectedEdge.to)} selected.`
      : "";

  // A new step continues from the selected step, or from the end of the line when the
  // workflow has one. Then its fields come into view, ready for a name.
  const [opening, setOpening] = useState<string | null>(null);
  const panel = useRef<HTMLDivElement>(null);
  const open = (id: string) => {
    setSelection({ kind: "node", id });
    setOpening(id);
  };
  const atLimit = value.nodes.length >= MAX_STEPS;
  const add = () => {
    const result = addStep(value, selectedNode?.id ?? lastStep(value));
    change(result.workflow);
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
    if (view === "canvas" && !full && top > wanted) window.scrollBy({ top: top - wanted, behavior: scrollBehavior() });
    input.focus({ preventScroll: true });
    if (view === "list" || full) input.scrollIntoView({ block: "nearest", behavior: scrollBehavior() });
    input.select();
    setOpening(null);
  }, [opening, selectedNode, view, full]);

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
      <div className="flex items-center">
        <Button
          type="button"
          size="icon-sm"
          variant="ghost"
          aria-label="Undo"
          title="Undo (Ctrl+Z)"
          disabled={!history.current.past.length}
          onClick={() => step("past")}
        >
          <Undo2 />
        </Button>
        <Button
          type="button"
          size="icon-sm"
          variant="ghost"
          aria-label="Redo"
          title="Redo (Ctrl+Shift+Z)"
          disabled={!history.current.future.length}
          onClick={() => step("future")}
        >
          <Redo2 />
        </Button>
      </div>
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
                onSelect={async () => {
                  if (
                    value.nodes.length &&
                    !(await confirm({
                      title: "Replace the current workflow?",
                      description: `The steps and lines are replaced by the "${example.title}" example. You choose the agents afterwards; Undo brings the current workflow back.`,
                      confirmLabel: "Replace workflow",
                      cancelLabel: "Keep mine",
                    }))
                  )
                    return;
                  change(example.build());
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
          <Button
            type="button"
            size="sm"
            variant="ghost"
            title={arranged ? "The steps are arranged for you" : "Arrange the steps again and keep them arranged"}
            onClick={() => {
              change(tidy(value));
              setFitKey((key) => key + 1);
            }}
            disabled={!value.nodes.length || arranged}
          >
            <LayoutGrid /> Tidy up
          </Button>
        ) : null}
        <Button type="button" size="sm" variant="ghost" onClick={() => setFull(!full)}>
          {full ? <Minimize2 /> : <Maximize2 />} {full ? "Exit full screen" : "Full screen"}
        </Button>
        <Button type="button" size="sm" variant="outline" onClick={add} disabled={atLimit}>
          <Plus /> Add step
        </Button>
      </div>
    </div>
  );

  const list = value.nodes.length ? (
    <ol className="space-y-2">
      {value.nodes.map((node, index) => {
        const next = edgesFrom(value, node.id);
        const before = edgesInto(value, node.id).filter((edge) => !isStartEdge(edge));
        const nameOf = (id: string) => value.nodes.find((item) => item.id === id)?.name || "?";
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
                [startsRun(value, node.id) ? "starts the run" : null, before.length ? `after ${before.map((edge) => nameOf(edge.from)).join(", ")}` : null]
                  .filter(Boolean)
                  .join(", ") || "no input yet",
                next.length ? `then ${next.map((edge) => nameOf(edge.to)).join(", ")}` : "last step",
              ].join(" · ")}
              open={selection?.kind === "node" && selection.id === node.id}
              onOpenChange={(open) => setSelection(open ? { kind: "node", id: node.id } : null)}
            >
              <StepFields
                workflow={value}
                node={node}
                agents={agents}
                onChange={change}
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
  );

  const canvas = (
    <WorkflowCanvas
      label="Workflow canvas"
      className={full ? "h-full min-h-[18rem]" : "h-[28rem] min-[1700px]:h-[36rem]"}
      workflow={value}
      agentName={agentName}
      selection={selection}
      onSelect={setSelection}
      onOpen={open}
      problems={problems}
      fitKey={fitKey}
      wheelZoom={full}
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
            <Button type="button" size="sm" variant="outline" onClick={() => change(WORKFLOW_EXAMPLES[0].build())}>
              <Sparkles /> Use &ldquo;{WORKFLOW_EXAMPLES[0].title}&rdquo;
            </Button>
          </div>
        </div>
      }
      // Dragging one step means the user arranges the workflow from here on: every step
      // keeps the place it has now.
      onMove={(moves) =>
        change(apart(moves.reduce((next, move) => patchStep(next, move.id, { position: move.position }), withPositions(value))))
      }
      onConnect={(from, to, port) => {
        const condition = from === WORKFLOW_START ? { when: "always" as const, outcome: null } : conditionOfPort(port);
        const result = connect(value, from, to, condition.when, condition.outcome);
        change(result.workflow);
        if (result.id) setSelection({ kind: "edge", id: result.id });
      }}
      onAddFrom={(from, port, at) => {
        if (atLimit) return;
        const result = addStep(value, from, { port: port ?? undefined, at: arranged ? undefined : at });
        change(result.workflow);
        open(result.id);
      }}
      onRemove={(removed) => {
        let next = removed.edges.reduce(removeEdge, value);
        next = removed.nodes.reduce(removeStep, next);
        change(next);
        setSelection(null);
      }}
    />
  );

  const help = (
    <div className="space-y-1.5">
      <WorkflowLegend workflow={value} />
      <p className="text-xs text-muted-foreground">
        Drag from an output (the dot on the right of a step) to the step that runs next, or let go over empty canvas
        to add one. Select a step to see all its outputs and edit it. Backspace removes what is selected.{" "}
        {arranged
          ? "The steps are arranged for you; drag one to place them yourself."
          : "You placed the steps; Tidy up arranges them again."}
      </p>
    </div>
  );

  const inspector = (
    <div ref={panel} className="scroll-mb-24 rounded-lg border bg-card p-4">
      {/* One short line for screen readers instead of reading the whole form (#448). */}
      <p className="sr-only" aria-live="polite">
        {announcement}
      </p>
      {selectedNode ? (
        <>
          <h3 className="mb-3 text-sm font-semibold">Step</h3>
          <StepFields
            key={selectedNode.id}
            workflow={value}
            node={selectedNode}
            agents={agents}
            onChange={change}
            onSelect={setSelection}
            inLoop={loops.has(selectedNode.id)}
          />
        </>
      ) : selectedEdge ? (
        <>
          <h3 className="mb-3 text-sm font-semibold">Line</h3>
          <LineFields key={selectedEdge.id} workflow={value} edge={selectedEdge} onChange={change} onSelect={setSelection} />
        </>
      ) : (
        <div className="space-y-3 text-[13px] text-muted-foreground">
          <h3 className="text-sm font-semibold text-foreground">How a workflow runs</h3>
          <ul className="list-disc space-y-1.5 pl-4">
            <li>Each step runs one agent. When it finishes, every line whose condition matches hands the work on.</li>
            <li>Two lines that match at once start two steps that work at the same time.</li>
            <li>A step with several inputs waits until no earlier step is still working.</li>
            <li>A line back to an earlier step makes a loop. Each step has a limit on how often it runs.</li>
          </ul>
          <p>Select a step or a line to edit it.</p>
        </div>
      )}
    </div>
  );

  if (full) {
    return (
      <>
        <button
          type="button"
          className="w-full rounded-lg border border-dashed p-8 text-center text-sm text-muted-foreground"
          onClick={() => setFull(false)}
        >
          The workflow is open in full screen. Select to bring it back here.
        </button>
        <Dialog open onOpenChange={setFull}>
          <DialogContent
            hideClose
            className="flex h-[100dvh] max-h-[100dvh] w-screen max-w-none flex-col gap-3 rounded-none border-0 bg-background p-3 sm:p-4"
          >
            <DialogTitle className="sr-only">Workflow editor</DialogTitle>
            <DialogDescription className="sr-only">
              Edit the workflow with the whole screen. Changes are kept when you leave full screen; save the team to
              store them.
            </DialogDescription>
            {toolbar}
            {view === "list" ? (
              <div className="min-h-0 flex-1 overflow-y-auto">{list}</div>
            ) : (
              <div className="flex min-h-0 flex-1 flex-col gap-3 lg:grid lg:grid-cols-[minmax(0,1fr)_25rem]">
                <div className="flex min-h-0 flex-col gap-2 max-lg:h-[52dvh] max-lg:shrink-0">
                  <div className="min-h-0 flex-1">{canvas}</div>
                  {help}
                </div>
                <div className="min-h-0 flex-1 overflow-y-auto">{inspector}</div>
              </div>
            )}
          </DialogContent>
        </Dialog>
      </>
    );
  }

  return (
    <div className="space-y-3">
      {toolbar}
      {view === "list" ? (
        list
      ) : (
        <div className="grid gap-3 min-[1700px]:grid-cols-[minmax(0,1fr)_25rem] min-[1700px]:items-start">
          <div className="space-y-2">
            {canvas}
            {help}
          </div>
          {inspector}
        </div>
      )}
    </div>
  );
}
