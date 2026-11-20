import { Cron } from "croner";

/**
 * Schedules are stored as a 5-field cron expression plus an optional IANA time zone
 * (UTC when omitted); the API evaluates them with the same rules, including daylight
 * saving. This module turns the common shapes into a plain model the schedule editor
 * can show as ordinary controls, and back. Anything it does not recognize stays as
 * custom cron, untouched.
 */

export type Frequency = "minutes" | "hourly" | "daily" | "weekly" | "monthly" | "custom";

export interface ScheduleModel {
  frequency: Frequency;
  /** Minutes between runs (minutes) or hours between runs (hourly). */
  every: number;
  minute: number;
  hour: number;
  /** Weekdays for "weekly", 0 = Sunday … 6 = Saturday. */
  days: number[];
  /** Day of the month for "monthly", 1–31. */
  dayOfMonth: number;
}

export const DEFAULT_MODEL: ScheduleModel = {
  frequency: "weekly",
  every: 15,
  minute: 0,
  hour: 9,
  days: [1, 2, 3, 4, 5],
  dayOfMonth: 1,
};

export const WEEKDAYS = [1, 2, 3, 4, 5];
export const WEEKEND = [0, 6];
/** Monday first, the way most calendars outside the US read. */
export const DAY_ORDER = [1, 2, 3, 4, 5, 6, 0];
const DAY_NAMES = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
export const DAY_SHORT = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

const int = (value: string, min: number, max: number): number | null => {
  if (!/^\d{1,2}$/.test(value)) return null;
  const n = Number(value);
  return n >= min && n <= max ? n : null;
};

/** `*`, or `*\/N` → N (1 for `*`). Null for anything else. */
const step = (value: string, max: number): number | null => {
  if (value === "*") return 1;
  const match = /^\*\/(\d{1,2})$/.exec(value);
  if (!match) return null;
  const n = Number(match[1]);
  return n >= 1 && n <= max ? n : null;
};

/** `1-5`, `0,6`, `1,3,5`, `7` (Sunday) → sorted day numbers; null when not a plain list. */
function parseDays(value: string): number[] | null {
  const days = new Set<number>();
  for (const part of value.split(",")) {
    const range = /^(\d)(?:-(\d))?$/.exec(part);
    if (!range) return null;
    const from = Number(range[1]);
    const to = range[2] === undefined ? from : Number(range[2]);
    if (from > 7 || to > 7 || to < from) return null;
    for (let day = from; day <= to; day++) days.add(day % 7);
  }
  return [...days].sort((a, b) => a - b);
}

/** Sorted day numbers → the shortest cron list (`1-5`, `0,6`, `1,3-5`). */
function formatDays(days: number[]): string {
  const sorted = [...new Set(days)].sort((a, b) => a - b);
  const parts: string[] = [];
  for (let i = 0; i < sorted.length; ) {
    let j = i;
    while (j + 1 < sorted.length && sorted[j + 1] === sorted[j] + 1) j++;
    parts.push(j - i >= 2 ? `${sorted[i]}-${sorted[j]}` : sorted.slice(i, j + 1).join(","));
    i = j + 1;
  }
  return parts.join(",");
}

/** Read a cron expression as one of the simple shapes; `custom` when it is anything else. */
export function parseCron(cron: string): ScheduleModel {
  const custom = { ...DEFAULT_MODEL, frequency: "custom" as const };
  const fields = cron.trim().split(/\s+/);
  if (fields.length !== 5) return custom;
  const [mi, h, dom, mon, dow] = fields;
  if (mon !== "*") return custom;

  const minutes = step(mi, 59);
  if (minutes !== null && h === "*" && dom === "*" && dow === "*") {
    return { ...DEFAULT_MODEL, frequency: "minutes", every: minutes };
  }
  const minute = int(mi, 0, 59);
  if (minute === null) return custom;

  const hours = step(h, 23);
  if (hours !== null && dom === "*" && dow === "*") {
    return { ...DEFAULT_MODEL, frequency: "hourly", every: hours, minute };
  }
  const hour = int(h, 0, 23);
  if (hour === null) return custom;

  if (dom === "*" && dow === "*") return { ...DEFAULT_MODEL, frequency: "daily", minute, hour };
  if (dom === "*") {
    const days = parseDays(dow);
    if (!days?.length) return custom;
    if (days.length === 7) return { ...DEFAULT_MODEL, frequency: "daily", minute, hour };
    return { ...DEFAULT_MODEL, frequency: "weekly", minute, hour, days };
  }
  const dayOfMonth = int(dom, 1, 31);
  if (dayOfMonth !== null && dow === "*") return { ...DEFAULT_MODEL, frequency: "monthly", minute, hour, dayOfMonth };
  return custom;
}

/** The cron expression for a simple shape. Not defined for `custom`. */
export function buildCron(model: ScheduleModel): string {
  const { minute, hour } = model;
  switch (model.frequency) {
    case "minutes":
      return model.every <= 1 ? "* * * * *" : `*/${model.every} * * * *`;
    case "hourly":
      return model.every <= 1 ? `${minute} * * * *` : `${minute} */${model.every} * * *`;
    case "daily":
      return `${minute} ${hour} * * *`;
    case "weekly":
      return model.days.length === 0 || model.days.length === 7
        ? `${minute} ${hour} * * *`
        : `${minute} ${hour} * * ${formatDays(model.days)}`;
    case "monthly":
      return `${minute} ${hour} ${model.dayOfMonth} * *`;
    case "custom":
      throw new Error("custom schedules keep their own cron expression");
  }
}

const pad = (n: number) => String(n).padStart(2, "0");
export const clock = (hour: number, minute: number) => `${pad(hour)}:${pad(minute)}`;

const ordinal = (n: number) => {
  const tens = n % 100;
  if (tens >= 11 && tens <= 13) return `${n}th`;
  return `${n}${["th", "st", "nd", "rd"][n % 10] ?? "th"}`;
};

function joinWords(words: string[]): string {
  if (words.length <= 1) return words.join("");
  return `${words.slice(0, -1).join(", ")} and ${words[words.length - 1]}`;
}

export function describeDays(days: number[]): string {
  const set = [...new Set(days)].sort((a, b) => a - b).join(",");
  if (set === "1,2,3,4,5") return "Every weekday";
  if (set === "0,6") return "Every weekend day";
  if (days.length === 7) return "Every day";
  const ordered = DAY_ORDER.filter((day) => days.includes(day));
  return `Every ${joinWords(ordered.map((day) => DAY_NAMES[day]))}`;
}

/** "Amsterdam time", "UTC". */
export function zoneLabel(timezone?: string | null): string {
  if (!timezone || timezone === "UTC" || timezone === "Etc/UTC") return "UTC";
  const city = timezone.split("/").pop() ?? timezone;
  return `${city.replace(/_/g, " ")} time`;
}

/**
 * Plain-English description. Clock times carry the zone they are evaluated in;
 * custom expressions fall back to the cron itself.
 */
export function describeSchedule(cron: string | null | undefined, timezone?: string | null): string {
  if (!cron?.trim()) return "No schedule";
  const model = parseCron(cron);
  const at = `at ${clock(model.hour, model.minute)} ${zoneLabel(timezone)}`;
  switch (model.frequency) {
    case "minutes":
      return model.every === 1 ? "Every minute" : `Every ${model.every} minutes`;
    case "hourly": {
      const past = model.minute === 0 ? "on the hour" : `at ${pad(model.minute)} past`;
      return model.every === 1 ? `Every hour, ${past}` : `Every ${model.every} hours, ${past}`;
    }
    case "daily":
      return `Every day ${at}`;
    case "weekly":
      return `${describeDays(model.days)} ${at}`;
    case "monthly":
      return `On the ${ordinal(model.dayOfMonth)} of every month ${at}`;
    case "custom":
      return `Cron ${cron.trim()}${timezone ? ` (${zoneLabel(timezone)})` : " (UTC)"}`;
  }
}

function cronOf(cron: string, timezone?: string | null): Cron {
  // `legacyMode` keeps the classic rule the API uses: when both day of month and day
  // of week are restricted, either one matching is enough.
  return new Cron(cron.trim(), { paused: true, timezone: timezone || "UTC", legacyMode: true });
}

/** Why a cron expression is not usable, or null when it is. */
export function cronError(cron: string, timezone?: string | null): string | null {
  const fields = cron.trim().split(/\s+/).filter(Boolean);
  if (fields.length !== 5) return "Use five fields: minute, hour, day of month, month, day of week.";
  try {
    const next = cronOf(cron, timezone).nextRun();
    return next ? null : "This schedule never runs.";
  } catch (error) {
    const message = error instanceof Error ? error.message.replace(/^CronPattern:\s*/, "") : "";
    return message ? `Not a valid cron expression: ${message}` : "Not a valid cron expression.";
  }
}

/** The next `count` runs, or an empty list when the expression is invalid. */
export function nextRuns(cron: string, timezone: string | null | undefined, count = 3, from = new Date()): Date[] {
  try {
    return cronOf(cron, timezone).nextRuns(count, from);
  } catch {
    return [];
  }
}

/** The browser's time zone, falling back to UTC. */
export function browserTimeZone(): string {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
  } catch {
    return "UTC";
  }
}

/** Every IANA zone the browser knows, UTC first. */
export function timeZones(): string[] {
  let zones: string[] = [];
  try {
    zones = (Intl as unknown as { supportedValuesOf?: (key: string) => string[] }).supportedValuesOf?.("timeZone") ?? [];
  } catch {
    zones = [];
  }
  return ["UTC", ...zones.filter((zone) => zone !== "UTC")];
}

/** "Mon 23 Nov, 09:00" in `timezone`. */
export function formatRun(date: Date, timezone?: string | null): string {
  return new Intl.DateTimeFormat(undefined, {
    timeZone: timezone || "UTC",
    weekday: "short",
    day: "numeric",
    month: "short",
    hour: "2-digit",
    minute: "2-digit",
    hourCycle: "h23",
  }).format(date);
}

/** "10:00" in `timezone`, to show a run in the viewer's own time. */
export function formatClock(date: Date, timezone?: string | null): string {
  return new Intl.DateTimeFormat(undefined, {
    timeZone: timezone || "UTC",
    hour: "2-digit",
    minute: "2-digit",
    hourCycle: "h23",
  }).format(date);
}
