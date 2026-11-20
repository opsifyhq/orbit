"use client";

import { useMemo, useState } from "react";
import Link from "next/link";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Bot, Plus, Search, X } from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { BulkActionsBar } from "@/components/bulk-actions-bar";
import { BulkAccessDialog } from "@/components/bulk-access-dialog";
import { Callout } from "@/components/ui/callout";
import { Card } from "@/components/ui/card";
import { useConfirm } from "@/components/ui/confirm";
import { EmptyState } from "@/components/ui/empty-state";
import { Input } from "@/components/ui/input";
import { PageHeader } from "@/components/ui/page-header";
import { ImportPortableButton } from "@/components/import-portable-dialog";
import { ImportOpenCodeButton } from "@/components/import-opencode-button";
import { Segmented } from "@/components/ui/segmented";
import { Skeleton } from "@/components/ui/skeleton";
import { AgentRow } from "@/components/agents/agent-row";
import { api } from "@/lib/api";
import { queryKeys, unwrap, useAllAgents } from "@/lib/queries";
import { errorMessage } from "@orbit/api-client";
import type { AgentSummary } from "@/lib/types";

type Filter = "current" | "active" | "draft" | "paused" | "archived";

const FILTER_LABEL: Record<Filter, string> = {
  current: "All",
  active: "Active",
  draft: "Draft",
  paused: "Paused",
  archived: "Archived",
};

function inFilter(agent: AgentSummary, filter: Filter) {
  if (filter === "current") return agent.status !== "archived";
  return agent.status === filter;
}

export default function AgentsPage() {
  // "All" means every agent still in use. Drafts are included on purpose: a
  // freshly created agent is a draft and must be visible where its author looks.
  const [filter, setFilter] = useState<Filter>("current");
  const [search, setSearch] = useState("");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [bulkOpen, setBulkOpen] = useState(false);
  const agents = useAllAgents();
  const queryClient = useQueryClient();
  const confirm = useConfirm();

  const invalidateAgents = () => queryClient.invalidateQueries({ queryKey: queryKeys.agents() });

  const hardDelete = useMutation({
    mutationFn: async (agentId: string) =>
      unwrap(
        await api.DELETE("/api/v1/agents/{agent_id}", {
          params: { path: { agent_id: agentId }, query: { hard: true } },
        }),
      ),
    onSuccess: () => {
      toast.success("Agent deleted");
      void invalidateAgents();
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  const all = agents.data?.items ?? [];
  const counts = useMemo(() => {
    const result: Record<Filter, number> = { current: 0, active: 0, draft: 0, paused: 0, archived: 0 };
    for (const agent of all) {
      for (const key of Object.keys(result) as Filter[]) {
        if (inFilter(agent, key)) result[key] += 1;
      }
    }
    return result;
  }, [all]);

  const visible = useMemo(() => {
    const terms = search.toLowerCase().split(/\s+/).filter(Boolean);
    return all.filter((agent) => {
      if (!inFilter(agent, filter)) return false;
      if (!terms.length) return true;
      const haystack = `${agent.name} ${agent.description ?? ""} ${agent.harness ?? ""}`.toLowerCase();
      return terms.every((term) => haystack.includes(term));
    });
  }, [all, filter, search]);

  const truncated = agents.data ? agents.data.total > all.length : false;

  return (
    <div className="space-y-6">
      <PageHeader
        title="Agents"
        description="Each agent is a versioned definition: instructions, harness, model, connectors, triggers and how much it may do on its own."
        actions={
          <div className="flex flex-wrap gap-2">
            <ImportPortableButton />
            <ImportOpenCodeButton kind="agent" />
            <Button asChild>
              <Link href="/agents/new">
                <Plus /> New agent
              </Link>
            </Button>
          </div>
        }
      />

      <div className="flex flex-col gap-3 md:flex-row md:items-center">
        <div className="relative md:w-80">
          <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
          <Input
            type="search"
            className="pl-9 pr-9 [&::-webkit-search-cancel-button]:hidden"
            placeholder="Filter by name, description or harness"
            aria-label="Filter agents"
            value={search}
            onChange={(event) => setSearch(event.target.value)}
          />
          {search ? (
            <button
              type="button"
              aria-label="Clear filter"
              onClick={() => setSearch("")}
              className="absolute right-1.5 top-1/2 flex h-6 w-6 -translate-y-1/2 items-center justify-center rounded text-muted-foreground hover:bg-muted hover:text-foreground"
            >
              <X className="h-3.5 w-3.5" />
            </button>
          ) : null}
        </div>
        <Segmented
          label="Agent status"
          value={filter}
          onChange={setFilter}
          options={(Object.keys(FILTER_LABEL) as Filter[]).map((value) => ({
            value,
            label: FILTER_LABEL[value],
            count: agents.data ? counts[value] : undefined,
          }))}
        />
      </div>

      {selected.size ? (
        <BulkActionsBar
          resource="agents"
          selectedIds={[...selected]}
          allShownSelected={visible.length > 0 && visible.every((agent) => selected.has(agent.id))}
          onToggleAllShown={(checked) =>
            setSelected(
              checked
                ? new Set([...selected, ...visible.map((agent) => agent.id)])
                : new Set([...selected].filter((id) => !visible.some((agent) => agent.id === id))),
            )
          }
          onChangeAccess={() => setBulkOpen(true)}
          archivedIds={all
            .filter((agent) => selected.has(agent.id) && agent.status === "archived")
            .map((agent) => agent.id)}
          onSucceeded={(ids) => {
            setSelected((current) => {
              const next = new Set(current);
              ids.forEach((id) => next.delete(id));
              return next;
            });
            void invalidateAgents();
          }}
          onClear={() => setSelected(new Set())}
        />
      ) : null}

      <BulkAccessDialog
        open={bulkOpen}
        onOpenChange={setBulkOpen}
        resource="agents"
        ids={[...selected]}
        onDone={() => {
          setSelected(new Set());
          void invalidateAgents();
        }}
      />

      {agents.isLoading ? (
        <Card className="overflow-hidden">
          {Array.from({ length: 4 }).map((_, index) => (
            <div key={index} className="border-b px-5 py-4 last:border-b-0">
              <Skeleton className="h-10" />
            </div>
          ))}
        </Card>
      ) : agents.isError ? (
        <Callout tone="danger" title="Agents could not be loaded">
          {errorMessage(agents.error)}
        </Callout>
      ) : visible.length ? (
        <>
          <Card className="overflow-hidden">
            <div
              aria-hidden
              className="hidden grid-cols-[minmax(0,1fr)_7rem_11rem_6.5rem] gap-x-4 border-b bg-subtle px-5 py-2 text-xs font-medium text-muted-foreground md:grid"
            >
              <span>Agent</span>
              <span>Status</span>
              <span>Harness and autonomy</span>
              <span className="text-right">Updated</span>
            </div>
            <ul>
              {visible.map((agent) => (
                <AgentRow
                  key={agent.id}
                  agent={agent}
                  selected={selected.has(agent.id)}
                  onSelect={(checked) =>
                    setSelected((current) => {
                      const next = new Set(current);
                      if (checked) next.add(agent.id);
                      else next.delete(agent.id);
                      return next;
                    })
                  }
                  onDelete={async (target) => {
                    const confirmed = await confirm({
                      title: `Delete ${target.name} permanently?`,
                      description:
                        "This removes the agent and its whole execution history. It cannot be undone.",
                      confirmLabel: "Delete agent",
                      destructive: true,
                    });
                    if (confirmed) hardDelete.mutate(target.id);
                  }}
                />
              ))}
            </ul>
          </Card>
          <p className="tabular text-[13px] text-muted-foreground">
            {visible.length === counts[filter]
              ? `${visible.length} agent${visible.length === 1 ? "" : "s"}.`
              : `${visible.length} of ${counts[filter]} agents match.`}
            {truncated ? ` Only the first ${all.length} of ${agents.data?.total} are loaded.` : ""}
          </p>
        </>
      ) : (
        <EmptyState
          icon={<Bot />}
          title={
            search
              ? `No agents match “${search}”`
              : all.length === 0
                ? "No agents yet"
                : `No ${FILTER_LABEL[filter].toLowerCase()} agents`
          }
          description={
            search
              ? "Try a shorter term, or clear the filter to see every agent."
              : all.length === 0
                ? "An agent needs instructions and a harness to start. It is saved as a draft, so you can test it before activating."
                : "Agents in other states are one tap away in the status filter."
          }
          action={
            search ? (
              <Button variant="outline" onClick={() => setSearch("")}>
                Clear filter
              </Button>
            ) : all.length === 0 ? (
              <Button asChild variant="outline">
                <Link href="/agents/new">
                  <Plus /> New agent
                </Link>
              </Button>
            ) : (
              <Button variant="outline" onClick={() => setFilter("current")}>
                Show all agents
              </Button>
            )
          }
        />
      )}
    </div>
  );
}
