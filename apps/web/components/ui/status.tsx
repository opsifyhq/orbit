import { cn, titleCase } from "@/lib/utils";

/**
 * The orbit status glyph: a ring with a satellite. Shape carries the state, colour
 * reinforces it, and the label says it in words, so status never depends on colour
 * alone. Only live work moves.
 */
export type StatusTone = "neutral" | "info" | "success" | "warning" | "danger";

type GlyphShape =
  | "dashed" // waiting, not started
  | "satellite" // handed to a worker
  | "orbiting" // running
  | "paused" // held for a person
  | "solid" // finished well / healthy
  | "cross" // failed or refused
  | "alert" // ran out of time
  | "slash" // stopped on purpose / retired
  | "ring"; // unknown

const TONE_TEXT: Record<StatusTone, string> = {
  neutral: "text-muted-foreground",
  info: "text-info",
  success: "text-success",
  warning: "text-warning",
  danger: "text-destructive",
};

export interface StatusMeta {
  label: string;
  tone: StatusTone;
  shape: GlyphShape;
}

const EXECUTION_STATUS: Record<string, StatusMeta> = {
  queued: { label: "Queued", tone: "neutral", shape: "dashed" },
  dispatched: { label: "Dispatched", tone: "info", shape: "satellite" },
  running: { label: "Running", tone: "info", shape: "orbiting" },
  awaiting_approval: { label: "Needs approval", tone: "warning", shape: "paused" },
  succeeded: { label: "Succeeded", tone: "success", shape: "solid" },
  failed: { label: "Failed", tone: "danger", shape: "cross" },
  timed_out: { label: "Timed out", tone: "danger", shape: "alert" },
  cancelled: { label: "Cancelled", tone: "neutral", shape: "slash" },
  rejected: { label: "Rejected", tone: "danger", shape: "cross" },
  // Team runs and steps reuse the same vocabulary.
  pending: { label: "Pending", tone: "neutral", shape: "dashed" },
  completed: { label: "Completed", tone: "success", shape: "solid" },
  skipped: { label: "Skipped", tone: "neutral", shape: "slash" },
};

const AGENT_STATUS: Record<string, StatusMeta> = {
  active: { label: "Active", tone: "success", shape: "solid" },
  draft: { label: "Draft", tone: "neutral", shape: "dashed" },
  paused: { label: "Paused", tone: "warning", shape: "paused" },
  archived: { label: "Archived", tone: "neutral", shape: "slash" },
};

export function executionStatusMeta(status: string): StatusMeta {
  return EXECUTION_STATUS[status] ?? { label: titleCase(status), tone: "neutral", shape: "ring" };
}

export function agentStatusMeta(status: string): StatusMeta {
  return AGENT_STATUS[status] ?? { label: titleCase(status), tone: "neutral", shape: "ring" };
}

export function StatusGlyph({
  shape,
  tone,
  className,
  title,
}: {
  shape: GlyphShape;
  tone: StatusTone;
  className?: string;
  title?: string;
}) {
  return (
    <svg
      viewBox="0 0 16 16"
      className={cn("h-3.5 w-3.5 shrink-0", TONE_TEXT[tone], className)}
      fill="none"
      stroke="currentColor"
      strokeWidth="1.6"
      strokeLinecap="round"
      role={title ? "img" : undefined}
      aria-label={title}
      aria-hidden={title ? undefined : true}
    >
      {shape === "dashed" ? <circle cx="8" cy="8" r="5.6" strokeDasharray="2.2 2.6" /> : null}
      {shape === "ring" ? <circle cx="8" cy="8" r="5.6" /> : null}
      {shape === "satellite" ? (
        <>
          <circle cx="8" cy="8" r="5" strokeOpacity="0.4" />
          <circle cx="13" cy="8" r="2" fill="currentColor" stroke="none" />
        </>
      ) : null}
      {shape === "orbiting" ? (
        <>
          <circle cx="8" cy="8" r="5" strokeOpacity="0.4" />
          <g style={{ transformOrigin: "8px 8px", animation: "orbit-spin 1.5s linear infinite" }}>
            <circle cx="13" cy="8" r="2" fill="currentColor" stroke="none" />
          </g>
        </>
      ) : null}
      {shape === "paused" ? (
        <>
          <circle cx="8" cy="8" r="5.6" />
          <path d="M6.5 6v4M9.5 6v4" />
        </>
      ) : null}
      {shape === "solid" ? (
        <>
          <circle cx="8" cy="8" r="6.4" fill="currentColor" stroke="none" />
          <path d="M5.4 8.2l1.8 1.8 3.4-3.7" stroke="hsl(var(--card))" strokeWidth="1.7" strokeLinejoin="round" />
        </>
      ) : null}
      {shape === "cross" ? (
        <>
          <circle cx="8" cy="8" r="6.4" fill="currentColor" stroke="none" />
          <path d="M6 6l4 4M10 6l-4 4" stroke="hsl(var(--card))" strokeWidth="1.7" />
        </>
      ) : null}
      {shape === "alert" ? (
        <>
          <circle cx="8" cy="8" r="5.6" />
          <path d="M8 5.2v3.2" />
          <circle cx="8" cy="10.7" r="0.6" fill="currentColor" stroke="none" />
        </>
      ) : null}
      {shape === "slash" ? (
        <>
          <circle cx="8" cy="8" r="5.6" strokeOpacity="0.7" />
          <path d="M4.4 11.6l7.2-7.2" strokeOpacity="0.7" />
        </>
      ) : null}
    </svg>
  );
}

/** Glyph + label. The standard way to show any status in a row or header. */
export function Status({
  meta,
  className,
  hideLabel,
}: {
  meta: StatusMeta;
  className?: string;
  hideLabel?: boolean;
}) {
  if (hideLabel) {
    return <StatusGlyph shape={meta.shape} tone={meta.tone} title={meta.label} className={className} />;
  }
  return (
    <span
      className={cn(
        "inline-flex items-center gap-1.5 whitespace-nowrap text-[13px] font-medium",
        TONE_TEXT[meta.tone],
        className,
      )}
    >
      <StatusGlyph shape={meta.shape} tone={meta.tone} />
      {meta.label}
    </span>
  );
}
