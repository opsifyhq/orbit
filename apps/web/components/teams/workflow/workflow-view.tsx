"use client";

import { useCallback, useMemo, useState } from "react";

import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Disclosure } from "@/components/ui/disclosure";
import {
  conditionPhrase,
  edgesFrom,
  limitReached,
  liveEdges,
  nodeRunStates,
  portOf,
  startsRun,
  stepPorts,
  takenEdges,
  type Workflow,
  type WorkflowStepRun,
} from "@/lib/workflow";

import { WorkflowCanvas, WorkflowLegend, portColor, type WorkflowSelection } from "./workflow-canvas";

/**
 * A workflow to look at: on the team page as it is defined, on a run page with how each
 * step went and which lines the run followed. The same content is available as text.
 */
export function WorkflowView({
  workflow,
  agentName,
  steps,
  failed,
  onSelectStep,
  className,
}: {
  workflow: Workflow;
  agentName: (id: string) => string;
  /** The steps of a run, to show its progress on the graph. */
  steps?: WorkflowStepRun[];
  /** The run failed: mark a step that had used all its runs when it was due again. */
  failed?: boolean;
  /** A step that ran was clicked; called with the index of its latest run. */
  onSelectStep?: (stepIndex: number) => void;
  className?: string;
}) {
  const name = useCallback((id: string) => agentName(id), [agentName]);
  const runStates = useMemo(() => {
    if (!steps) return undefined;
    const states = nodeRunStates(steps);
    if (failed) {
      for (const id of limitReached(workflow, steps)) {
        const state = states.get(id);
        if (state) states.set(id, { ...state, limitReached: true });
      }
    }
    return states;
  }, [steps, failed, workflow]);
  const taken = useMemo(() => (steps ? takenEdges(steps) : undefined), [steps]);
  const live = useMemo(() => (steps ? liveEdges(steps) : undefined), [steps]);
  const stepName = (id: string) => workflow.nodes.find((node) => node.id === id)?.name ?? id;
  const [expanded, setExpanded] = useState(false);
  const select = onSelectStep
    ? (selection: WorkflowSelection) => {
        const state = selection?.kind === "node" ? runStates?.get(selection.id) : undefined;
        if (!state) return;
        setExpanded(false);
        onSelectStep(state.stepIndex);
      }
    : undefined;
  const title = steps ? "Workflow progress" : "Workflow";

  return (
    <div className="space-y-3">
      <WorkflowCanvas
        label={title}
        className={className}
        autoHeight={!className}
        workflow={workflow}
        agentName={name}
        runStates={runStates}
        onSelect={select}
        taken={taken}
        live={live}
        onExpand={() => setExpanded(true)}
      />
      <WorkflowLegend workflow={workflow} />
      <Dialog open={expanded} onOpenChange={setExpanded}>
        <DialogContent className="flex h-[92dvh] w-[96vw] max-w-[96vw] flex-col gap-3 sm:max-w-[96vw]">
          <DialogHeader>
            <DialogTitle>{title}</DialogTitle>
            <DialogDescription>
              The whole workflow. Zoom with the buttons in the corner and drag to move around.
              {onSelectStep ? " Select a step to find it in the list of steps." : ""}
            </DialogDescription>
          </DialogHeader>
          {expanded ? (
            <WorkflowCanvas
              label={`${title}, enlarged`}
              className="h-auto min-h-0 flex-1"
              showAll
              workflow={workflow}
              agentName={name}
              runStates={runStates}
              onSelect={select}
              taken={taken}
              live={live}
            />
          ) : null}
        </DialogContent>
      </Dialog>
      <Disclosure
        title="Steps as text"
        // On a phone the picture is an overview; the text is how the workflow is read.
        defaultOpen={typeof window !== "undefined" && window.matchMedia("(max-width: 639px)").matches}
        summary={`${workflow.nodes.length} ${workflow.nodes.length === 1 ? "step" : "steps"}`}>
        <ol className="space-y-2.5 text-sm">
          {workflow.nodes.map((node) => {
            const lines = edgesFrom(workflow, node.id);
            const state = runStates?.get(node.id);
            return (
              <li key={node.id}>
                <p>
                  <span className="font-medium">{node.name}</span>{" "}
                  <span className="text-muted-foreground">
                    · {agentName(node.agentId)}
                    {startsRun(workflow, node.id) ? " · starts the run" : ""}
                    {state
                      ? ` · ${state.limitReached ? "reached its limit" : state.status}${state.runs > 1 ? `, ran ${state.runs} times` : ""}`
                      : steps
                        ? " · did not run"
                        : ""}
                  </span>
                </p>
                {lines.length ? (
                  <ul className="mt-1 space-y-0.5 text-[13px] text-muted-foreground">
                    {lines.map((edge) => {
                      const port = stepPorts(workflow, node).find((item) => item.id === portOf(edge));
                      return (
                        <li key={edge.id} className="flex items-baseline gap-2">
                          <span
                            className="h-2 w-2 shrink-0 translate-y-[-1px] rounded-full"
                            style={{ background: port ? portColor(port) : undefined }}
                            aria-hidden
                          />
                          <span>
                            Then {stepName(edge.to)}, {conditionPhrase(edge)}
                          </span>
                        </li>
                      );
                    })}
                  </ul>
                ) : null}
              </li>
            );
          })}
        </ol>
      </Disclosure>
    </div>
  );
}
