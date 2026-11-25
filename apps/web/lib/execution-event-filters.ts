import type { ExecutionEventRecord } from "@/lib/types";

/**
 * Presentation-only filters for execution event timelines.
 *
 * Nothing here changes an event or its order: it only decides which lines are
 * drawn and which consecutive tool calls fold into one expandable group.
 * Labels are derived from tool/capability/connector identifiers, never from
 * arguments or other event payload values that could contain credentials.
 */

export const EVENT_TYPE_FILTERS = [
  { value: "all", label: "All" },
  { value: "messages", label: "Messages/output" },
  { value: "tools", label: "Tool calls" },
  { value: "approvals", label: "Approvals" },
  { value: "errors", label: "Errors" },
  { value: "system", label: "System/lifecycle" },
] as const;

export type EventTypeFilter = (typeof EVENT_TYPE_FILTERS)[number]["value"];

export const EFFECT_FILTERS = [
  { value: "all", label: "All effects" },
  { value: "read", label: "Read" },
  { value: "write", label: "Write" },
  { value: "external_side_effect", label: "External side effect" },
] as const;

export type EffectFilter = (typeof EFFECT_FILTERS)[number]["value"];

export interface EventFilters {
  type: EventTypeFilter;
  /** Free text, also offered as a searchable list of tools seen in this run. */
  tool: string;
  effect: EffectFilter;
}

export interface TimelineEventGroup {
  kind: "group";
  key: string;
  label: string;
  events: ExecutionEventRecord[];
}

export type TimelineEntry =
  | { kind: "event"; event: ExecutionEventRecord }
  | TimelineEventGroup;

const MAX_TOOL_LENGTH = 200;

export const DEFAULT_EVENT_FILTERS: EventFilters = {
  type: "all",
  tool: "",
  effect: "all",
};

function data(event: ExecutionEventRecord): Record<string, unknown> {
  return event.data && typeof event.data === "object" ? (event.data as Record<string, unknown>) : {};
}

function string(value: unknown): string | null {
  if (typeof value !== "string") return null;
  const trimmed = value.trim();
  return trimmed ? trimmed : null;
}

function isOneOf<T extends string>(
  value: string | null,
  options: ReadonlyArray<{ value: T }>,
  fallback: T,
): T {
  return options.some((option) => option.value === value) ? (value as T) : fallback;
}

/** Read the filter state from a URL so filtered timelines can be linked or reloaded. */
export function parseEventFilters(params: URLSearchParams): EventFilters {
  return {
    type: isOneOf(params.get("events"), EVENT_TYPE_FILTERS, "all"),
    tool: (params.get("tool") ?? "").trim().slice(0, MAX_TOOL_LENGTH),
    effect: isOneOf(params.get("effect"), EFFECT_FILTERS, "all"),
  };
}

/** Only non-default values go into the URL; unknown or empty values disappear. */
export function eventFilterParams(filters: EventFilters): Record<string, string> {
  const parsed = parseEventFilters(
    new URLSearchParams({
      events: filters.type,
      tool: filters.tool,
      effect: filters.effect,
    }),
  );
  const params: Record<string, string> = {};
  if (parsed.type !== "all") params.events = parsed.type;
  if (parsed.tool) params.tool = parsed.tool;
  if (parsed.effect !== "all") params.effect = parsed.effect;
  return params;
}

/** Approval lifecycle events are tool_call events with an `approval …` message. */
export function isApprovalEvent(event: ExecutionEventRecord): boolean {
  return /^approval (requested|approved|rejected|expired)\b/i.test(event.message.trim());
}

export function eventType(event: ExecutionEventRecord): EventTypeFilter {
  if (isApprovalEvent(event)) return "approvals";
  if (event.kind === "error" || event.level === "error") return "errors";
  if (event.kind === "output") return "messages";
  if (event.kind === "tool_call") return "tools";
  return "system";
}

/** Identifiers safe to show in a label or datalist: no arguments, no payload text. */
export function eventToolIdentities(event: ExecutionEventRecord): string[] {
  const payload = data(event);
  const args = payload.arguments && typeof payload.arguments === "object"
    ? (payload.arguments as Record<string, unknown>)
    : {};
  const candidates = [
    payload.tool,
    payload.capability,
    payload.connector,
    payload.connectorName,
    payload.connection,
    payload.connectionName,
    args.connector,
    args.connectorName,
    args.connection,
    args.connectionName,
  ];
  const seen = new Set<string>();
  const values: string[] = [];
  for (const candidate of candidates) {
    const value = string(candidate);
    if (!value || seen.has(value.toLowerCase())) continue;
    seen.add(value.toLowerCase());
    values.push(value);
  }
  return values;
}

export function eventEffect(event: ExecutionEventRecord): EffectFilter | "none" {
  const effect = string(data(event).effect);
  if (effect === "read" || effect === "write" || effect === "external_side_effect") return effect;
  return "none";
}

/** Matching text includes the server message, but never becomes a visible label. */
export function eventSearchText(event: ExecutionEventRecord): string {
  return [event.message, ...eventToolIdentities(event)].join(" ").toLowerCase();
}

export function matchesEventFilters(event: ExecutionEventRecord, filters: EventFilters): boolean {
  const parsed = parseEventFilters(
    new URLSearchParams({
      events: filters.type,
      tool: filters.tool,
      effect: filters.effect,
    }),
  );
  if (parsed.type !== "all" && eventType(event) !== parsed.type) return false;
  const needle = parsed.tool.toLowerCase();
  if (needle && !eventSearchText(event).includes(needle)) return false;
  if (parsed.effect !== "all" && eventEffect(event) !== parsed.effect) return false;
  return true;
}

/**
 * Story/Everything (`isKey`) runs first; filters then narrow that selection.
 * `hiddenCount` is measured against every event, not just the current page.
 */
export function filterEvents(
  events: ExecutionEventRecord[],
  filters: EventFilters,
  isKey: (event: ExecutionEventRecord) => boolean,
  showAll: boolean,
): { visible: ExecutionEventRecord[]; hiddenCount: number } {
  const visible = events.filter(
    (event) => (showAll || isKey(event)) && matchesEventFilters(event, filters),
  );
  return { visible, hiddenCount: events.length - visible.length };
}

function humanize(value: string): string {
  const words = value.replace(/[_-]+/g, " ").trim();
  return words ? `${words.charAt(0).toUpperCase()}${words.slice(1)}` : value;
}

/** "issues.comment" -> "Comment issues"; "github_issues_list" -> "Github issues list". */
export function toolLabel(identity: string): string {
  const [resource, action, ...rest] = identity.split(".");
  if (resource && action) return `${humanize(action)} ${humanize([resource, ...rest].join(" "))}`;
  return humanize(identity);
}

/** Tool/action choices for the datalist, in the order they first appear. */
export function toolOptions(events: ExecutionEventRecord[]): Array<{ value: string; label: string }> {
  const options: Array<{ value: string; label: string }> = [];
  const seen = new Set<string>();
  for (const event of events) {
    for (const identity of eventToolIdentities(event)) {
      const key = identity.toLowerCase();
      if (seen.has(key) || options.length >= 100) continue;
      seen.add(key);
      options.push({ value: identity, label: toolLabel(identity) });
    }
  }
  return options;
}

function groupKey(event: ExecutionEventRecord): string | null {
  // Errors and approvals are decisions a person must be able to see immediately.
  if (event.kind !== "tool_call" || event.level === "error" || isApprovalEvent(event)) return null;
  const payload = data(event);
  const identity = string(payload.capability) ?? string(payload.tool);
  return identity;
}

/**
 * Fold runs of the same tool/action into one collapsed row. Grouped lines are
 * still visible (the row says how many there are); filtering and ordering are
 * untouched. Errors, approvals and non-tool events are always left expanded.
 */
export function groupTimelineEvents(
  events: ExecutionEventRecord[],
  enabled: boolean,
): TimelineEntry[] {
  if (!enabled) return events.map((event) => ({ kind: "event", event }));

  const entries: TimelineEntry[] = [];
  let pending: TimelineEventGroup | null = null;
  const flush = () => {
    if (pending) entries.push(pending);
    pending = null;
  };

  for (const event of events) {
    const key = groupKey(event);
    if (key && pending?.key.toLowerCase() === key.toLowerCase()) {
      pending.events.push(event);
      continue;
    }
    flush();
    if (key) pending = { kind: "group", key, label: toolLabel(key), events: [event] };
    else entries.push({ kind: "event", event });
  }
  flush();
  return entries;
}

/**
 * Whether two renders of the same group show the same thing. `groupTimelineEvents`
 * builds new group objects on every stream flush; the timeline compares them with
 * this so a group whose events did not change is not rendered again.
 */
export function sameTimelineGroup(a: TimelineEventGroup, b: TimelineEventGroup): boolean {
  if (a === b) return true;
  if (a.key !== b.key || a.label !== b.label || a.events.length !== b.events.length) return false;
  return a.events.every((event, index) => event === b.events[index]);
}
