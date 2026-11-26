"use client";

export const dynamic = "force-dynamic";

import { useEffect, useMemo, useState } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { History, Plus, Users } from "lucide-react";

import { BulkAccessDialog } from "@/components/bulk-access-dialog";
import { ImportPortableButton } from "@/components/import-portable-dialog";
import { MODE_LABEL, agentLookup, teamMode } from "@/components/teams/team-parts";
import { Button } from "@/components/ui/button";
import { Callout } from "@/components/ui/callout";
import { Checkbox } from "@/components/ui/checkbox";
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
  SelectionBar,
  toggleId,
  toggleShown,
} from "@/components/ui/resource-list";
import { Segmented } from "@/components/ui/segmented";
import { Select } from "@/components/ui/select";
import { ResourceAccess } from "@/components/ui/resource-access";
import { Status, type StatusMeta } from "@/components/ui/status";
import { queryKeys, useAllAgents, useAgentTeams } from "@/lib/queries";
import { listAccessTeams } from "@/lib/access-teams";
import type { AgentTeam } from "@/lib/types";
import { relativeTime } from "@/lib/utils";
import { errorMessage } from "@orbit/api-client";

type Filter = "all" | "source" | "manual";

const FILTER_LABEL: Record<Filter, string> = {
  all: "All",
  source: "Picks up issues",
  manual: "On demand",
};

/** How a team gets its work: from a work source (healthy or failing), or when someone runs it. */
function teamStatus(team: AgentTeam): StatusMeta {
  if (!team.definition.workSource?.enabled) return { label: "On demand", tone: "neutral", shape: "ring" };
  if (team.lastPollError) return { label: "Source failing", tone: "warning", shape: "alert" };
  return { label: "Picks up issues", tone: "success", shape: "solid" };
}

/** The lineup as one line of text, cut off at the end like the other detail columns. */
function LineupText({ team, agentName }: { team: AgentTeam; agentName: (id: string) => string }) {
  const names = (team.definition.steps ?? []).map((step) => agentName(step.agentId));
  if (!names.length) return <span className="block max-w-full truncate text-muted-foreground">No agents yet</span>;
  const [lead, ...rest] = names;
  // A pipeline is a line of agents; a workflow's order is in its graph, so list them.
  const mode = teamMode(team);
  const joiner = mode === "native" ? " with " : mode === "workflow" ? ", " : " → ";
  const tail = rest.length ? `${joiner}${rest.join(mode === "pipeline" ? " → " : ", ")}` : "";
  return (
    <span className="block max-w-full truncate" title={`${lead}${tail}`}>
      <span className="font-medium text-foreground">{lead}</span>
      <span className="text-muted-foreground">{tail}</span>
    </span>
  );
}

function inFilter(team: AgentTeam, filter: Filter) {
  if (filter === "all") return true;
  const fromSource = Boolean(team.definition.workSource?.enabled);
  return filter === "source" ? fromSource : !fromSource;
}

export default function AgentTeamsPage() {
  const router = useRouter();
  const queryClient = useQueryClient();
  const teams = useAgentTeams();
  const agents = useAllAgents();
  const accessTeams = useQuery({ queryKey: ["access-teams"], queryFn: listAccessTeams, staleTime: 60_000 });
  const [filter, setFilter] = useState<Filter>("all");
  const [search, setSearch] = useState("");
  const [teamFilter, setTeamFilter] = useState("all");
  const [visibilityFilter, setVisibilityFilter] = useState("all");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [bulkOpen, setBulkOpen] = useState(false);

  // Old links: `/agent-teams?run=<id>` opens that run, `#runs` the runs list.
  useEffect(() => {
    const runId = new URLSearchParams(window.location.search).get("run");
    if (runId) router.replace(`/agent-teams/runs/${runId}`);
    else if (window.location.hash === "#runs") router.replace("/agent-teams/runs");
  }, [router]);

  const agent = useMemo(() => agentLookup(agents.data?.items ?? []), [agents.data]);
  const agentName = (id: string) => agent(id).name;

  const all = teams.data ?? [];
  const counts = useMemo(() => {
    const result: Record<Filter, number> = { all: 0, source: 0, manual: 0 };
    for (const team of all) {
      for (const key of Object.keys(result) as Filter[]) if (inFilter(team, key)) result[key] += 1;
    }
    return result;
  }, [all]);

  const visible = useMemo(() => {
    const terms = search.toLowerCase().split(/\s+/).filter(Boolean);
    return all.filter((team) => {
      if (!inFilter(team, filter)) return false;
      if (teamFilter === "none" && team.ownerTeamId) return false;
      if (teamFilter !== "all" && teamFilter !== "none" && team.ownerTeamId !== teamFilter) return false;
      if (visibilityFilter !== "all" && team.visibility !== visibilityFilter) return false;
      if (!terms.length) return true;
      const names = (team.definition.steps ?? []).map((step) => agent(step.agentId).name).join(" ");
      const haystack = `${team.name} ${team.description} ${team.definition.repository ?? ""} ${names}`.toLowerCase();
      return terms.every((term) => haystack.includes(term));
    });
  }, [all, filter, search, agent, teamFilter, visibilityFilter]);

  // Bulk actions only ever touch rows that are on screen.
  const selectedIds = useMemo(
    () => visible.filter((team) => selected.has(team.id)).map((team) => team.id),
    [visible, selected],
  );

  return (
    <div className="space-y-6">
      <PageHeader
        title="Agent teams"
        description="A primary agent and its helpers working toward one goal. Open a team to run it, follow its runs and manage its queue."
        actions={
          <div className="flex flex-wrap gap-2">
            <Button asChild variant="outline">
              <Link href="/agent-teams/runs">
                <History /> Runs
              </Link>
            </Button>
            <ImportPortableButton />
            <Button asChild>
              <Link href="/agent-teams/new">
                <Plus /> New agent team
              </Link>
            </Button>
          </div>
        }
      />

      <ListToolbar>
        <SearchField
          value={search}
          onChange={setSearch}
          placeholder="Filter by name, agent or repository"
          label="Filter agent teams"
        />
        <Segmented
          label="How teams get work"
          value={filter}
          onChange={setFilter}
          options={(Object.keys(FILTER_LABEL) as Filter[]).map((value) => ({
            value,
            label: FILTER_LABEL[value],
            count: teams.data ? counts[value] : undefined,
          }))}
        />
        <div className="flex flex-wrap gap-2">
          <Select aria-label="Filter by team" value={teamFilter} onChange={(event) => setTeamFilter(event.target.value)} className="w-auto min-w-40">
            <option value="all">All teams</option>
            <option value="none">No team</option>
            {(accessTeams.data ?? []).map((team) => (
              <option key={team.id} value={team.id}>{team.name}</option>
            ))}
          </Select>
          <Select aria-label="Filter by visibility" value={visibilityFilter} onChange={(event) => setVisibilityFilter(event.target.value)} className="w-auto min-w-40">
            <option value="all">All access</option>
            <option value="private">Private</option>
            <option value="team">Team</option>
            <option value="org">Organization</option>
          </Select>
        </div>
      </ListToolbar>

      <SelectionBar
        count={selectedIds.length}
        noun="agent teams"
        allShownSelected={visible.length > 0 && selectedIds.length === visible.length}
        onSelectAllShown={(checked) => setSelected((current) => toggleShown(current, visible.map((team) => team.id), checked))}
        onClear={() => setSelected(new Set())}
      >
        <Button size="sm" variant="outline" onClick={() => setBulkOpen(true)}>
          Change access
        </Button>
      </SelectionBar>

      <BulkAccessDialog
        open={bulkOpen}
        onOpenChange={setBulkOpen}
        resource="agent_teams"
        ids={selectedIds}
        onDone={() => {
          setSelected(new Set());
          void queryClient.invalidateQueries({ queryKey: queryKeys.agentTeams });
        }}
      />

      {teams.isLoading ? (
        <ListSkeleton />
      ) : teams.isError ? (
        <Callout tone="danger" title="Agent teams could not be loaded">
          {errorMessage(teams.error)}
        </Callout>
      ) : visible.length ? (
        <>
          <ListCard columns={["Team", "Status", "Agents", "Updated"]} accessColumn="Team & access">
            {visible.map((team) => {
              const mode = teamMode(team);
              return (
                <ListRow
                  key={team.id}
                  checkbox={
                    <Checkbox
                      aria-label={`Select ${team.name}`}
                      checked={selected.has(team.id)}
                      onChange={(event) => setSelected((current) => toggleId(current, team.id, event.target.checked))}
                    />
                  }
                  name={
                    <>
                      <p className="flex items-center gap-2">
                        <Link href={`/agent-teams/${team.id}`} className={ROW_LINK}>
                          {team.name}
                        </Link>
                        <span className="tabular shrink-0 text-xs text-muted-foreground">
                          {team.definition.steps?.length ?? 0} agent{team.definition.steps?.length === 1 ? "" : "s"}
                        </span>
                      </p>
                      <p className="mt-0.5 line-clamp-1 text-[13px] text-muted-foreground">
                        {team.description || team.definition.instructions || "No description"}
                      </p>
                      <div className="mt-1.5 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted-foreground [@container(min-width:60rem)]:hidden">
                        <LineupText team={team} agentName={agentName} />
                        <span>{MODE_LABEL[mode]}</span>
                        <span>{relativeTime(team.updatedAt)}</span>
                      </div>
                    </>
                  }
                  status={<Status meta={teamStatus(team)} />}
                  access={<ResourceAccess ownerTeamId={team.ownerTeamId} visibility={team.visibility} />}
                  details={
                    <>
                      <LineupText team={team} agentName={agentName} />
                      <p className="mt-0.5 truncate text-xs text-muted-foreground">
                        {MODE_LABEL[mode]}
                        {team.definition.repository ? <span className="font-mono"> · {team.definition.repository}</span> : null}
                      </p>
                    </>
                  }
                  updated={relativeTime(team.updatedAt)}
                />
              );
            })}
          </ListCard>
          <ListCount shown={visible.length} total={counts[filter]} singular="agent team" plural="agent teams" />
        </>
      ) : (
        <EmptyState
          icon={<Users />}
          title={
            search
              ? `No agent teams match “${search}”`
              : all.length === 0
                ? "No agent teams yet"
                : `No teams that ${filter === "source" ? "pick up issues" : "run on demand"}`
          }
          description={
            search
              ? "Try a shorter term, or clear the filter to see every team."
              : all.length === 0
                ? "A team pairs a primary agent with helpers, such as a builder, a tester and a reviewer, that take over in turn or work as its subagents."
                : "Teams that get work the other way are one tap away in the filter."
          }
          action={
            search ? (
              <Button variant="outline" onClick={() => setSearch("")}>
                Clear filter
              </Button>
            ) : all.length === 0 ? (
              <Button asChild variant="outline">
                <Link href="/agent-teams/new">
                  <Plus /> New agent team
                </Link>
              </Button>
            ) : (
              <Button variant="outline" onClick={() => setFilter("all")}>
                Show all agent teams
              </Button>
            )
          }
        />
      )}
    </div>
  );
}
