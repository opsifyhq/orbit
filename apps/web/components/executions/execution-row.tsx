import Link from "next/link";
import { ExecutionStatusBadge } from "@/components/ui/status-badge";
import { executionStatusMeta } from "@/components/ui/status";
import { cn, formatDuration, relativeTime, titleCase } from "@/lib/utils";
import type { ExecutionRecord } from "@/lib/types";

export function executionTask(execution: ExecutionRecord): string {
  const input = execution.input;
  return input && typeof input === "object" && "task" in input
    ? String((input as { task?: unknown }).task ?? "")
    : "";
}

export function executionTitle(execution: ExecutionRecord): string {
  const input = execution.input;
  const name =
    input && typeof input === "object" && "name" in input
      ? String((input as { name?: unknown }).name ?? "")
      : "";
  return name || execution.agentName;
}

/**
 * One execution in a list. Status leads (it is what people scan for), then what
 * ran, then when and how long. Collapses to two lines on a phone.
 */
export function ExecutionRow({
  execution,
  showAgent = true,
  className,
}: {
  execution: ExecutionRecord;
  showAgent?: boolean;
  className?: string;
}) {
  const task = executionTask(execution);
  const title = showAgent ? executionTitle(execution) : task || "Execution";
  const detail = showAgent ? task || "No task recorded" : titleCase(execution.triggerSource);
  const status = executionStatusMeta(execution.status);

  return (
    <Link
      href={`/executions/${execution.id}`}
      className={cn(
        "group grid grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-x-3 border-b px-4 py-3 transition-colors last:border-b-0 hover:bg-muted/50 focus-visible:outline-offset-[-2px] sm:grid-cols-[8.5rem_minmax(0,1fr)_auto] sm:px-5",
        className,
      )}
    >
      <ExecutionStatusBadge status={execution.status} className="hidden sm:inline-flex" />
      {/* The status is spelled out in the line below on phones; do not announce it twice. */}
      <span aria-hidden className="sm:hidden">
        <ExecutionStatusBadge status={execution.status} hideLabel className="h-4 w-4" />
      </span>
      <div className="min-w-0">
        <p className="truncate text-sm font-medium">{title}</p>
        <p className="truncate text-[13px] text-muted-foreground">
          <span className="sm:hidden">{status.label} · </span>
          {detail}
        </p>
      </div>
      <div className="tabular shrink-0 text-right text-[13px] text-muted-foreground">
        <p>{relativeTime(execution.createdAt)}</p>
        <p className="hidden text-xs sm:block">
          {execution.startedAt ? formatDuration(execution.startedAt, execution.finishedAt) : "Not started"}
        </p>
      </div>
    </Link>
  );
}

/** Column labels for a full executions list (shown from `sm`, where the columns exist). */
export function ExecutionListHeader({ showAgent = true }: { showAgent?: boolean }) {
  return (
    <div
      aria-hidden
      className="hidden grid-cols-[8.5rem_minmax(0,1fr)_auto] gap-x-3 border-b bg-subtle px-5 py-2 text-xs font-medium text-muted-foreground sm:grid"
    >
      <span>Status</span>
      <span>{showAgent ? "Execution" : "Task"}</span>
      <span className="text-right">Started and duration</span>
    </div>
  );
}
