"use client";

import { useMemo, useState } from "react";
import Link from "next/link";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Bot, Plus } from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { BulkAccessDialog } from "@/components/bulk-access-dialog";
import { Callout } from "@/components/ui/callout";
import { useConfirm } from "@/components/ui/confirm";
import { EmptyState } from "@/components/ui/empty-state";
import { PageHeader } from "@/components/ui/page-header";
import { ImportPortableButton } from "@/components/import-portable-dialog";
import { ImportOpenCodeButton } from "@/components/import-opencode-button";
import { Segmented } from "@/components/ui/segmented";
import {
  ListCard,
  ListCount,
  ListSkeleton,
  ListToolbar,
  SearchField,
  SelectionBar,
  toggleId,
  toggleShown,
} from "@/components/ui/resource-list";
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

      <ListToolbar>
        <SearchField
          value={search}
          onChange={setSearch}
          placeholder="Filter by name, description or harness"
          label="Filter agents"
        />
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
      </ListToolbar>

      <SelectionBar
        count={selected.size}
        noun="agents"
        allShownSelected={visible.length > 0 && visible.every((agent) => selected.has(agent.id))}
        onSelectAllShown={(checked) => setSelected((current) => toggleShown(current, visible.map((agent) => agent.id), checked))}
        onClear={() => setSelected(new Set())}
      >
        <Button size="sm" variant="outline" onClick={() => setBulkOpen(true)}>
          Change access
        </Button>
      </SelectionBar>

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
        <ListSkeleton />
      ) : agents.isError ? (
        <Callout tone="danger" title="Agents could not be loaded">
          {errorMessage(agents.error)}
        </Callout>
      ) : visible.length ? (
        <>
          <ListCard columns={["Agent", "Status", "Harness and autonomy", "Updated"]}>
              {visible.map((agent) => (
                <AgentRow
                  key={agent.id}
                  agent={agent}
                  selected={selected.has(agent.id)}
                  onSelect={(checked) => setSelected((current) => toggleId(current, agent.id, checked))}
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
          </ListCard>
          <ListCount
            shown={visible.length}
            total={counts[filter]}
            singular="agent"
            plural="agents"
            note={truncated ? `Only the first ${all.length} of ${agents.data?.total} are loaded.` : undefined}
          />
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
