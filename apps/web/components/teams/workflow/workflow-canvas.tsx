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
  NODE_HEIGHT,
  NODE_WIDTH,
  WORKFLOW_START,
  conditionLabel,
  isStartEdge,
  loopSteps,
  withPositions,
  type NodeRunState,
  type Workflow,
  type WorkflowEdge,
} from "@/lib/workflow";
import { cn } from "@/lib/utils";

export type WorkflowSelection = { kind: "node" | "edge"; id: string } | null;

type Tone = "start" | "success" | "failure" | "outcome" | "always";

const TONE_COLOR: Record<Tone, string> = {
  start: "hsl(var(--muted-foreground))",
  always: "hsl(var(--muted-foreground))",
  success: "hsl(var(--success))",
  failure: "hsl(var(--destructive))",
  outcome: "hsl(var(--primary))",
};

const TONE_TEXT: Record<Tone, string> = {
  start: "text-muted-foreground",
  always: "text-muted-foreground",
  success: "text-success",
  failure: "text-destructive",
  outcome: "text-accent-foreground",
};

function toneOf(edge: WorkflowEdge): Tone {
  if (isStartEdge(edge)) return "start";
  return edge.when === "always" || edge.when === "failure" || edge.when === "outcome" ? edge.when : "success";
}

interface StepData extends Record<string, unknown> {
  name: string;
  agent: string | null;
  outcomes: string[];
  /** Run limit, when the step sits on a loop. */
  loopLimit: number | null;
  problem: boolean;
  readOnly: boolean;
  run?: NodeRunState;
  /** A run is shown and this step did not run in it. */
  idle: boolean;
}

interface LineData extends Record<string, unknown> {
  label: string;
  tone: Tone;
  /** Lines between the same two steps are spread out so their labels do not overlap. */
  shift: number;
  taken: number;
  /** A run is shown and it did not follow this line. */
  idle: boolean;
  live: boolean;
  /**
   * How a line that points back is drawn: `elbow` from the left of a step that hangs
   * below straight up into the step it returns to, `under` beneath the steps in between.
   */
  route?: "elbow" | "under";
  /** Where an `under` line runs. */
  lane?: number;
}

const handleClass =
  "!h-3 !w-3 !rounded-full !border-2 !border-border-strong !bg-card transition-colors hover:!border-primary";

const hiddenHandle = "!pointer-events-none !h-px !w-px !min-w-0 !min-h-0 !border-0 !opacity-0";
const BACK_LEFT = "back-left";
const BACK_BOTTOM = "back-bottom";
const BACK_IN = "back-in";

const StepNode = memo(function StepNode({ data, selected }: NodeProps<Node<StepData>>) {
  const run = data.run;
  return (
    <div
      style={{ width: NODE_WIDTH, minHeight: NODE_HEIGHT }}
      className={cn(
        "relative flex flex-col justify-center gap-1 rounded-xl border bg-card px-3 py-2.5 text-left shadow-sm transition-shadow",
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
        isConnectable={!data.readOnly}
        className={cn(handleClass, data.readOnly && "!opacity-0")}
      />
      <div className="flex items-start gap-2">
        <p className="min-w-0 flex-1 truncate text-sm font-medium leading-5">{data.name || "Unnamed step"}</p>
        {run ? (
          <span
            className={cn(
              "mt-0.5 flex h-4 shrink-0 items-center gap-1 text-[11px] font-medium",
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
          <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0 text-warning" aria-label="Needs attention" />
        ) : null}
      </div>
      <p className={cn("flex items-center gap-1.5 text-xs", data.agent ? "text-muted-foreground" : "text-warning")}>
        <Bot className="h-3.5 w-3.5 shrink-0" aria-hidden />
        <span className="truncate">{data.agent ?? "Choose an agent"}</span>
      </p>
      {run?.limitReached || run?.outcome || data.outcomes.length || data.loopLimit ? (
        <div className="flex flex-wrap items-center gap-1 pt-0.5">
          {run?.limitReached ? (
            <span className="rounded-full bg-destructive/10 px-1.5 py-px text-[11px] font-medium text-destructive">
              limit reached
            </span>
          ) : run?.outcome ? (
            <span className="rounded-full bg-primary/10 px-1.5 py-px text-[11px] font-medium text-accent-foreground">
              {run.outcome}
            </span>
          ) : (
            data.outcomes.map((outcome) => (
              <span key={outcome} className="rounded-full border px-1.5 py-px text-[11px] text-muted-foreground">
                {outcome}
              </span>
            ))
          )}
          {data.loopLimit ? (
            <span
              className="flex items-center gap-0.5 rounded-full border px-1.5 py-px text-[11px] text-muted-foreground"
              title={`Runs at most ${data.loopLimit} times in one run`}
            >
              <Repeat className="h-3 w-3" aria-hidden /> max {data.loopLimit}
            </span>
          ) : null}
        </div>
      ) : null}
      <Handle
        type="source"
        position={Position.Right}
        isConnectable={!data.readOnly}
        className={cn(handleClass, data.readOnly && "!opacity-0")}
      />
      {/* Where lines that point back leave and arrive; never shown, drawn by LineEdge. */}
      <Handle id={BACK_LEFT} type="source" position={Position.Left} isConnectable={false} className={hiddenHandle} />
      <Handle id={BACK_BOTTOM} type="source" position={Position.Bottom} isConnectable={false} className={hiddenHandle} />
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
        className={cn(handleClass, data.readOnly && "!opacity-0")}
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
  const shift = data?.shift ?? 0;
  const route = data?.route;
  // A line to a step in an earlier column that has no route yet (a step being dragged).
  const back = !route && targetX + NODE_WIDTH < sourceX - NODE_WIDTH / 2;
  // Forward, level, and far enough apart that another step can sit in between.
  const skips = !route && Math.abs(targetY - sourceY) < NODE_HEIGHT / 2 && targetX - sourceX > NODE_WIDTH;
  const [path, labelX, labelY] =
    route === "elbow" && sourceX - targetX > 2 * CORNER && sourceY - targetY > 2 * CORNER
      ? elbowPath(sourceX, sourceY, targetX, targetY)
      : route
        ? underPath(sourceX, sourceY, targetX, targetY, data?.lane ?? 0)
        : back || skips
          ? aroundPath(sourceX, sourceY, targetX, targetY, shift)
          : curvePath(
              getBezierPath({
                sourceX,
                sourceY: sourceY + shift * 0.4,
                sourcePosition,
                targetX,
                targetY: targetY + shift * 0.4,
                targetPosition,
              }),
              shift,
            );
  const tone = data?.tone ?? "success";
  return (
    <>
      <BaseEdge
        id={id}
        path={path}
        markerEnd={markerEnd}
        interactionWidth={24}
        style={{
          stroke: TONE_COLOR[tone],
          strokeWidth: selected ? 2.5 : 1.5,
          strokeDasharray: data?.idle ? "4 4" : data?.live ? "6 4" : undefined,
          opacity: data?.idle ? 0.35 : 1,
          animation: data?.live ? "orbit-line-flow 0.6s linear infinite" : undefined,
        }}
      />
      {data?.label ? (
        <EdgeLabelRenderer>
          <span
            style={{ transform: `translate(-50%, -50%) translate(${labelX}px, ${labelY}px)` }}
            className={cn(
              "nodrag nopan pointer-events-none absolute rounded-full border bg-card px-2 py-px text-[11px] font-medium leading-4 shadow-sm",
              TONE_TEXT[tone],
              selected && "border-primary",
              data.idle && "opacity-50",
            )}
          >
            {data.label}
            {data.taken > 1 ? <span className="tabular ml-1 text-muted-foreground">{data.taken}×</span> : null}
          </span>
        </EdgeLabelRenderer>
      ) : null}
    </>
  );
});

/**
 * A line that cannot go straight: back to an earlier step, or forward past a step that
 * sits in between on the same row. It leaves to the right, passes over the top (or
 * underneath, when the source is the lower of the two) and enters from the left, so it
 * stays clear of the steps and of the straight lines between them.
 */
function aroundPath(sx: number, sy: number, tx: number, ty: number, shift: number): [string, number, number] {
  const clear = NODE_HEIGHT / 2 + 34 + shift;
  const over = sy <= ty;
  const y = over ? Math.min(sy, ty) - clear : Math.max(sy, ty) + clear;
  const out = 26;
  const r = 14;
  const up = over ? -1 : 1;
  const dir = tx - out >= sx + out ? 1 : -1;
  const path = [
    `M ${sx} ${sy}`,
    `L ${sx + out - r} ${sy}`,
    `Q ${sx + out} ${sy} ${sx + out} ${sy + up * r}`,
    `L ${sx + out} ${y - up * r}`,
    `Q ${sx + out} ${y} ${sx + out + dir * r} ${y}`,
    `L ${tx - out - dir * r} ${y}`,
    `Q ${tx - out} ${y} ${tx - out} ${y - up * r}`,
    `L ${tx - out} ${ty + up * r}`,
    `Q ${tx - out} ${ty} ${tx - out + r} ${ty}`,
    `L ${tx} ${ty}`,
  ].join(" ");
  return [path, (sx + tx) / 2, y + shift];
}

const CORNER = 12;

const curvePath = (curve: [string, number, number, ...unknown[]], shift: number): [string, number, number] => [
  curve[0],
  curve[1],
  curve[2] + shift,
];

/**
 * Back from a step that hangs below the main line: out of its left side, then straight
 * up into the step it returns to. The shortest way a loop can be drawn.
 */
function elbowPath(sx: number, sy: number, tx: number, ty: number): [string, number, number] {
  const path = [`M ${sx} ${sy}`, `L ${tx + CORNER} ${sy}`, `Q ${tx} ${sy} ${tx} ${sy - CORNER}`, `L ${tx} ${ty}`].join(" ");
  return [path, (sx + tx) / 2, sy];
}

/** Back underneath: down out of the step, along a lane below the steps in between, up. */
function underPath(sx: number, sy: number, tx: number, ty: number, lane: number): [string, number, number] {
  const y = Math.max(lane, sy + 2 * CORNER, ty + 2 * CORNER);
  const dir = tx < sx ? -1 : 1;
  const r = Math.min(CORNER, Math.abs(tx - sx) / 2);
  const path = [
    `M ${sx} ${sy}`,
    `L ${sx} ${y - r}`,
    `Q ${sx} ${y} ${sx + dir * r} ${y}`,
    `L ${tx - dir * r} ${y}`,
    `Q ${tx} ${y} ${tx} ${y - r}`,
    `L ${tx} ${ty}`,
  ].join(" ");
  return [path, (sx + tx) / 2, y];
}

const MIN_ZOOM = 0.3;
/** A workflow that is only looked at is never drawn smaller than this; it pans instead. */
const READ_ZOOM = 0.7;
/** A step card with its row of chips. */
const CARD_HEIGHT = NODE_HEIGHT + 12;
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

/**
 * The workflow as a picture: steps as cards, lines with their condition. With
 * `onChange…` callbacks it is an editor (drag to move, drag from a dot to connect,
 * Backspace to remove); without them it is a read-only view, optionally of one run.
 */
export function WorkflowCanvas({
  workflow,
  agentName,
  selection,
  onSelect,
  onOpen,
  onMove,
  onConnect,
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
  onExpand,
}: {
  workflow: Workflow;
  agentName: (id: string) => string | null;
  selection?: WorkflowSelection;
  onSelect?: (selection: WorkflowSelection) => void;
  /** A step was double-clicked: the user wants to edit it. */
  onOpen?: (id: string) => void;
  onMove?: (id: string, position: { x: number; y: number }) => void;
  onConnect?: (from: string, to: string) => void;
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
  /** Adds a button that asks for a larger view of the workflow. */
  onExpand?: () => void;
}) {
  const dark = useIsDark();
  const readOnly = !onMove;
  const placed = useMemo(() => withPositions(workflow), [workflow]);
  const loops = useMemo(() => loopSteps(workflow), [workflow]);
  const showsRun = Boolean(runStates);

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
      position: { x: left - 150, y: middle + NODE_HEIGHT / 2 - 18 },
      data: { readOnly },
      draggable: false,
      deletable: false,
      selectable: false,
    };
    const steps = placed.nodes.map<Node<StepData>>((node) => ({
      id: node.id,
      type: "step",
      position: node.position ?? { x: 0, y: 0 },
      selected: selection?.kind === "node" && selection.id === node.id,
      draggable: !readOnly,
      deletable: !readOnly,
      data: {
        name: node.name,
        agent: node.agentId ? agentName(node.agentId) : null,
        outcomes: node.outcomes ?? [],
        loopLimit: loops.has(node.id) ? (node.maxRuns ?? DEFAULT_MAX_RUNS) : null,
        problem: Boolean(problems?.has(node.id)),
        readOnly,
        run: runStates?.get(node.id),
        idle: showsRun && !runStates?.has(node.id),
      },
    }));
    return [start, ...steps];
  }, [placed, selection, readOnly, agentName, loops, problems, runStates, showsRun]);

  // How lines that point back are drawn, and the room the whole picture takes.
  const geometry = useMemo(() => {
    const rects = new Map(
      placed.nodes.map((node) => {
        const at = node.position ?? { x: 0, y: 0 };
        return [node.id, { l: at.x, r: at.x + NODE_WIDTH, t: at.y, b: at.y + CARD_HEIGHT }] as const;
      }),
    );
    const routes = new Map<string, { route: "elbow" | "under"; lane?: number }>();
    let top = Math.min(...built.map((node) => node.position.y)) - 24;
    let bottom = Math.max(...built.map((node) => node.position.y + CARD_HEIGHT)) + 24;
    let lanes = 0;
    for (const edge of placed.edges) {
      const s = rects.get(edge.from);
      const t = rects.get(edge.to);
      if (!s || !t) continue;
      if (t.l >= s.l - NODE_WIDTH / 2) {
        // A level line past a step in between goes over the top (see aroundPath).
        if (Math.abs(t.t - s.t) < NODE_HEIGHT / 2 && t.l - s.r > NODE_WIDTH) top = Math.min(top, Math.min(s.t, t.t) - 56);
        continue;
      }
      const x = (t.l + t.r) / 2;
      const y = (s.t + s.b) / 2;
      const blocked = (x1: number, y1: number, x2: number, y2: number) =>
        [...rects].some(
          ([id, o]) => id !== edge.from && id !== edge.to && x2 > o.l - 8 && x1 < o.r + 8 && y2 > o.t - 8 && y1 < o.b + 8,
        );
      if (y > t.b + 2 * CORNER && !blocked(x, y, s.l, y) && !blocked(x, t.b, x, y)) {
        routes.set(edge.id, { route: "elbow" });
        continue;
      }
      const between = [...rects.values()].filter((o) => o.r > x - 8 && o.l < (s.l + s.r) / 2 + 8);
      const lane = Math.max(...between.map((o) => o.b)) + 30 + lanes * 14;
      lanes += 1;
      routes.set(edge.id, { route: "under", lane });
      bottom = Math.max(bottom, lane + 24);
    }
    const left = Math.min(...built.map((node) => node.position.x)) - 16;
    const right = Math.max(...built.map((node) => node.position.x + (node.type === "start" ? 80 : NODE_WIDTH))) + 16;
    return { routes, left, right, top, bottom };
  }, [placed, built]);

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
      const floor = readOnly && !showAll && !narrow ? READ_ZOOM : MIN_ZOOM;
      let zoom = Math.min(1, size.width / wide);
      let room = size.height;
      if (autoHeight) {
        zoom = Math.max(floor, zoom);
        room = Math.min(MAX_HEIGHT, Math.max(MIN_HEIGHT, Math.round(tall * zoom + strip)));
        setHeight(room);
      } else {
        zoom = Math.max(floor, Math.min(zoom, (size.height - strip) / tall));
      }
      const tooWide = wide * zoom > size.width + 1;
      const tooTall = tall * zoom > room - strip + 1;
      setOverflows((tooWide || tooTall) && !narrow);
      void flow.setViewport(
        {
          // What does not fit starts at Start, top left; the rest is centred.
          x: tooWide ? -left * zoom : size.width / 2 - ((left + right) / 2) * zoom,
          y: tooTall ? strip - top * zoom : strip + (room - strip) / 2 - ((top + bottom) / 2) * zoom,
          zoom,
        },
        // No animation in a background tab: it would not be drawn and would stall.
        { duration: first || document.hidden ? 0 : 200 },
      );
      fittedRef.current = true;
      setFitted(true);
    }, first ? 0 : 80);
    return () => window.clearTimeout(timer);
  }, [flow, stepIds, fitKey, refit, size, readOnly, autoHeight, showAll, extent]);
  // Before the width is known: close to the final height, so the page does not jump.
  const estimate = Math.min(MAX_HEIGHT, Math.max(MIN_HEIGHT, Math.round((geometry.bottom - geometry.top) * 0.85 + CONTROLS_STRIP)));

  const edges = useMemo<Edge<LineData>[]>(() => {
    const pairs = new Map<string, number>();
    return placed.edges.map((edge) => {
      const pair = [edge.from, edge.to].sort().join("|");
      const index = pairs.get(pair) ?? 0;
      pairs.set(pair, index + 1);
      const tone = toneOf(edge);
      const count = taken?.get(edge.id) ?? 0;
      const routed = geometry.routes.get(edge.id);
      return {
        id: edge.id,
        type: "line",
        source: edge.from,
        target: edge.to,
        sourceHandle: routed ? (routed.route === "elbow" ? BACK_LEFT : BACK_BOTTOM) : undefined,
        targetHandle: routed ? BACK_IN : undefined,
        selected: selection?.kind === "edge" && selection.id === edge.id,
        deletable: !readOnly,
        selectable: !readOnly,
        markerEnd: { type: MarkerType.ArrowClosed, width: 16, height: 16, color: TONE_COLOR[tone] },
        data: {
          label: conditionLabel(edge),
          tone,
          shift: index * 22,
          taken: count,
          idle: showsRun && !count,
          live: Boolean(live?.has(edge.id)),
          route: routed?.route,
          lane: routed?.lane,
        },
      };
    });
  }, [placed, selection, readOnly, taken, showsRun, live, geometry]);

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
        onNodeDragStop={(_, node) => {
          dragging.current = false;
          onMove?.(node.id, node.position);
        }}
        onNodeClick={(_, node) => {
          if (node.id !== WORKFLOW_START) onSelect?.({ kind: "node", id: node.id });
        }}
        onEdgeClick={(_, edge) => {
          if (!readOnly) onSelect?.({ kind: "edge", id: edge.id });
        }}
        onPaneClick={() => onSelect?.(null)}
        onConnect={(connection: Connection) => {
          if (connection.source && connection.target) onConnect?.(connection.source, connection.target);
        }}
        isValidConnection={(connection) =>
          connection.source !== connection.target && connection.target !== WORKFLOW_START
        }
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
        minZoom={MIN_ZOOM}
        maxZoom={1.5}
        // The canvas sits inside a long page: the wheel scrolls the page, not the canvas.
        zoomOnScroll={false}
        preventScrolling={false}
        connectionRadius={28}
        connectionLineStyle={{ stroke: "hsl(var(--primary))", strokeWidth: 1.5 }}
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
        {overflows && readOnly ? (
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
