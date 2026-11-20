import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

export function titleCase(value: string): string {
  return value
    .replace(/[_-]+/g, " ")
    .replace(/\b\w/g, (char) => char.toUpperCase())
    .trim();
}

export function truncate(value: string, length = 120): string {
  if (value.length <= length) return value;
  return `${value.slice(0, length - 1).trimEnd()}…`;
}

export function formatDateTime(value?: string | null): string {
  if (!value) return "—";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "—";
  return date.toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    // 24-hour, matching the timeline and logs, so one page never mixes clocks.
    hourCycle: "h23",
  });
}

export function formatDate(value?: string | null): string {
  if (!value) return "—";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "—";
  return date.toLocaleDateString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
  });
}

export function relativeTime(value?: string | null): string {
  if (!value) return "—";
  const date = new Date(value);
  const diff = Date.now() - date.getTime();
  if (Number.isNaN(diff)) return "—";
  const abs = Math.abs(diff);
  const units: Array<[Intl.RelativeTimeFormatUnit, number]> = [
    ["year", 365 * 24 * 3600_000],
    ["month", 30 * 24 * 3600_000],
    ["day", 24 * 3600_000],
    ["hour", 3600_000],
    ["minute", 60_000],
    ["second", 1000],
  ];
  const formatter = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });
  for (const [unit, ms] of units) {
    if (abs >= ms || unit === "second") {
      return formatter.format(Math.round(-diff / ms), unit);
    }
  }
  return "just now";
}

export function formatDuration(start?: string | null, end?: string | null): string {
  if (!start) return "—";
  const startMs = new Date(start).getTime();
  const endMs = end ? new Date(end).getTime() : Date.now();
  if (Number.isNaN(startMs) || Number.isNaN(endMs)) return "—";
  return formatElapsed(endMs - startMs);
}

/** A span of milliseconds as the two most significant units: `850ms`, `4.2s`, `2m 5s`, `3h 12m`. */
export function formatElapsed(elapsedMs: number): string {
  const ms = Math.max(0, Math.round(elapsedMs));
  if (ms < 1000) return `${ms}ms`;
  // Round once, then split: rounding the remainder alone produced "1m 60s".
  if (ms < 59_950) return `${(ms / 1000).toFixed(1)}s`;
  const totalSeconds = Math.round(ms / 1000);
  if (totalSeconds < 3600) {
    return `${Math.floor(totalSeconds / 60)}m ${totalSeconds % 60}s`;
  }
  const totalMinutes = Math.round(totalSeconds / 60);
  if (totalMinutes < 24 * 60) {
    return `${Math.floor(totalMinutes / 60)}h ${totalMinutes % 60}m`;
  }
  const totalHours = Math.round(totalMinutes / 60);
  return `${Math.floor(totalHours / 24)}d ${totalHours % 24}h`;
}

export function formatTokens(value?: number | null): string {
  if (value === null || value === undefined) return "—";
  if (value < 1000) return String(value);
  // 999,960 must read "1.0M", not "1000.0k".
  if (value < 999_950) return `${(value / 1000).toFixed(1)}k`;
  return `${(value / 1_000_000).toFixed(1)}M`;
}

export const EXECUTION_STATUS_TONE: Record<
  string,
  "default" | "secondary" | "destructive" | "outline" | "success" | "warning"
> = {
  queued: "secondary",
  dispatched: "warning",
  running: "warning",
  awaiting_approval: "warning",
  succeeded: "success",
  failed: "destructive",
  timed_out: "destructive",
  cancelled: "outline",
  rejected: "destructive",
};

export const AGENT_STATUS_TONE: Record<
  string,
  "default" | "secondary" | "destructive" | "outline" | "success" | "warning"
> = {
  draft: "secondary",
  active: "success",
  paused: "warning",
  archived: "outline",
};

export function isTerminalStatus(status?: string | null): boolean {
  return ["succeeded", "failed", "timed_out", "cancelled", "rejected"].includes(
    status ?? "",
  );
}
