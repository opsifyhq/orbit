"use client";

import { useMemo, useState } from "react";
import Link from "next/link";
import { Megaphone, Plus } from "lucide-react";

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
import { Status } from "@/components/ui/status";
import {
  OUTPUT_STATUS,
  deliveryFailed,
  deliveryReason,
  eventsLabel,
  kindLabel,
  outputProblem,
  outputState,
  type OutputState,
} from "@/lib/output-meta";
import type { OutputView } from "@/lib/outputs";
import { useAgentTeams, useAllAgents, useOutputs } from "@/lib/queries";
import { relativeTime } from "@/lib/utils";
import { errorMessage } from "@orbit/api-client";

type Filter = "all" | OutputState;

const FILTER_LABEL: Record<Filter, string> = {
  all: "All",
  sending: "On",
  failing: "Failing",
  off: "Off",
  incomplete: "Cannot send",
};

/** "Delivered 5 minutes ago", "Failed 2 hours ago · HTTP 404" or "Nothing sent yet". */
function lastDeliveryText(output: OutputView): string {
  const last = output.lastDelivery;
  if (!last) return "Nothing sent yet";
  const when = relativeTime(last.updatedAt);
  const reason = deliveryReason(last);
  switch (last.status) {
    case "succeeded":
      return `Delivered ${when}`;
    case "failed":
    case "dead":
      return `Failed ${when}${reason ? ` · ${reason}` : ""}`;
    case "pending":
      return last.attempts > 0 ? `Retrying${reason ? ` · ${reason}` : ""}` : "Queued";
    case "sending":
      return "Sending now";
    default:
      return `Skipped ${when}`;
  }
}

export default function OutputsPage() {
  const outputs = useOutputs();
  const agents = useAllAgents();
  const teams = useAgentTeams();
  const [search, setSearch] = useState("");
  const [filter, setFilter] = useState<Filter>("all");

  const appliesTo = useMemo(() => {
    const agentNames = new Map((agents.data?.items ?? []).map((agent) => [agent.id, agent.name]));
    const teamNames = new Map((teams.data ?? []).map((team) => [team.id, team.name]));
    return (output: OutputView) => {
      if (output.teamId) return teamNames.get(output.teamId) ?? "A team you cannot see";
      const ids = output.agentIds?.length
        ? output.agentIds
        : output.agentId
          ? [output.agentId]
          : [];
      if (!ids.length) return "Every agent";
      return ids.map((id) => agentNames.get(id) ?? "An agent you cannot see").join(", ");
    };
  }, [agents.data, teams.data]);

  const all = useMemo(() => outputs.data ?? [], [outputs.data]);
  const counts = useMemo(() => {
    const result: Record<Filter, number> = { all: all.length, sending: 0, failing: 0, off: 0, incomplete: 0 };
    for (const output of all) result[outputState(output)] += 1;
    return result;
  }, [all]);
  const visible = useMemo(() => {
    const terms = search.toLowerCase().split(/\s+/).filter(Boolean);
    return all.filter((output) => {
      if (filter !== "all" && outputState(output) !== filter) return false;
      const haystack = `${output.name} ${kindLabel(output.kind)} ${appliesTo(output)} ${output.template ?? ""}`.toLowerCase();
      return terms.every((term) => haystack.includes(term));
    });
  }, [all, filter, search, appliesTo]);

  return (
    <div className="space-y-6">
      <PageHeader
        title="Outputs"
        description="Post to Slack, Mattermost, Telegram or a webhook when a run finishes, so people hear about results without watching ORBIT."
        actions={
          <Button asChild>
            <Link href="/outputs/new">
              <Plus /> New output
            </Link>
          </Button>
        }
      />

      <ListToolbar>
        <SearchField
          value={search}
          onChange={setSearch}
          placeholder="Filter by name, destination or agent"
          label="Filter outputs"
        />
        <Segmented
          label="Output status"
          value={filter}
          onChange={setFilter}
          options={(Object.keys(FILTER_LABEL) as Filter[]).map((value) => ({
            value,
            label: FILTER_LABEL[value],
            count: outputs.data ? counts[value] : undefined,
          }))}
        />
      </ListToolbar>

      {outputs.isLoading ? (
        <ListSkeleton />
      ) : outputs.isError ? (
        <Callout tone="danger" title="Outputs could not be loaded">
          {errorMessage(outputs.error)}
        </Callout>
      ) : visible.length ? (
        <>
          <ListCard columns={["Output", "Status", "Sends", "Updated"]} selectable={false}>
            {visible.map((output) => {
              const problem = outputProblem(output);
              const target = appliesTo(output);
              const when = eventsLabel(output.events);
              const last = lastDeliveryText(output);
              return (
                <ListRow
                  key={output.id}
                  name={
                    <>
                      <p className="flex items-center gap-2">
                        <Link href={`/outputs/${output.id}`} className={ROW_LINK}>
                          {output.name}
                        </Link>
                        <span className="shrink-0 text-xs text-muted-foreground">{kindLabel(output.kind)}</span>
                      </p>
                      <p className="mt-0.5 line-clamp-1 text-[13px] text-muted-foreground">
                        {problem ? <span className="text-warning">{problem}</span> : target}
                      </p>
                      <p className="mt-1.5 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted-foreground md:hidden">
                        {problem ? <span>{target}</span> : null}
                        <span>{when}</span>
                        <span className={deliveryFailed(output.lastDelivery) ? "text-destructive" : undefined}>{last}</span>
                      </p>
                    </>
                  }
                  status={<Status meta={OUTPUT_STATUS[outputState(output)]} />}
                  details={
                    <>
                      <p className="truncate">{when}</p>
                      <p
                        className={
                          deliveryFailed(output.lastDelivery)
                            ? "mt-0.5 truncate text-xs text-destructive"
                            : "mt-0.5 truncate text-xs text-muted-foreground"
                        }
                        title={output.lastDelivery?.error ?? undefined}
                      >
                        {last}
                      </p>
                    </>
                  }
                  updated={relativeTime(output.updatedAt)}
                />
              );
            })}
          </ListCard>
          <ListCount shown={visible.length} total={counts[filter]} singular="output" plural="outputs" />
        </>
      ) : (
        <EmptyState
          icon={<Megaphone />}
          title={
            search
              ? `No outputs match “${search}”`
              : all.length === 0
                ? "No outputs yet"
                : `No outputs that are “${FILTER_LABEL[filter].toLowerCase()}”`
          }
          description={
            search
              ? "Try a shorter term, or clear the filter to see every output."
              : all.length === 0
                ? "Add an output to hear in Slack, Mattermost, Telegram or a webhook when an agent or team run succeeds, fails or is cancelled."
                : "Outputs with another status are one tap away in the filter."
          }
          action={
            search ? (
              <Button variant="outline" onClick={() => setSearch("")}>
                Clear filter
              </Button>
            ) : all.length === 0 ? (
              <Button asChild variant="outline">
                <Link href="/outputs/new">
                  <Plus /> New output
                </Link>
              </Button>
            ) : (
              <Button variant="outline" onClick={() => setFilter("all")}>
                Show all outputs
              </Button>
            )
          }
        />
      )}
    </div>
  );
}
