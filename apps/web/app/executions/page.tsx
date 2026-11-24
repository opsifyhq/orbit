"use client";

export const dynamic = "force-dynamic";

import { Suspense, useMemo, useState } from "react";
import Link from "next/link";
import { usePathname, useRouter, useSearchParams } from "next/navigation";
import { Activity, CalendarClock, ChevronDown, ChevronRight, Play, Workflow } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Callout } from "@/components/ui/callout";
import { Card } from "@/components/ui/card";
import { EmptyState } from "@/components/ui/empty-state";
import { PageHeader } from "@/components/ui/page-header";
import { Segmented } from "@/components/ui/segmented";
import { Select } from "@/components/ui/select";
import { Skeleton } from "@/components/ui/skeleton";
import { ExecutionListHeader, ExecutionRow } from "@/components/executions/execution-row";
import { ExecutionStatusBadge } from "@/components/ui/status-badge";
import { usePagedExecutions } from "@/lib/queries";
import { errorMessage } from "@orbit/api-client";

/**
 * Filters are the groups people ask about, not raw statuses: every status belongs to
 * exactly one, so nothing (dispatched, timed out, rejected) is unreachable, and the
 * Overview tiles link to the same sets they count.
 */
const STATUSES = [
  { value: "all", label: "All", statuses: undefined },
  { value: "active", label: "In flight", statuses: "queued,dispatched,running" },
  { value: "awaiting_approval", label: "Needs approval", statuses: "awaiting_approval" },
  { value: "succeeded", label: "Succeeded", statuses: "succeeded" },
  { value: "failed", label: "Failed", statuses: "failed,timed_out" },
  { value: "cancelled", label: "Cancelled", statuses: "cancelled,rejected" },
] as const;

type StatusFilter = (typeof STATUSES)[number]["value"];

/** Links from before the groups existed keep working. */
const LEGACY_STATUS: Record<string, StatusFilter> = {
  running: "active",
  queued: "active",
  dispatched: "active",
  timed_out: "failed",
  rejected: "cancelled",
};
type ArchiveFilter = "active" | "archived" | "all";

export default function ExecutionsPage() {
  // useSearchParams needs a Suspense boundary so the page can still prerender.
  return (
    <Suspense fallback={<ListSkeleton />}>
      <Executions />
    </Suspense>
  );
}

function ListSkeleton() {
  return (
    <Card className="overflow-hidden">
      {Array.from({ length: 6 }).map((_, index) => (
        <div key={index} className="border-b px-5 py-3.5 last:border-b-0">
          <Skeleton className="h-9" />
        </div>
      ))}
    </Card>
  );
}

function Executions() {
  const router = useRouter();
  const pathname = usePathname();
  const searchParams = useSearchParams();

  // Filters live in the URL so a filtered view can be linked, bookmarked and
  // survives the back button after opening an execution.
  const rawStatus = searchParams.get("status") ?? "all";
  const statusParam = LEGACY_STATUS[rawStatus] ?? rawStatus;
  const status: StatusFilter = STATUSES.some((item) => item.value === statusParam)
    ? (statusParam as StatusFilter)
    : "all";
  const archiveParam = searchParams.get("view");
  const archived: ArchiveFilter =
    archiveParam === "archived" || archiveParam === "all" ? archiveParam : "active";

  function setParam(key: string, value: string, fallback: string) {
    const next = new URLSearchParams(searchParams.toString());
    if (value === fallback) next.delete(key);
    else next.set(key, value);
    const query = next.toString();
    router.replace(query ? `${pathname}?${query}` : pathname, { scroll: false });
  }

  const paged = usePagedExecutions({
    status: STATUSES.find((item) => item.value === status)?.statuses,
    limit: 50,
    archived: archived === "all" ? undefined : archived === "archived",
  });
  const executions = paged.first;

  const [collapsedGroups, setCollapsedGroups] = useState<Set<string>>(() => new Set());
  const [expandedSchedules, setExpandedSchedules] = useState<Set<string>>(() => new Set());

  const groups = useMemo(() => {
    const grouped = new Map<
      string,
      { kind: "team" | "schedule" | "standalone"; items: typeof paged.items }
    >();
    for (const execution of paged.items) {
      const teamRunId = (execution as typeof execution & { teamRunId?: string | null }).teamRunId;
      const scheduled = execution.triggerSource === "schedule";
      const key = teamRunId
        ? `team:${teamRunId}`
        : scheduled
          ? `schedule:${execution.triggerId ?? execution.agentId}`
          : `execution:${execution.id}`;
      const kind = teamRunId ? "team" : scheduled ? "schedule" : "standalone";
      const current = grouped.get(key);
      if (current) current.items.push(execution);
      else grouped.set(key, { kind, items: [execution] });
    }
    return Array.from(grouped.entries());
  }, [paged.items]);

  function isGroupOpen(key: string, kind: "team" | "schedule" | "standalone") {
    if (kind === "schedule") return expandedSchedules.has(key);
    return !collapsedGroups.has(key);
  }

  function toggleGroup(key: string, kind: "team" | "schedule" | "standalone") {
    if (kind === "schedule") {
      setExpandedSchedules((current) => {
        const next = new Set(current);
        if (next.has(key)) next.delete(key);
        else next.add(key);
        return next;
      });
      return;
    }
    setCollapsedGroups((current) => {
      const next = new Set(current);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  }

  const filtered = status !== "all" || archived !== "active";
  const shown = paged.items.length;
  const total = paged.total;

  return (
    <div className="space-y-6">
      <PageHeader
        title="Executions"
        description="Every time an agent ran: what it was asked, what happened, and what it returned."
        actions={
          <Button asChild className="lg:hidden">
            <Link href="/work">
              <Play /> Start work
            </Link>
          </Button>
        }
      />

      <div className="flex flex-col gap-3 md:flex-row md:items-center md:justify-between">
        <Segmented
          label="Execution status"
          value={status}
          onChange={(value) => setParam("status", value, "all")}
          options={STATUSES.map(({ value, label }) => ({ value, label }))}
        />
        <label className="flex shrink-0 items-center gap-2 text-[13px] text-muted-foreground">
          Show
          <Select
            className="w-auto"
            value={archived}
            onChange={(event) => setParam("view", event.target.value, "active")}
          >
            <option value="active">Current</option>
            <option value="archived">Archived</option>
            <option value="all">Current and archived</option>
          </Select>
        </label>
      </div>

      {executions.isLoading ? (
        <ListSkeleton />
      ) : executions.isError ? (
        <Callout tone="danger" title="Executions could not be loaded">
          {errorMessage(executions.error)}
        </Callout>
      ) : shown ? (
        <>
          <Card className="overflow-hidden">
            <ExecutionListHeader />
            {groups.map(([key, group]) => {
              if (group.kind === "standalone" || (group.kind === "schedule" && group.items.length === 1)) {
                return group.items.map((execution) => (
                  <ExecutionRow key={execution.id} execution={execution} />
                ));
              }
              const open = isGroupOpen(key, group.kind);
              const latest = group.items[0];
              return (
                <div key={key} className="border-b last:border-b-0">
                  <button
                    type="button"
                    onClick={() => toggleGroup(key, group.kind)}
                    aria-expanded={open}
                    className="flex w-full items-center gap-2 border-b bg-subtle px-4 py-2 text-left text-[13px] hover:bg-muted/50 sm:px-5"
                  >
                    {open ? (
                      <ChevronDown className="h-4 w-4 shrink-0 text-muted-foreground" />
                    ) : (
                      <ChevronRight className="h-4 w-4 shrink-0 text-muted-foreground" />
                    )}
                    {group.kind === "team" ? (
                      <>
                        <Workflow className="h-4 w-4 shrink-0 text-muted-foreground" />
                        <Link
                          href={`/agent-teams/runs/${key.slice("team:".length)}`}
                          className="font-medium text-primary underline-offset-4 hover:underline"
                          onClick={(event) => event.stopPropagation()}
                        >
                          Agent team run
                        </Link>
                        <span className="font-mono text-xs text-muted-foreground">
                          {key.slice("team:".length, "team:".length + 8)}
                        </span>
                      </>
                    ) : (
                      <>
                        <CalendarClock className="h-4 w-4 shrink-0 text-muted-foreground" />
                        <span className="font-medium">Scheduled runs</span>
                        <span className="truncate text-muted-foreground">{latest.agentName}</span>
                        <ExecutionStatusBadge status={latest.status} hideLabel className="h-4 w-4" />
                      </>
                    )}
                    <span className="ml-auto shrink-0 text-muted-foreground">
                      {group.items.length} {group.kind === "team" ? "step" : "run"}
                      {group.items.length === 1 ? "" : "s"}
                    </span>
                  </button>
                  {open
                    ? group.items.map((execution) => (
                        <ExecutionRow key={execution.id} execution={execution} />
                      ))
                    : null}
                </div>
              );
            })}
          </Card>
          <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
            <p className="tabular text-[13px] text-muted-foreground" role="status">
              {paged.hasMore
                ? `Showing the latest ${shown} of ${total}.`
                : `${total} execution${total === 1 ? "" : "s"}.`}
            </p>
            {paged.hasMore ? (
              <Button variant="outline" size="sm" onClick={() => void paged.loadMore()} loading={paged.loadingMore}>
                Load older
              </Button>
            ) : null}
          </div>
          {paged.loadError ? (
            <Callout tone="danger" title="Older executions could not be loaded">
              {errorMessage(paged.loadError)}
            </Callout>
          ) : null}
        </>
      ) : (
        <EmptyState
          icon={<Activity />}
          title={filtered ? "No executions match these filters" : "No executions yet"}
          description={
            filtered
              ? "Nothing has this status in the selected view."
              : "Start work with an agent or add a trigger. Each execution appears here with a live timeline."
          }
          action={
            filtered ? (
              <Button variant="outline" onClick={() => router.replace(pathname, { scroll: false })}>
                Clear filters
              </Button>
            ) : (
              <Button asChild>
                <Link href="/work">
                  <Play /> Start work
                </Link>
              </Button>
            )
          }
        />
      )}
    </div>
  );
}
