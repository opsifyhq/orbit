"use client";

import { memo, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import {
  Background,
  BackgroundVariant,
  BaseEdge,
  ControlButton,
  Controls,
  EdgeLabelRenderer,
  Handle,
  MarkerType,
  MiniMap,
  Position,
  ReactFlow,
  getBezierPath,
  useNodesState,
  useUpdateNodeInternals,
  type Connection,
  type Edge,
  type EdgeProps,
  type Node,
  type NodeProps,
  type ReactFlowInstance,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { AlertTriangle, Ban, Bot, Check, Loader2, Maximize, Maximize2, Play, Repeat, X } from "lucide-react";

import {
  DEFAULT_MAX_RUNS,
  HANDLE_Y,
  NODE_HEIGHT,
  NODE_WIDTH,
  PORT_ROW,
  WORKFLOW_START,
  isStartEdge,
  loopSteps,
  portOf,
  portRows,
  portY,
  stepHeight,
  stepPorts,
  withPositions,
  type NodeRunState,
  type Workflow,
  type WorkflowPort,
} from "@/lib/workflow";
import { cn } from "@/lib/utils";
import { prefersReducedMotion } from "@/lib/motion";

export type WorkflowSelection = { kind: "node" | "edge"; id: string } | null;

/** How far left of the first step Start sits. */
const START_OFFSET = 116;
const START_WIDTH = 76;
const START_COLOR = "hsl(var(--muted-foreground))";
const FLOW_COLORS = 6;

/**
 * The colour of an output and of the lines that leave it: green for done, red for
 * failed, grey for always, and one colour per outcome so a branch can be followed by eye.
 */
export function portColor(port: Pick<WorkflowPort, "when" | "index">): string {
  switch (port.when) {
    case "outcome":
      return `hsl(var(--flow-${((port.index ?? 0) % FLOW_COLORS) + 1}))`;
    case "failure":
      return "hsl(var(--destructive))";
    case "always":
      return "hsl(var(--muted-foreground))";
    default:
      return "hsl(var(--success))";
  }
}

interface PortData extends WorkflowPort {
  color: string;
  /** A line leaves this output. */
  connected: boolean;
  /** The run shown reported this outcome. */
  reported: boolean;
}

interface StepData extends Record<string, unknown> {
  name: string;
  agent: string | null;
  ports: PortData[];
  /** Rows of outputs under the name; none for a step that only has "done". */
  rows: number;
  /** Outputs no line uses yet, offered under the selected step of an editor. */
  extra: PortData[];
  /** Run limit, when the step sits on a loop. */
  loopLimit: number | null;
  problem: boolean;
  readOnly: boolean;
  run?: NodeRunState;
  /** A run is shown and this step did not run in it. */
  idle: boolean;
}

interface LineData extends Record<string, unknown> {
  color: string;
  taken: number;
  /** A run is shown and it did not follow this line. */
  idle: boolean;
  live: boolean;
  /** Another step is selected: this line steps back. */
  dim: boolean;
  /** The step at one end is pointed at or selected. */
  strong: boolean;
  /**
   * How a line that cannot go straight is drawn: `under` back beneath the steps in
   * between and up into the step it returns to, `lane` forward past a step that is in
   * the way.
   */
  route?: "under" | "lane";
  /** Where an `under` or `lane` line runs. */
  lane?: number;
  /** Lines into the same step arrive next to each other, not on top of each other. */
  spread: number;
}

const hiddenHandle = "!pointer-events-none !h-px !w-px !min-w-0 !min-h-0 !border-0 !opacity-0";
const BACK_IN = "back-in";

// `orbit-handle` widens the hit area to 32px without changing the 12px dot (globals.css),
// the touch-target size the design system asks for (#448).
const inputClass =
  "orbit-handle !h-3 !w-3 !rounded-full !border-2 !border-border-strong !bg-card transition-[border-color,box-shadow] hover:!border-primary";

function OutputHandle({ port, readOnly, top }: { port: PortData; readOnly: boolean; top?: number }) {
  return (
    <Handle
      id={port.id}
      type="source"
      position={Position.Right}
      isConnectable={!readOnly}
      title={readOnly ? undefined : `Drag to the step that runs next (${port.label})`}
      style={{
        top,
        borderColor: port.color,
        background: port.connected ? port.color : "hsl(var(--card))",
      }}
      className={cn(
        "!rounded-full !border-2 transition-[box-shadow,transform]",
        readOnly ? "!h-2 !w-2" : "orbit-handle !h-3 !w-3 hover:!shadow-[0_0_0_4px_hsl(var(--primary)/0.25)]",
      )}
    />
  );
}

const StepNode = memo(function StepNode({ id, data, selected }: NodeProps<Node<StepData>>) {
  const run = data.run;
  // React Flow measures where the outputs are; tell it when they change.
  const update = useUpdateNodeInternals();
  const portKey = [...data.ports, ...data.extra].map((port) => port.id).join("|");
  useEffect(() => update(id), [update, id, portKey]);
  return (
    <div
      style={{ width: NODE_WIDTH }}
      className={cn(
        "relative rounded-xl border bg-card text-left shadow-sm transition-shadow",
        selected ? "border-primary ring-2 ring-primary/40" : "hover:border-border-strong",
        run?.status === "running" && "border-primary",
        run?.status === "succeeded" && "border-success/60",
        run?.status === "failed" && "border-destructive/70",
        run?.status === "stopped" && "border-border-strong",
        run?.limitReached && "border-destructive/70",
        data.idle && "border-dashed opacity-60 shadow-none",
      )}
    >
      <Handle
        type="target"
        position={Position.Left}
        style={{ top: HANDLE_Y }}
        isConnectable={!data.readOnly}
        className={cn(inputClass, data.readOnly && "!opacity-0")}
      />
      <div className="flex flex-col justify-center gap-0.5 px-3" style={{ height: NODE_HEIGHT }}>
        <div className="flex items-center gap-2">
          <p className="min-w-0 flex-1 truncate text-sm font-medium leading-5">{data.name || "Unnamed step"}</p>
          {run ? (
            <span
              className={cn(
                "flex h-4 shrink-0 items-center gap-1 text-[11px] font-medium",
                run.status === "running" && "text-primary",
                run.status === "succeeded" && "text-success",
                run.status === "failed" && "text-destructive",
                run.status === "stopped" && "text-muted-foreground",
                run.limitReached && "text-destructive",
              )}
            >
              {run.runs > 1 ? <span className="tabular">{run.runs}×</span> : null}
              {run.limitReached ? (
                <Repeat className="h-3.5 w-3.5" aria-label="Reached its run limit" />
              ) : run.status === "running" ? (
                <Loader2 className="h-3.5 w-3.5 animate-spin" aria-label="Running" />
              ) : run.status === "succeeded" ? (
                <Check className="h-3.5 w-3.5" aria-label="Succeeded" />
              ) : run.status === "stopped" ? (
                <Ban className="h-3.5 w-3.5" aria-label="Stopped" />
              ) : (
                <X className="h-3.5 w-3.5" aria-label="Failed" />
              )}
            </span>
          ) : data.problem ? (
            <AlertTriangle className="h-3.5 w-3.5 shrink-0 text-warning" aria-label="Needs attention" />
          ) : null}
        </div>
        <div className={cn("flex items-center gap-1.5 text-xs", data.agent ? "text-muted-foreground" : "text-warning")}>
          <Bot className="h-3.5 w-3.5 shrink-0" aria-hidden />
          <span className="min-w-0 flex-1 truncate">{data.agent ?? "Choose an agent"}</span>
          {run?.limitReached ? (
            <span className="shrink-0 rounded-full bg-destructive/10 px-1.5 py-px text-[11px] font-medium text-destructive">
              limit reached
            </span>
          ) : data.loopLimit ? (
            <span
              className="flex shrink-0 items-center gap-0.5 rounded-full border px-1.5 py-px text-[11px] text-muted-foreground"
              title={`On a loop: runs at most ${data.loopLimit} times in one run`}
            >
              <Repeat className="h-3 w-3" aria-hidden /> max {data.loopLimit}
            </span>
          ) : null}
        </div>
      </div>
      {data.rows ? (
        <ul className="border-t py-1" aria-label="Outputs">
          {data.ports.map((port) => (
            <li
              key={port.id}
              className="relative flex items-center justify-end pl-3 pr-3 text-[11px] leading-none"
              style={{ height: PORT_ROW }}
            >
              <span
                className={cn(
                  "max-w-full truncate rounded-full px-1.5 py-0.5",
                  port.reported ? "font-semibold text-foreground" : port.connected ? "text-foreground" : "text-muted-foreground",
                )}
                style={port.reported ? { background: `color-mix(in srgb, ${port.color} 22%, transparent)` } : undefined}
              >
                {port.label}
              </span>
              <OutputHandle port={port} readOnly={data.readOnly} />
            </li>
          ))}
        </ul>
      ) : (
        <OutputHandle port={data.ports[0]} readOnly={data.readOnly} top={HANDLE_Y} />
      )}
      {/* The other outputs float under the card, so selecting a step moves nothing. */}
      {data.extra.length ? (
        <ul
          className="absolute left-6 right-0 top-full z-10 mt-1.5 rounded-lg border bg-card py-1 shadow-md"
          aria-label="More outputs"
        >
          {data.extra.map((port) => (
            <li
              key={port.id}
              className="relative flex items-center justify-end pl-3 pr-3 text-[11px] leading-none text-muted-foreground"
              style={{ height: PORT_ROW }}
            >
              <span className="max-w-full truncate px-1.5 py-0.5">{port.label}</span>
              <OutputHandle port={port} readOnly={data.readOnly} />
            </li>
          ))}
        </ul>
      ) : null}
      {/* Where lines that point back arrive; never shown. */}
      <Handle id={BACK_IN} type="target" position={Position.Bottom} isConnectable={false} className={hiddenHandle} />
    </div>
  );
});

const StartNode = memo(function StartNode({ data }: NodeProps<Node<{ readOnly: boolean }>>) {
  return (
    <div className="flex h-9 items-center gap-1.5 rounded-full border bg-subtle px-3 text-xs font-medium text-muted-foreground">
      <Play className="h-3 w-3" aria-hidden /> Start
      <Handle
        type="source"
        position={Position.Right}
        isConnectable={!data.readOnly}
        title={data.readOnly ? undefined : "Drag to a step that starts the run"}
        style={{ borderColor: START_COLOR, background: START_COLOR }}
        className={cn("!rounded-full !border-2", data.readOnly ? "!h-2 !w-2" : "orbit-handle !h-3 !w-3")}
      />
    </div>
  );
});

const LineEdge = memo(function LineEdge({
  id,
  sourceX,
  sourceY,
  targetX,
  targetY,
  sourcePosition,
  targetPosition,
  data,
  selected,
  markerEnd,
}: EdgeProps<Edge<LineData>>) {
  const route = data?.route;
  const spread = data?.spread ?? 0;
  const [path, labelX, labelY] =
    route === "under"
      ? underPath(sourceX, sourceY, targetX + spread, targetY, data?.lane ?? 0)
      : route === "lane"
        ? lanePath(sourceX, sourceY, targetX, targetY + spread, data?.lane ?? 0)
        : straightPath(sourceX, sourceY, targetX, targetY + spread, sourcePosition, targetPosition);
  const color = data?.color ?? START_COLOR;
  return (
    <>
      <BaseEdge
        id={id}
        path={path}
        markerEnd={markerEnd}
        interactionWidth={24}
        style={{
          stroke: color,
          strokeWidth: selected ? 2.75 : data?.strong ? 2.25 : 1.75,
          strokeDasharray: data?.idle ? "4 4" : data?.live ? "6 4" : undefined,
          opacity: data?.idle ? 0.35 : data?.dim ? 0.3 : 1,
          transition: "opacity 120ms",
          animation: data?.live ? "orbit-line-flow 0.6s linear infinite" : undefined,
        }}
      />
      {data && data.taken > 1 ? (
        <EdgeLabelRenderer>
          <span
            style={{ transform: `translate(-50%, -50%) translate(${labelX}px, ${labelY}px)`, color }}
            className="nodrag nopan pointer-events-none absolute rounded-full border bg-card px-1.5 py-px text-[11px] font-semibold leading-4 shadow-sm"
            title={`Followed ${data.taken} times`}
          >
            <span className="tabular">{data.taken}×</span>
          </span>
        </EdgeLabelRenderer>
      ) : null}
    </>
  );
});

const CORNER = 12;
/** How far a line runs straight out of an output before it turns. */
const STUB = 26;

function straightPath(
  sx: number,
  sy: number,
  tx: number,
  ty: number,
  sourcePosition: Position,
  targetPosition: Position,
): [string, number, number] {
  const [path, x, y] = getBezierPath({ sourceX: sx, sourceY: sy, sourcePosition, targetX: tx, targetY: ty, targetPosition });
  return [path, x, y];
}

/**
 * Along a free lane at height `y`: out of the output, up or down to the lane, along it
 * (forward past the steps in the way, or back when the next step is not to the right)
 * and into the next step from the left.
 */
function lanePath(sx: number, sy: number, tx: number, ty: number, y: number): [string, number, number] {
  const x1 = sx + STUB;
  const x2 = tx - STUB;
  const d1 = y >= sy ? 1 : -1;
  const d2 = ty >= y ? 1 : -1;
  const r1 = Math.min(CORNER, Math.abs(y - sy) / 2);
  const r2 = Math.min(CORNER, Math.abs(ty - y) / 2);
  const dir = x2 >= x1 ? 1 : -1;
  const path = [
    `M ${sx} ${sy}`,
    `L ${x1 - r1} ${sy}`,
    `Q ${x1} ${sy} ${x1} ${sy + d1 * r1}`,
    `L ${x1} ${y - d1 * r1}`,
    `Q ${x1} ${y} ${x1 + dir * r1} ${y}`,
    `L ${x2 - dir * r2} ${y}`,
    `Q ${x2} ${y} ${x2} ${y + d2 * r2}`,
    `L ${x2} ${ty - d2 * r2}`,
    `Q ${x2} ${ty} ${x2 + r2} ${ty}`,
    `L ${tx} ${ty}`,
  ].join(" ");
  return [path, (x1 + x2) / 2, y];
}

/** Back underneath: out of the output, down to a lane below the steps in between, and up. */
function underPath(sx: number, sy: number, tx: number, ty: number, lane: number): [string, number, number] {
  const y = Math.max(lane, sy + 2 * CORNER, ty + 2 * CORNER);
  const x1 = sx + STUB;
  const r = CORNER;
  const path = [
    `M ${sx} ${sy}`,
    `L ${x1 - r} ${sy}`,
    `Q ${x1} ${sy} ${x1} ${sy + r}`,
    `L ${x1} ${y - r}`,
    `Q ${x1} ${y} ${x1 - r} ${y}`,
    `L ${tx + r} ${y}`,
    `Q ${tx} ${y} ${tx} ${y - r}`,
    `L ${tx} ${ty}`,
  ].join(" ");
  return [path, (x1 + tx) / 2, y];
}

/** The whole workflow in view, however large it is: an overview rather than something to read. */
const MIN_ZOOM = 0.12;
/** A workflow that is only looked at is never drawn smaller than this; it pans instead. */
const READ_ZOOM = 0.7;
/** An editor inside a page keeps its steps readable too; with the screen to itself it shows everything. */
const EDIT_ZOOM = 0.6;
/** Room under a view that pans, for its small map. */
const MAP_STRIP = 104;
/** Screen pixels along the top of a view, kept free for its buttons. */
const CONTROLS_STRIP = 40;
/** A canvas narrower than this is a phone. */
const NARROW = 520;
const MIN_HEIGHT = 150;
const MAX_HEIGHT = 560;

const NODE_TYPES = { step: StepNode, start: StartNode };
const EDGE_TYPES = { line: LineEdge };

function useIsDark(): boolean {
  const [dark, setDark] = useState(false);
  useEffect(() => {
    const root = document.documentElement;
    const sync = () => setDark(root.classList.contains("dark"));
    sync();
    const observer = new MutationObserver(sync);
    observer.observe(root, { attributes: true, attributeFilter: ["class"] });
    return () => observer.disconnect();
  }, []);
  return dark;
}

type Rect = { l: number; r: number; t: number; b: number };

/** A point of the curve a straight line is drawn as (the same one React Flow draws). */
function curvePoint(sx: number, sy: number, tx: number, ty: number, at: number): [number, number] {
  const bend = Math.max(0, tx - sx) / 2;
  const c1 = sx + bend;
  const c2 = tx - bend;
  const u = 1 - at;
  return [
    u * u * u * sx + 3 * u * u * at * c1 + 3 * u * at * at * c2 + at * at * at * tx,
    u * u * u * sy + 3 * u * u * at * sy + 3 * u * at * at * ty + at * at * at * ty,
  ];
}

/**
 * What the colours of the lines mean, for under a canvas. Lists only what the workflow
 * uses.
 */
export function WorkflowLegend({ workflow, className }: { workflow: Workflow; className?: string }) {
  const used = new Set(workflow.edges.filter((edge) => !isStartEdge(edge)).map((edge) => edge.when ?? "success"));
  const items: Array<{ label: string; colors: string[] }> = [];
  if (used.has("success")) items.push({ label: "when the step is done", colors: [portColor({ when: "success" })] });
  if (used.has("outcome"))
    items.push({
      label: "when it reports that outcome",
      colors: [0, 1, 2].map((index) => portColor({ when: "outcome", index })),
    });
  if (used.has("failure")) items.push({ label: "when the step fails", colors: [portColor({ when: "failure" })] });
  if (used.has("always")) items.push({ label: "whatever the result", colors: [portColor({ when: "always" })] });
  if (!items.length) return null;
  return (
    <ul className={cn("flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-muted-foreground", className)} aria-label="What the line colours mean">
      {items.map((item) => (
        <li key={item.label} className="flex items-center gap-1.5">
          <span className="flex items-center gap-0.5" aria-hidden>
            {item.colors.map((color) => (
              <span key={color} className="h-0.5 w-3 rounded-full" style={{ background: color }} />
            ))}
          </span>
          {item.label}
        </li>
      ))}
    </ul>
  );
}

/**
 * The workflow as a picture: steps as cards with their outputs, lines in the colour of
 * the output they leave. With `onChange…` callbacks it is an editor (drag to move, drag
 * from an output to connect, Backspace to remove); without them it is a read-only view,
 * optionally of one run.
 */
export function WorkflowCanvas({
  workflow,
  agentName,
  selection,
  onSelect,
  onOpen,
  onMove,
  onConnect,
  onAddFrom,
  onRemove,
  problems,
  runStates,
  taken,
  live,
  className,
  label,
  empty,
  fitKey,
  autoHeight,
  showAll,
  wheelZoom,
  onExpand,
}: {
  workflow: Workflow;
  agentName: (id: string) => string | null;
  selection?: WorkflowSelection;
  onSelect?: (selection: WorkflowSelection) => void;
  /** A step was double-clicked: the user wants to edit it. */
  onOpen?: (id: string) => void;
  /** Steps were dragged to new places. */
  onMove?: (moves: Array<{ id: string; position: { x: number; y: number } }>) => void;
  /** A line was drawn from the output `port` of `from` (or from Start) to `to`. */
  onConnect?: (from: string, to: string, port: string | null) => void;
  /** A line was drawn from an output and let go over empty canvas: a new step goes there. */
  onAddFrom?: (from: string, port: string | null, at: { x: number; y: number }) => void;
  onRemove?: (removed: { nodes: string[]; edges: string[] }) => void;
  /** Steps to mark as needing attention. */
  problems?: Set<string>;
  /** Show a run: how each step went and which lines it followed. */
  runStates?: Map<string, NodeRunState>;
  taken?: Map<string, number>;
  /** Lines that lead into a step that is working now. */
  live?: Set<string>;
  className?: string;
  label: string;
  /** Shown over the canvas while the workflow has no steps. */
  empty?: ReactNode;
  /** Change it to bring the whole workflow back into view, after a tidy-up for example. */
  fitKey?: number;
  /** As tall as the workflow needs (within limits) instead of the height of `className`. */
  autoHeight?: boolean;
  /** A view that always shows the whole workflow, however small that makes it. */
  showAll?: boolean;
  /** The canvas has the screen to itself: the wheel zooms it instead of scrolling a page. */
  wheelZoom?: boolean;
  /** Adds a button that asks for a larger view of the workflow. */
  onExpand?: () => void;
}) {
  const dark = useIsDark();
  const readOnly = !onMove;
  const placed = useMemo(() => withPositions(workflow), [workflow]);
  const loops = useMemo(() => loopSteps(workflow), [workflow]);
  const showsRun = Boolean(runStates);
  const selectedNode = selection?.kind === "node" ? selection.id : null;
  const [hovered, setHovered] = useState<string | null>(null);

  // The outputs each card shows: the ones that matter. The selected step of an editor
  // offers the rest as well (see `extra`), so a "failed" or "always" line can be drawn.
  const shown = useMemo(() => new Map(placed.nodes.map((node) => [node.id, stepPorts(placed, node)])), [placed]);

  const built = useMemo<Node[]>(() => {
    const entries = placed.edges.filter(isStartEdge).map((edge) => edge.to);
    const anchors = placed.nodes.filter((node) => entries.includes(node.id));
    const left = Math.min(0, ...placed.nodes.map((node) => node.position?.x ?? 0));
    const middle = anchors.length
      ? anchors.reduce((sum, node) => sum + (node.position?.y ?? 0), 0) / anchors.length
      : (placed.nodes[0]?.position?.y ?? 0);
    const start: Node = {
      id: WORKFLOW_START,
      type: "start",
      position: { x: left - START_OFFSET, y: middle + HANDLE_Y - 18 },
      data: { readOnly },
      draggable: false,
      deletable: false,
      selectable: false,
    };
    const steps = placed.nodes.map<Node<StepData>>((node) => {
      const run = runStates?.get(node.id);
      const leaving = new Set(placed.edges.filter((edge) => edge.from === node.id).map(portOf));
      const ports = (shown.get(node.id) ?? []).map<PortData>((port) => ({
        ...port,
        color: portColor(port),
        connected: leaving.has(port.id),
        reported: Boolean(run?.outcome) && port.when === "outcome" && port.outcome === run?.outcome,
      }));
      const extra =
        !readOnly && node.id === selectedNode
          ? stepPorts(placed, node, true)
              .filter((port) => !ports.some((item) => item.id === port.id))
              .map<PortData>((port) => ({ ...port, color: portColor(port), connected: false, reported: false }))
          : [];
      return {
        id: node.id,
        type: "step",
        position: node.position ?? { x: 0, y: 0 },
        selected: node.id === selectedNode,
        draggable: !readOnly,
        deletable: !readOnly,
        data: {
          name: node.name,
          agent: node.agentId ? agentName(node.agentId) : null,
          ports,
          rows: portRows(ports),
          extra,
          loopLimit: loops.has(node.id) ? (node.maxRuns ?? DEFAULT_MAX_RUNS) : null,
          problem: Boolean(problems?.has(node.id)),
          readOnly,
          run,
          idle: showsRun && !run,
        },
      };
    });
    return [start, ...steps];
  }, [placed, shown, selectedNode, readOnly, agentName, loops, problems, runStates, showsRun]);

  // How lines that cannot go straight are drawn, and the room the whole picture takes.
  const geometry = useMemo(() => {
    const rects = new Map<string, Rect>();
    const rowsOf = new Map<string, number>();
    for (const node of placed.nodes) {
      const at = node.position ?? { x: 0, y: 0 };
      const rows = portRows(shown.get(node.id) ?? []);
      rowsOf.set(node.id, rows);
      rects.set(node.id, { l: at.x, r: at.x + NODE_WIDTH, t: at.y, b: at.y + stepHeight(rows) });
    }
    const start = built[0];
    const startY = start.position.y + 18;
    const routes = new Map<string, { route: "under" | "lane"; lane?: number }>();
    // Where each line arrives, to spread lines into the same step.
    const arrivals = new Map<string, Array<{ id: string; from: number }>>();
    const arrive = (key: string, id: string, from: number) => arrivals.set(key, [...(arrivals.get(key) ?? []), { id, from }]);
    let top = Math.min(start.position.y, ...[...rects.values()].map((rect) => rect.t)) - 24;
    let bottom = Math.max(start.position.y + 36, ...[...rects.values()].map((rect) => rect.b)) + 24;
    let under = 0;
    const lanes: number[] = [];
    const hits = (x1: number, y1: number, x2: number, y2: number, skip: string[], pad = 8) =>
      [...rects].filter(
        ([id, o]) => !skip.includes(id) && x2 > o.l - pad && x1 < o.r + pad && y2 > o.t - pad && y1 < o.b + pad,
      );
    const startRect: Rect = { l: start.position.x, r: start.position.x + START_WIDTH, t: start.position.y, b: start.position.y + 36 };
    let reach = -Infinity;
    /** A lane under every step between `from` and `to`, one for each line that needs it. */
    const below = (from: number, to: number) => {
      const between = [...rects.values()].filter((o) => o.r > from - 8 && o.l < to + 8);
      const lane = Math.max(...between.map((o) => o.b)) + 26 + under * 14;
      under += 1;
      bottom = Math.max(bottom, lane + 30);
      return lane;
    };
    for (const edge of placed.edges) {
      const t = rects.get(edge.to);
      if (!t) continue;
      const ty = t.t + HANDLE_Y;
      const fromStart = isStartEdge(edge);
      const s = fromStart ? startRect : rects.get(edge.from);
      if (!s) continue;
      const ports = shown.get(edge.from) ?? [];
      const sy = fromStart
        ? startY
        : s.t + portY(rowsOf.get(edge.from) ?? 0, ports.findIndex((port) => port.id === portOf(edge)));
      const skip = [edge.from, edge.to];
      if (edge.from === edge.to) {
        // Round to the step itself: a small loop under it.
        const lane = s.b + 18;
        routes.set(edge.id, { route: "under", lane });
        arrive(`${edge.to}|bottom`, edge.id, -lane);
        bottom = Math.max(bottom, lane + 20);
        reach = Math.max(reach, s.r + STUB);
        continue;
      }
      if (!fromStart && t.l < s.l - NODE_WIDTH / 2) {
        // Back to an earlier step: underneath, and up into it. When another step sits
        // under the one it returns to, it goes round and arrives from the left instead.
        const x = (t.l + t.r) / 2;
        const lane = below(Math.min(x, t.l - STUB), s.r + STUB);
        reach = Math.max(reach, s.r + STUB);
        if (hits(x - 6, t.b + 1, x + 6, lane, skip, 0).length) {
          routes.set(edge.id, { route: "lane", lane });
          arrive(`${edge.to}|left`, edge.id, lane);
        } else {
          routes.set(edge.id, { route: "under", lane });
          arrive(`${edge.to}|bottom`, edge.id, -lane);
        }
        continue;
      }
      arrive(`${edge.to}|left`, edge.id, sy);
      if (t.l - s.r < 2 * STUB) {
        // Not clearly to the right (stacked, or dragged close together): through the gap
        // between the two when there is one, otherwise underneath.
        if (fromStart) continue;
        const gap = t.t > s.b + 20 ? (s.b + t.t) / 2 : s.t > t.b + 20 ? (t.b + s.t) / 2 : null;
        const clear = gap !== null && !hits(Math.min(t.l - STUB, s.l), gap - 6, s.r + STUB, gap + 6, [], 0).length;
        routes.set(edge.id, { route: "lane", lane: clear ? (gap as number) : below(t.l - STUB, s.r + STUB) });
        reach = Math.max(reach, s.r + STUB);
        continue;
      }
      // Forward. A step in the way of the curve sends the line along a free lane.
      let blocked = false;
      for (let step = 1; step < 16 && !blocked; step += 1) {
        const [x, y] = curvePoint(s.r, sy, t.l, ty, step / 16);
        blocked = hits(x, y, x, y, skip, 6).length > 0;
      }
      if (!blocked) continue;
      const x1 = s.r + STUB;
      const x2 = t.l - STUB;
      const inWay = hits(x1, -Infinity, x2, Infinity, [], 0).map(([, rect]) => rect);
      const free = (y: number) => !inWay.some((o) => y > o.t - 14 && y < o.b + 14) && !lanes.some((used) => Math.abs(used - y) < 10);
      const middle = (sy + ty) / 2;
      const candidates = inWay.flatMap((o) => [o.t - 22, o.b + 22, o.t - 34, o.b + 34, o.t - 46, o.b + 46]).filter(free);
      if (!candidates.length) continue;
      const lane = candidates.reduce((best, y) => (Math.abs(y - middle) < Math.abs(best - middle) ? y : best));
      lanes.push(lane);
      routes.set(edge.id, { route: "lane", lane });
      top = Math.min(top, lane - 20);
      bottom = Math.max(bottom, lane + 20);
    }
    const spread = new Map<string, number>();
    for (const [key, list] of arrivals) {
      const step = key.endsWith("|left") ? Math.min(9, 36 / Math.max(1, list.length - 1)) : Math.min(16, 96 / Math.max(1, list.length - 1));
      [...list]
        .sort((a, b) => a.from - b.from)
        .forEach((item, index) => spread.set(item.id, Math.round((index - (list.length - 1) / 2) * step)));
    }
    const left = Math.min(...built.map((node) => node.position.x)) - 16;
    const right = Math.max(reach, ...built.map((node) => node.position.x + (node.type === "start" ? START_WIDTH : NODE_WIDTH))) + 16;
    return { routes, spread, left, right, top, bottom };
  }, [placed, shown, built]);

  // Dragging moves the local copy; the position is saved when the drag ends, so the
  // form around the canvas does not re-render on every pixel.
  // A re-render from outside (a query refetch, say) must not snap a step back mid-drag;
  // the drag's end saves the position, which syncs again.
  const [nodes, setNodes, onNodesChange] = useNodesState(built);
  const dragging = useRef(false);
  useEffect(() => {
    if (!dragging.current) setNodes(built);
  }, [built, setNodes]);

  // Bring everything back into view when steps are added or removed (an example, a new
  // step): a step that lands outside the visible part would look like nothing happened.
  const [flow, setFlow] = useState<ReactFlowInstance | null>(null);
  const stepIds = workflow.nodes.map((node) => node.id).join("|");
  const [box, setBox] = useState<HTMLDivElement | null>(null);
  // Fit the viewport ourselves from the known positions and the measured box. React
  // Flow's own fitView waits for its next internal update, which is one step late after
  // a resize.
  const [size, setSize] = useState<{ width: number; height: number } | null>(null);
  useEffect(() => {
    if (!box) return;
    const observer = new ResizeObserver(([entry]) => {
      const width = Math.round(entry.contentRect.width / 8) * 8;
      const height = Math.round(entry.contentRect.height / 8) * 8;
      setSize((current) => (current?.width === width && current.height === height ? current : { width, height }));
    });
    observer.observe(box);
    return () => observer.disconnect();
  }, [box]);
  // Steps the user placed stay where the user is looking; a workflow that is laid out
  // for them (a view, or steps without a position yet) is brought back into view when
  // the layout changes shape.
  const laidOut = readOnly || workflow.nodes.some((node) => !node.position);
  const extent = laidOut ? `${geometry.left}|${geometry.right}|${geometry.top}|${geometry.bottom}` : "";
  const geometryRef = useRef(geometry);
  geometryRef.current = geometry;
  // The first fit happens before anything is shown, so the graph does not jump into
  // place; later fits (a new step, a resize) glide.
  const [fitted, setFitted] = useState(false);
  const fittedRef = useRef(false);
  // Too big to show whole at a readable size: it pans, and a small map shows where you are.
  const [overflows, setOverflows] = useState(false);
  const [height, setHeight] = useState<number | null>(null);
  const [refit, setRefit] = useState(0);
  useEffect(() => {
    if (!flow || !size || !size.width || !size.height) return;
    const first = !fittedRef.current;
    const timer = window.setTimeout(() => {
      const { left, right, top, bottom } = geometryRef.current;
      if (!Number.isFinite(left + right + top + bottom)) return;
      const wide = Math.max(1, right - left);
      const tall = Math.max(1, bottom - top);
      // A view keeps a strip along the top free for its buttons.
      const strip = readOnly ? CONTROLS_STRIP : 0;
      // An editor shows everything. A view stays readable and pans when it must, except
      // on a phone: there is no room to read and pan, so the picture is an overview and
      // the steps are read as text below it.
      const narrow = size.width < NARROW;
      const floor = showAll || wheelZoom || narrow ? MIN_ZOOM : readOnly ? READ_ZOOM : EDIT_ZOOM;
      let zoom = Math.min(1, size.width / wide);
      let room = size.height;
      // The map of a view that pans sits under the steps, not on top of them.
      let foot = 0;
      if (autoHeight) {
        zoom = Math.max(floor, zoom);
        if (wide * zoom > size.width + 1 && !narrow) foot = MAP_STRIP;
        room = Math.min(MAX_HEIGHT, Math.max(MIN_HEIGHT, Math.round(tall * zoom + strip))) + foot;
        setHeight(room);
      } else {
        zoom = Math.max(floor, Math.min(zoom, (size.height - strip) / tall));
      }
      const tooWide = wide * zoom > size.width + 1;
      const tooTall = tall * zoom > room - strip - foot + 1;
      setOverflows((tooWide || tooTall) && !narrow);
      void flow.setViewport(
        {
          // What does not fit starts at Start, top left; the rest is centred.
          x: tooWide ? -left * zoom : size.width / 2 - ((left + right) / 2) * zoom,
          y: tooTall ? strip - top * zoom : strip + (room - strip - foot) / 2 - ((top + bottom) / 2) * zoom,
          zoom,
        },
        // No animation in a background tab: it would not be drawn and would stall.
        // Nor with reduced motion asked for (#448).
        { duration: first || document.hidden || prefersReducedMotion() ? 0 : 200 },
      );
      fittedRef.current = true;
      setFitted(true);
    }, first ? 0 : 80);
    return () => window.clearTimeout(timer);
  }, [flow, stepIds, fitKey, refit, size, readOnly, autoHeight, showAll, wheelZoom, extent]);
  // Before the width is known: close to the final height, so the page does not jump.
  const estimate = Math.min(MAX_HEIGHT, Math.max(MIN_HEIGHT, Math.round((geometry.bottom - geometry.top) * 0.85 + CONTROLS_STRIP)));

  // The cards reach React Flow one render after the lines; a line waits for its output.
  const outputs = useMemo(
    () =>
      new Set(
        nodes.flatMap((node) => {
          const data = node.data as Partial<StepData>;
          return [...(data.ports ?? []), ...(data.extra ?? [])].map((port) => `${node.id}|${port.id}`);
        }),
      ),
    [nodes],
  );
  // Pointing at a step makes its lines heavier; selecting one also lets the others step back.
  const focus = hovered ?? selectedNode;
  const edges = useMemo<Edge<LineData>[]>(() => {
    return placed.edges
      .filter((edge) => isStartEdge(edge) || outputs.has(`${edge.from}|${portOf(edge)}`))
      .map((edge) => {
      const port = (shown.get(edge.from) ?? []).find((item) => item.id === portOf(edge));
      const color = isStartEdge(edge) || !port ? START_COLOR : portColor(port);
      const count = taken?.get(edge.id) ?? 0;
      const routed = geometry.routes.get(edge.id);
      const back = routed?.route === "under";
      return {
        id: edge.id,
        type: "line",
        source: edge.from,
        target: edge.to,
        sourceHandle: isStartEdge(edge) ? undefined : portOf(edge),
        targetHandle: back ? BACK_IN : undefined,
        selected: selection?.kind === "edge" && selection.id === edge.id,
        deletable: !readOnly,
        selectable: !readOnly,
        zIndex: focus && (edge.from === focus || edge.to === focus) ? 1 : 0,
        markerEnd: { type: MarkerType.ArrowClosed, width: 18, height: 18, color },
        data: {
          color,
          taken: count,
          idle: showsRun && !count,
          live: Boolean(live?.has(edge.id)),
          dim: Boolean(selectedNode) && !hovered && edge.from !== selectedNode && edge.to !== selectedNode,
          strong: Boolean(focus) && (edge.from === focus || edge.to === focus),
          route: routed?.route,
          lane: routed?.lane,
          spread: geometry.spread.get(edge.id) ?? 0,
        },
      };
    });
  }, [placed, shown, outputs, selection, selectedNode, hovered, readOnly, taken, showsRun, live, geometry, focus]);

  return (
    <div
      className={cn(
        "orbit-workflow relative h-[26rem] overflow-hidden rounded-lg border bg-subtle",
        className,
      )}
      style={autoHeight ? { height: height ?? (Number.isFinite(estimate) ? estimate : MIN_HEIGHT) } : undefined}
      role="group"
      aria-label={label}
      ref={setBox}
      // Keyboard on a focused step does what a click does (#448): Enter or Space opens it in
      // the editor, Escape clears the selection, and an arrow-key nudge is saved like a drag.
      onKeyDown={(event) => {
        if (!onSelect) return;
        const target = event.target as HTMLElement;
        const id = target.closest(".react-flow__node")?.getAttribute("data-id");
        if (!id || id === WORKFLOW_START || target.closest("input, textarea, select, button")) return;
        if (event.key === "Enter" || event.key === " ") onSelect({ kind: "node", id });
        else if (event.key === "Escape") onSelect(null);
      }}
      onKeyUp={(event) => {
        if (!onMove || !flow || readOnly || !event.key.startsWith("Arrow")) return;
        if (!(event.target as HTMLElement).closest(".react-flow__node")) return;
        const moved = flow
          .getNodes()
          .filter((node) => node.selected && node.id !== WORKFLOW_START)
          .map((node) => ({ id: node.id, position: node.position }));
        if (moved.length) onMove(moved);
      }}
    >
      <ReactFlow
        className={cn("transition-opacity duration-150", fitted ? "opacity-100" : "opacity-0")}
        nodes={nodes}
        edges={edges}
        nodeTypes={NODE_TYPES}
        edgeTypes={EDGE_TYPES}
        colorMode={dark ? "dark" : "light"}
        onInit={(instance) => setFlow(instance as unknown as ReactFlowInstance)}
        onNodesChange={onNodesChange}
        onNodeDragStart={() => {
          dragging.current = true;
        }}
        onNodeDragStop={(_, node, dragged) => {
          dragging.current = false;
          // Every step that moved, when several were selected.
          onMove?.(
            (dragged.length ? dragged : [node])
              .filter((item) => item.id !== WORKFLOW_START)
              .map((item) => ({ id: item.id, position: item.position })),
          );
        }}
        // Several steps dragged as one selection (Shift and drag draws the selection).
        onSelectionDragStart={() => {
          dragging.current = true;
        }}
        onSelectionDragStop={(_, dragged) => {
          dragging.current = false;
          onMove?.(
            dragged.filter((item) => item.id !== WORKFLOW_START).map((item) => ({ id: item.id, position: item.position })),
          );
        }}
        onNodeClick={(event, node) => {
          // Cmd/Ctrl-click adds a step to the selection, to drag several at once; the
          // panel keeps showing the step that was selected first.
          if (event.metaKey || event.ctrlKey || event.shiftKey) return;
          if (node.id !== WORKFLOW_START) onSelect?.({ kind: "node", id: node.id });
        }}
        onNodeMouseEnter={(_, node) => {
          if (node.id !== WORKFLOW_START && !dragging.current) setHovered(node.id);
        }}
        onNodeMouseLeave={() => setHovered(null)}
        onEdgeClick={(_, edge) => {
          if (!readOnly) onSelect?.({ kind: "edge", id: edge.id });
        }}
        onPaneClick={() => onSelect?.(null)}
        onConnect={(connection: Connection) => {
          if (connection.source && connection.target)
            onConnect?.(connection.source, connection.target, connection.sourceHandle ?? null);
        }}
        onConnectEnd={(event, state) => {
          // Let go over empty canvas: the line gets a new step to lead to.
          if (!onAddFrom || !flow || state.isValid || !state.fromNode || state.fromHandle?.type !== "source") return;
          const point = "changedTouches" in event ? event.changedTouches[0] : event;
          // Where the pointer was let go; a touch reports the element it started on.
          const target = document.elementFromPoint(point.clientX, point.clientY);
          // Empty canvas: inside this canvas and not on a step or a button. A line that
          // happens to run under the pointer does not count as something in the way.
          if (!target || !box?.contains(target)) return;
          if (target.closest(".react-flow__node, .react-flow__controls, .react-flow__minimap, .react-flow__panel")) return;
          const at = flow.screenToFlowPosition({ x: point.clientX, y: point.clientY });
          onAddFrom(state.fromNode.id, state.fromHandle.id ?? null, { x: Math.round(at.x), y: Math.round(at.y - HANDLE_Y) });
        }}
        isValidConnection={(connection) => connection.target !== WORKFLOW_START}
        // One call for everything that was selected, so nothing is lost when several
        // steps or lines are removed at once.
        onDelete={({ nodes: removedNodes, edges: removedEdges }) =>
          onRemove?.({ nodes: removedNodes.map((node) => node.id), edges: removedEdges.map((edge) => edge.id) })
        }
        deleteKeyCode={readOnly ? null : ["Backspace", "Delete"]}
        nodesConnectable={!readOnly}
        nodesDraggable={!readOnly}
        elementsSelectable={Boolean(onSelect)}
        onNodeDoubleClick={(_, node) => {
          if (node.id !== WORKFLOW_START) onOpen?.(node.id);
        }}
        minZoom={0.1}
        maxZoom={1.5}
        // Inside a long page the wheel scrolls the page, not the canvas.
        zoomOnScroll={Boolean(wheelZoom)}
        preventScrolling={Boolean(wheelZoom)}
        connectionRadius={28}
        connectionLineStyle={{ stroke: "hsl(var(--primary))", strokeWidth: 1.75 }}
      >
        <Background variant={BackgroundVariant.Dots} gap={20} size={1} color="hsl(var(--border-strong))" />
        {/* A view keeps its buttons in a strip of their own along the top (see the fit),
            so they never cover a step; an editor has its toolbar there already. */}
        <Controls
          showInteractive={false}
          showFitView={false}
          position={readOnly ? "top-right" : "bottom-left"}
          orientation={readOnly ? "horizontal" : "vertical"}
        >
          <ControlButton onClick={() => setRefit((count) => count + 1)} title="Show everything" aria-label="Show everything">
            <Maximize />
          </ControlButton>
          {onExpand ? (
            <ControlButton onClick={onExpand} title="Open larger" aria-label="Open the workflow larger">
              <Maximize2 />
            </ControlButton>
          ) : null}
        </Controls>
        {overflows ? (
          <MiniMap
            pannable
            position="bottom-right"
            ariaLabel="Map of the workflow"
            style={{ width: 132, height: 84, marginBottom: 22 }}
            nodeBorderRadius={12}
            nodeColor="hsl(var(--muted-foreground) / 0.45)"
            maskColor="hsl(var(--muted) / 0.6)"
            bgColor="hsl(var(--card))"
          />
        ) : null}
      </ReactFlow>

      {empty && !workflow.nodes.length ? (
        <div className="pointer-events-none absolute inset-0 flex items-center justify-center p-6">
          <div className="pointer-events-auto max-w-sm rounded-lg border bg-card p-5 text-center shadow-sm">{empty}</div>
        </div>
      ) : null}
    </div>
  );
}
