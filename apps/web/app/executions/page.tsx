"use client";

export const dynamic = "force-dynamic";

import { Suspense, useMemo } from "react";
import Link from "next/link";
import { usePathname, useRouter, useSearchParams } from "next/navigation";
import { Activity, Play, Workflow } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Callout } from "@/components/ui/callout";
import { Card } from "@/components/ui/card";
import { EmptyState } from "@/components/ui/empty-state";
import { PageHeader } from "@/components/ui/page-header";
import { Segmented } from "@/components/ui/segmented";
import { Select } from "@/components/ui/select";
import { Skeleton } from "@/components/ui/skeleton";
import { ExecutionListHeader, ExecutionRow } from "@/components/executions/execution-row";
import { usePagedExecutions } from "@/lib/queries";
import { errorMessage } from "@orbit/api-client";

const STATUSES = [
  { value: "all", label: "All" },
  { value: "running", label: "Running" },
  { value: "queued", label: "Queued" },
  { value: "awaiting_approval", label: "Needs approval" },
  { value: "succeeded", label: "Succeeded" },
  { value: "failed", label: "Failed" },
  { value: "cancelled", label: "Cancelled" },
] as const;

type StatusFilter = (typeof STATUSES)[number]["value"];
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
  const statusParam = searchParams.get("status") ?? "all";
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
    status: status === "all" ? undefined : status,
    limit: 50,
    archived: archived === "all" ? undefined : archived === "archived",
  });
  const executions = paged.first;

  const groups = useMemo(() => {
    const items = paged.items;
    const grouped = new Map<string, typeof items>();
    for (const execution of items) {
      const teamRunId = (execution as typeof execution & { teamRunId?: string | null }).teamRunId;
      const key = teamRunId ?? "standalone";
      const current = grouped.get(key) ?? [];
      current.push(execution);
      grouped.set(key, current);
    }
    return Array.from(grouped.entries());
  }, [paged.items]);

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
          options={STATUSES}
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
            {groups.map(([key, items]) =>
              key === "standalone" ? (
                items.map((execution) => <ExecutionRow key={execution.id} execution={execution} />)
              ) : (
                <div key={key} className="border-b last:border-b-0">
                  <div className="flex items-center gap-2 border-b bg-subtle px-4 py-2 text-[13px] sm:px-5">
                    <Workflow className="h-4 w-4 text-muted-foreground" />
                    <Link
                      href={`/agent-teams/runs/${key}`}
                      className="font-medium text-primary underline-offset-4 hover:underline"
                    >
                      Agent team run
                    </Link>
                    <span className="font-mono text-xs text-muted-foreground">{key.slice(0, 8)}</span>
                    <span className="ml-auto text-muted-foreground">
                      {items.length} step{items.length === 1 ? "" : "s"}
                    </span>
                  </div>
                  {items.map((execution) => (
                    <ExecutionRow key={execution.id} execution={execution} />
                  ))}
                </div>
              ),
            )}
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
