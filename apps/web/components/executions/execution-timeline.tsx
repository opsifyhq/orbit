"use client";

import { useEffect, useMemo, useRef, useState } from "react";
import { usePathname, useRouter, useSearchParams } from "next/navigation";
import { ChevronRight } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Card, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Markdown } from "@/components/ui/markdown";
import { Segmented } from "@/components/ui/segmented";
import { Select } from "@/components/ui/select";
import { StatusGlyph } from "@/components/ui/status";
import { Switch } from "@/components/ui/switch";
import {
  DEFAULT_EVENT_FILTERS,
  EFFECT_FILTERS,
  EVENT_TYPE_FILTERS,
  eventEffect,
  eventFilterParams,
  filterEvents,
  groupTimelineEvents,
  parseEventFilters,
  toolOptions,
  type EventFilters,
  type TimelineEventGroup,
} from "@/lib/execution-event-filters";
import { describeEvent, isKeyEvent } from "@/lib/execution-events";
import type { ExecutionEventRecord } from "@/lib/types";
import { cn, formatDateTime, relativeTime } from "@/lib/utils";

/**
 * The execution timeline, shared by every view that shows a run's events.
 * Filters are presentation only: the event list and its sequence are untouched,
 * and the chosen filters live in the URL so they survive reloads and live tailing.
 */
export function ExecutionTimeline({
  events,
  active,
  connected,
  status,
  pendingApproval,
}: {
  events: ExecutionEventRecord[];
  active: boolean;
  connected: boolean;
  status: string;
  pendingApproval: boolean;
}) {
  const router = useRouter();
  const pathname = usePathname();
  const searchParams = useSearchParams();
  const filters = useMemo(
    () => parseEventFilters(new URLSearchParams(searchParams.toString())),
    [searchParams],
  );
  const [showAll, setShowAll] = useState(false);
  const [groupRepeats, setGroupRepeats] = useState(true);
  const [expandedGroups, setExpandedGroups] = useState<Set<string>>(() => new Set());
  const [follow, setFollow] = useState(true);
  const logRef = useRef<HTMLDivElement>(null);
  // Follow keeps a live run pinned to its newest line; a finished run opens at the start.
  const liveNow = useRef(false);
  liveNow.current = active;

  const { visible, hiddenCount } = useMemo(
    () => filterEvents(events, filters, isKeyEvent, showAll),
    [events, filters, showAll],
  );
  const entries = useMemo(() => groupTimelineEvents(visible, groupRepeats), [visible, groupRepeats]);
  const choices = useMemo(() => toolOptions(events), [events]);
  const hasEffects = useMemo(
    () => events.some((event) => eventEffect(event) !== "none"),
    [events],
  );

  function updateFilters(next: EventFilters) {
    const query = new URLSearchParams(searchParams.toString());
    for (const key of ["events", "tool", "effect"]) query.delete(key);
    for (const [key, value] of Object.entries(eventFilterParams(next))) query.set(key, value);
    const search = query.toString();
    router.replace(search ? `${pathname}?${search}` : pathname, { scroll: false });
  }

  function clearFilters() {
    updateFilters(DEFAULT_EVENT_FILTERS);
    setShowAll(true);
  }

  // The text field keeps its own value and writes to the URL after a pause. Bound
  // straight to the URL it lost every trailing space (the URL value is trimmed) and
  // navigated on each keystroke.
  const [toolText, setToolText] = useState(filters.tool);
  const latestFilters = useRef(filters);
  latestFilters.current = filters;
  const lastWrittenTool = useRef(filters.tool);
  useEffect(() => {
    // Our own write arriving back (after more was typed) must not undo the typing; only
    // a change from outside the field (Clear filters, back button) replaces the text.
    if (filters.tool === lastWrittenTool.current) return;
    lastWrittenTool.current = filters.tool;
    setToolText(filters.tool);
  }, [filters.tool]);
  useEffect(() => {
    const next = toolText.trim();
    if (next === latestFilters.current.tool) return;
    const timer = window.setTimeout(() => {
      lastWrittenTool.current = next;
      updateFilters({ ...latestFilters.current, tool: next });
    }, 250);
    return () => window.clearTimeout(timer);
    // `updateFilters` reads the current URL when it runs; only typing should schedule it.
  }, [toolText]);

  function toggleGroup(key: string) {
    setExpandedGroups((current) => {
      const next = new Set(current);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  }

  useEffect(() => {
    if (follow && liveNow.current && logRef.current) {
      logRef.current.scrollTop = logRef.current.scrollHeight;
    }
  }, [visible.length, follow, groupRepeats]);

  const filtered = filters.type !== "all" || Boolean(filters.tool) || filters.effect !== "all" || !showAll;

  // What a screen reader hears while the run is live: a count, at most every few seconds.
  const [announcedCount, setAnnouncedCount] = useState(events.length);
  const pendingCount = useRef(events.length);
  pendingCount.current = events.length;
  useEffect(() => {
    if (!active) {
      setAnnouncedCount(pendingCount.current);
      return;
    }
    const timer = window.setInterval(() => setAnnouncedCount(pendingCount.current), 5000);
    return () => window.clearInterval(timer);
  }, [active]);

  return (
    <Card className="min-w-0 overflow-hidden">
      <CardHeader className="flex-col gap-3 border-b sm:flex-row sm:items-center sm:justify-between">
        <div>
          <CardTitle as="h2" className="flex items-center gap-2">
            Timeline
            {connected && active ? (
              <span className="inline-flex items-center gap-1.5 text-xs font-medium text-info">
                <StatusGlyph shape="orbiting" tone="info" /> Live
              </span>
            ) : null}
          </CardTitle>
          <p className="mt-0.5 text-[13px] text-muted-foreground">
            {events.length === 0
              ? "No events yet"
              : `${visible.length} of ${events.length} events, last ${relativeTime(events.at(-1)!.createdAt)}`}
          </p>
          <p className="sr-only" role="status">
            {announcedCount ? `${announcedCount} timeline events${active ? " so far" : ""}` : ""}
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
          <Segmented
            label="Event detail"
            size="sm"
            value={showAll ? "all" : "key"}
            onChange={(value) => setShowAll(value === "all")}
            options={[
              { value: "key", label: "Story" },
              { value: "all", label: "Everything" },
            ]}
          />
          {active ? (
            <label className="flex cursor-pointer items-center gap-2 text-[13px] text-muted-foreground">
              <Switch checked={follow} onCheckedChange={setFollow} aria-label="Follow new events" />
              Follow
            </label>
          ) : null}
          <label className="flex cursor-pointer items-center gap-2 text-[13px] text-muted-foreground">
            <Switch
              checked={groupRepeats}
              onCheckedChange={setGroupRepeats}
              aria-label="Group repeated tool calls"
            />
            Group repeats
          </label>
        </div>
      </CardHeader>

      <div className="space-y-3 border-b px-4 py-3 sm:px-5">
        <div className="min-w-0">
          <p className="mb-1.5 text-[13px] font-medium">Event type</p>
          <Segmented
            label="Event type"
            size="sm"
            className="w-full sm:w-auto"
            value={filters.type}
            onChange={(type) => updateFilters({ ...filters, type })}
            options={EVENT_TYPE_FILTERS.map((option) => ({ value: option.value, label: option.label }))}
          />
        </div>
        <div className="grid min-w-0 grid-cols-1 gap-2 sm:grid-cols-2">
          <div className="min-w-0">
            <label htmlFor="event-tool-filter" className="mb-1.5 block text-[13px] font-medium">
              Tool or action
            </label>
            <Input
              id="event-tool-filter"
              list="event-tool-choices"
              value={toolText}
              onChange={(event) => setToolText(event.target.value)}
              placeholder="Search name, capability, connector"
              autoComplete="off"
            />
            <datalist id="event-tool-choices">
              {choices.map((choice) => (
                <option key={choice.value} value={choice.value} label={choice.label} />
              ))}
            </datalist>
          </div>
          {hasEffects ? (
            <div className="min-w-0">
              <label htmlFor="event-effect-filter" className="mb-1.5 block text-[13px] font-medium">
                Effect
              </label>
              <Select
                id="event-effect-filter"
                value={filters.effect}
                onChange={(event) => updateFilters({ ...filters, effect: event.target.value as EventFilters["effect"] })}
              >
                {EFFECT_FILTERS.map((option) => (
                  <option key={option.value} value={option.value}>
                    {option.label}
                  </option>
                ))}
              </Select>
            </div>
          ) : null}
        </div>
        <details className="text-[13px] text-muted-foreground">
          <summary className="w-fit cursor-pointer rounded font-medium text-primary hover:underline">
            How filters work
          </summary>
          <div className="mt-2 space-y-1 rounded-lg border bg-subtle p-3">
            <p>
              <span className="font-medium text-foreground">Story</span> shows the run&apos;s key
              events; <span className="font-medium text-foreground">Everything</span> adds debug logs
              and bookkeeping. Chips, tool/action text and effect narrow either view further.
            </p>
            <p>
              Tool text matches tool and capability names, connectors, connections and event
              messages. Repeated calls fold into a row you can expand; errors and approvals never do.
              Filters stay in the URL and keep applying as new events arrive.
            </p>
          </div>
        </details>
      </div>

      <div
        ref={logRef}
        className="max-h-[620px] min-h-[180px] overflow-y-auto px-4 py-1 sm:px-5"
        tabIndex={0}
        role="log"
        // A live run can emit dozens of events a second and a filter change swaps the
        // whole list: the summary line above announces progress instead.
        aria-live="off"
        aria-label="Execution timeline"
      >
        {entries.length === 0 ? (
          <div className="py-12 text-center text-sm text-muted-foreground">
            <p>
              {events.length > 0
                ? "Nothing matches these filters."
                : active
                  ? "Waiting for the harness to report."
                  : "No events were recorded."}
            </p>
            {events.length > 0 ? (
              <button
                type="button"
                className="mt-2 rounded font-medium text-primary underline-offset-4 hover:underline"
                onClick={clearFilters}
              >
                Clear filters
              </button>
            ) : null}
          </div>
        ) : (
          entries.map((entry) =>
            entry.kind === "group" ? (
              <EventGroup
                key={`group-${entry.events[0].seq}`}
                group={entry}
                expanded={expandedGroups.has(String(entry.events[0].seq))}
                onToggle={() => toggleGroup(String(entry.events[0].seq))}
              />
            ) : (
              <EventLine key={entry.event.seq} event={entry.event} />
            ),
          )
        )}
      </div>

      {active && events.length > 0 ? (
        <div className="flex items-center gap-2 border-t bg-subtle px-4 py-2.5 text-[13px] text-muted-foreground sm:px-5">
          <StatusGlyph shape={pendingApproval ? "paused" : "orbiting"} tone={pendingApproval ? "warning" : "info"} />
          {pendingApproval
            ? "Paused until someone approves or rejects the action above."
            : status === "queued"
              ? "Waiting for a worker to pick it up."
              : status === "dispatched"
                ? `A worker picked it up and is starting the harness. Last activity ${relativeTime(events.at(-1)!.createdAt)}.`
                : `The agent is working. Last activity ${relativeTime(events.at(-1)!.createdAt)}.`}
        </div>
      ) : null}
      {hiddenCount > 0 ? (
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1 border-t bg-subtle px-4 py-2.5 text-[13px] text-muted-foreground sm:px-5">
          <span className="tabular">
            {hiddenCount} event{hiddenCount === 1 ? "" : "s"} hidden by filters
            {!showAll ? " and the Story view" : ""}.
          </span>
          {filtered ? (
            <button
              type="button"
              className="ml-auto font-medium text-primary underline-offset-4 hover:underline"
              onClick={clearFilters}
            >
              Clear filters
            </button>
          ) : null}
        </div>
      ) : null}
    </Card>
  );
}

function EventGroup({
  group,
  expanded,
  onToggle,
}: {
  group: TimelineEventGroup;
  expanded: boolean;
  onToggle: () => void;
}) {
  return (
    <div className="border-b border-border/70 last:border-b-0">
      <button
        type="button"
        onClick={onToggle}
        aria-expanded={expanded}
        className="flex w-full min-h-9 items-center justify-between gap-2 py-2 text-left text-[13px] font-medium text-muted-foreground hover:text-foreground"
      >
        <span className="inline-flex min-w-0 items-center gap-1.5">
          <ChevronRight
            aria-hidden="true"
            className={cn("h-3.5 w-3.5 shrink-0 transition-transform", expanded && "rotate-90")}
          />
          <span className="truncate">{group.label}</span>
          <span className="tabular shrink-0">× {group.events.length}</span>
        </span>
        <span className="shrink-0 text-xs font-normal">repeated calls</span>
      </button>
      {expanded ? (
        <div className="pl-2 sm:pl-6">
          {group.events.map((event) => (
            <EventLine key={event.seq} event={event} />
          ))}
        </div>
      ) : null}
    </div>
  );
}

const TONE_TEXT: Record<string, string> = {
  plain: "text-foreground",
  muted: "text-muted-foreground",
  agent: "text-foreground",
  action: "text-foreground",
  warning: "text-warning",
  danger: "text-destructive",
  success: "text-success",
};

const LABEL_TEXT: Record<string, string> = {
  plain: "text-muted-foreground",
  muted: "text-muted-foreground",
  agent: "text-primary",
  action: "text-info",
  warning: "text-warning",
  danger: "text-destructive",
  success: "text-success",
};

/**
 * What the agent said along the way, rendered as Markdown like its final answer.
 * Long messages fold to a few lines so one report cannot push the rest of the timeline
 * out of view; the full text is one click away.
 */
function AgentProse({ text }: { text: string }) {
  const [expanded, setExpanded] = useState(false);
  const [overflows, setOverflows] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const node = ref.current;
    if (!node) return;
    const measure = () => setOverflows(node.scrollHeight > node.clientHeight + 4);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(node);
    return () => observer.disconnect();
  }, [text]);

  return (
    <div>
      <div
        ref={ref}
        className={cn(
          "relative",
          !expanded && "max-h-56 overflow-hidden",
          !expanded &&
            overflows &&
            "after:pointer-events-none after:absolute after:inset-x-0 after:bottom-0 after:h-12 after:bg-gradient-to-t after:from-card after:to-transparent",
        )}
      >
        <Markdown className="text-[13px]">{text}</Markdown>
      </div>
      {overflows || expanded ? (
        <button
          type="button"
          onClick={() => setExpanded((value) => !value)}
          aria-expanded={expanded}
          className="mt-1 rounded text-xs font-medium text-primary hover:underline"
        >
          {expanded ? "Show less" : "Show all"}
        </button>
      ) : null}
    </div>
  );
}

function EventLine({ event }: { event: ExecutionEventRecord }) {
  const view = describeEvent(event);
  const hasData = Boolean(event.data && typeof event.data === "object" && Object.keys(event.data).length);

  return (
    <div className="grid grid-cols-[4.25rem_minmax(0,1fr)] gap-x-3 border-b border-border/70 py-2.5 text-[13px] last:border-b-0 sm:grid-cols-[4.25rem_4.75rem_minmax(0,1fr)]">
      <time
        className="tabular pt-px font-mono text-xs text-muted-foreground"
        dateTime={event.createdAt}
        title={formatDateTime(event.createdAt)}
      >
        {formatClock(event.createdAt)}
      </time>
      <span className={cn("hidden pt-px text-xs font-medium sm:block", LABEL_TEXT[view.tone])}>{view.label}</span>
      <div className="min-w-0">
        <div className="flex flex-wrap items-start gap-x-2 gap-y-1">
          {view.prose ? (
            <div className="min-w-0 flex-1">
              <span className={cn("text-xs font-medium sm:hidden", LABEL_TEXT[view.tone])}>{view.label}</span>
              <AgentProse text={view.title} />
            </div>
          ) : (
            <p className={cn("min-w-0 flex-1 break-words font-medium", TONE_TEXT[view.tone])}>
              <span className={cn("font-medium sm:hidden", LABEL_TEXT[view.tone])}>{view.label}: </span>
              {view.title}
            </p>
          )}
          {view.badge ? (
            <Badge
              variant={
                view.badge.tone === "success"
                  ? "success"
                  : view.badge.tone === "warning"
                    ? "warning"
                    : view.badge.tone === "danger"
                      ? "destructive"
                      : "secondary"
              }
            >
              {view.badge.text}
            </Badge>
          ) : null}
        </div>
        {view.detail ? <p className="mt-0.5 break-words text-muted-foreground">{view.detail}</p> : null}
        {hasData ? (
          <details className="mt-1">
            <summary className="w-fit cursor-pointer select-none rounded text-xs text-muted-foreground hover:text-foreground">
              Details
            </summary>
            <pre className="code-block mt-1.5 max-h-64 whitespace-pre-wrap">{JSON.stringify(event.data, null, 2)}</pre>
          </details>
        ) : null}
      </div>
    </div>
  );
}

function formatClock(value: string): string {
  return new Date(value).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    // `hour12: false` renders midnight as 24:xx in some locales.
    hourCycle: "h23",
  });
}
