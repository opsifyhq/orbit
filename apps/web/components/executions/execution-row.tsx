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
    // The row is its own size container: the status column appears when the row is wide
    // enough, whatever the viewport, so a narrow card (Overview with the sidebar open)
    // keeps the name readable instead of cutting it to a few letters (#456).
    <Link
      href={`/executions/${execution.id}`}
      className={cn(
        "group block border-b transition-colors [container-type:inline-size] last:border-b-0 hover:bg-muted/50 focus-visible:outline-offset-[-2px]",
        className,
      )}
    >
      <div className="grid grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-x-3 px-4 py-3 sm:px-5 [@container(min-width:32rem)]:grid-cols-[8.5rem_minmax(0,1fr)_auto]">
        <ExecutionStatusBadge status={execution.status} className="hidden [@container(min-width:32rem)]:inline-flex" />
        {/* On a narrow row the status is spelled out in the line below; do not announce it twice. */}
        <span aria-hidden className="[@container(min-width:32rem)]:hidden">
          <ExecutionStatusBadge status={execution.status} hideLabel className="h-4 w-4" />
        </span>
        <div className="min-w-0">
          <p className="truncate text-sm font-medium">{title}</p>
          <p className="truncate text-[13px] text-muted-foreground">
            <span className="[@container(min-width:32rem)]:hidden">{status.label} · </span>
            {detail}
          </p>
        </div>
        <div className="tabular shrink-0 text-right text-[13px] text-muted-foreground">
          <p>{relativeTime(execution.createdAt)}</p>
          <p className="hidden text-xs [@container(min-width:32rem)]:block">
            {execution.startedAt ? formatDuration(execution.startedAt, execution.finishedAt) : "Not started"}
          </p>
        </div>
      </div>
    </Link>
  );
}

/** Column labels for a full executions list, shown at the same width as the row columns. */
export function ExecutionListHeader({ showAgent = true }: { showAgent?: boolean }) {
  return (
    <div aria-hidden className="[container-type:inline-size]">
      <div className="hidden grid-cols-[8.5rem_minmax(0,1fr)_auto] gap-x-3 border-b bg-subtle px-5 py-2 text-xs font-medium text-muted-foreground [@container(min-width:32rem)]:grid">
        <span>Status</span>
        <span>{showAgent ? "Execution" : "Task"}</span>
        <span className="text-right">Started and duration</span>
      </div>
    </div>
  );
}
