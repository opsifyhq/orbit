"use client";

export const dynamic = "force-dynamic";

import { useEffect, useState } from "react";
import Link from "next/link";
import { Play } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Callout } from "@/components/ui/callout";
import { EmptyState } from "@/components/ui/empty-state";
import { PageHeader } from "@/components/ui/page-header";
import {
  ListCard,
  ListCount,
  ListRow,
  ListSkeleton,
  ListToolbar,
  ROW_LINK,
  SearchField,
} from "@/components/ui/resource-list";
import { Segmented } from "@/components/ui/segmented";
import { ExecutionStatusBadge } from "@/components/ui/status-badge";
import { useAgentTeams, usePagedTeamRuns } from "@/lib/queries";
import { formatDuration, relativeTime } from "@/lib/utils";
import { errorMessage } from "@orbit/api-client";

type Filter = "active" | "archived" | "all";

/** Every agent team run, in the same list layout as the registries. */
export default function TeamRunsPage() {
  const [filter, setFilter] = useState<Filter>("active");
  const [search, setSearch] = useState("");
  // The server does the filtering; avoid a request for every keystroke.
  const [searchQuery, setSearchQuery] = useState("");
  useEffect(() => {
    const timer = window.setTimeout(() => setSearchQuery(search.trim()), 200);
    return () => window.clearTimeout(timer);
  }, [search]);

  const runs = usePagedTeamRuns({
    archived: filter === "all" ? undefined : filter === "archived",
    q: searchQuery || undefined,
    limit: 100,
  });
  const teams = useAgentTeams();
  const teamName = (id: string) => teams.data?.find((team) => team.id === id)?.name;
  const all = runs.data?.pages.flatMap((page) => page.items) ?? [];
  const total = runs.data?.pages[0]?.total ?? 0;

  return (
    <div className="space-y-6">
      <PageHeader
        back={{ href: "/agent-teams", label: "Agent teams" }}
        title="Team runs"
        description="Every run of every agent team. Open one to follow its steps; start new runs from a team."
      />

      <ListToolbar>
        <SearchField value={search} onChange={setSearch} placeholder="Filter by task, team or status" label="Filter runs" />
        <Segmented
          label="Show runs"
          value={filter}
          onChange={setFilter}
          options={[
            { value: "active", label: "Active" },
            { value: "archived", label: "Archived" },
            { value: "all", label: "All" },
          ]}
        />
      </ListToolbar>

      {runs.isLoading ? (
        <ListSkeleton />
      ) : runs.isError ? (
        <Callout tone="danger" title="Runs could not be loaded">
          {errorMessage(runs.error)}
        </Callout>
      ) : all.length ? (
        <>
          <ListCard columns={["Run", "Status", "Team", "Started"]} selectable={false}>
            {all.map((run) => {
              const task = String(run.input.task ?? "") || "Agent team run";
              const team = teamName(run.teamId) ?? "Agent team";
              return (
                <ListRow
                  key={run.id}
                  name={
                    <>
                      <Link href={`/agent-teams/runs/${run.id}`} className={`block ${ROW_LINK}`}>
                        {task}
                      </Link>
                      {run.error ? (
                        <p className="mt-0.5 line-clamp-1 text-[13px] text-destructive">{run.error}</p>
                      ) : (
                        <p className="mt-0.5 line-clamp-1 text-[13px] text-muted-foreground">
                          {run.finishedAt ? `Took ${formatDuration(run.createdAt, run.finishedAt)}` : "Not finished"}
                        </p>
                      )}
                      <p className="mt-1.5 flex flex-wrap gap-x-3 text-xs text-muted-foreground md:hidden">
                        <span>{team}</span>
                        <span>{relativeTime(run.createdAt)}</span>
                      </p>
                    </>
                  }
                  status={<ExecutionStatusBadge status={run.status} />}
                  details={<p className="truncate">{team}</p>}
                  updated={relativeTime(run.createdAt)}
                />
              );
            })}
          </ListCard>
          <ListCount shown={all.length} total={total} singular="run" plural="runs" verb="shown" />
          {runs.hasNextPage ? (
            <div className="mt-3">
              <Button
                variant="outline"
                loading={runs.isFetchingNextPage}
                onClick={() => void runs.fetchNextPage()}
              >
                Load older runs
              </Button>
            </div>
          ) : null}
        </>
      ) : (
        <EmptyState
          icon={<Play />}
          title={search ? `No runs match “${search}”` : filter === "active" ? "No runs yet" : `No ${filter === "all" ? "" : "archived "}runs`}
          description={
            search
              ? "Try a shorter term, or clear the filter."
              : filter === "active"
                ? "Open a team and choose Run. Its runs show up here."
                : "Nothing matches this filter."
          }
          action={
            search ? (
              <Button variant="outline" onClick={() => setSearch("")}>
                Clear filter
              </Button>
            ) : filter !== "active" ? (
              <Button variant="outline" onClick={() => setFilter("active")}>
                Show active runs
              </Button>
            ) : (
              <Button asChild variant="outline">
                <Link href="/agent-teams">Open agent teams</Link>
              </Button>
            )
          }
        />
      )}
    </div>
  );
}
