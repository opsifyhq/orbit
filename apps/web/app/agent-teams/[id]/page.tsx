"use client";

import { LoadFailure } from "@/components/load-failure";
import { useEffect, useMemo, useState } from "react";
import Link from "next/link";
import { useParams, useRouter } from "next/navigation";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Download, Ellipsis, Pencil, Play, Trash2 } from "lucide-react";
import { toast } from "sonner";

import { exportResource } from "@/components/export-resource-button";
import { LinkedOutputs } from "@/components/outputs/linked-outputs";
import { RunTeamDialog } from "@/components/teams/run-team-dialog";
import { WorkflowView } from "@/components/teams/workflow/workflow-view";
import { AgentSteps, MODE_HINT, MODE_LABEL, agentLookup, modelLabel, teamMode } from "@/components/teams/team-parts";
import { TeamQueue } from "@/components/teams/team-queue";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Callout } from "@/components/ui/callout";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { useConfirm } from "@/components/ui/confirm";
import { DescriptionList } from "@/components/ui/description-list";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { EmptyState } from "@/components/ui/empty-state";
import { PageHeader } from "@/components/ui/page-header";
import { Skeleton } from "@/components/ui/skeleton";
import { ExecutionStatusBadge } from "@/components/ui/status-badge";
import { TagList } from "@/components/ui/tag-list";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { brandName, cleanDescription } from "@/lib/presentation";
import {
  queryKeys,
  useAgentTeam,
  useAllAgents,
  useConnections,
  useEnvMappings,
  useOutputs,
  useSkills,
  useTeamQueue,
  useTeamRuns,
  useWorkerProfiles,
} from "@/lib/queries";
import { deleteAgentTeam } from "@/lib/teams";
import { relativeTime } from "@/lib/utils";
import { errorMessage } from "@orbit/api-client";

const TEAM_TABS = ["overview", "runs", "queue"] as const;
type TeamTab = (typeof TEAM_TABS)[number];

export default function AgentTeamPage() {
  const params = useParams<{ id: string }>();
  const teamId = params.id;
  const router = useRouter();
  const queryClient = useQueryClient();
  const confirm = useConfirm();
  const team = useAgentTeam(teamId);
  const agents = useAllAgents();
  const skills = useSkills();
  const connections = useConnections();
  const envMappings = useEnvMappings();
  const workerProfiles = useWorkerProfiles();
  const outputs = useOutputs();
  const runs = useTeamRuns({ teamId });
  const queue = useTeamQueue(teamId);

  // The open tab lives in the URL hash so a link or reload returns to it.
  const [tab, setTab] = useState<TeamTab>("overview");
  useEffect(() => {
    const fromHash = window.location.hash.slice(1) as TeamTab;
    if (TEAM_TABS.includes(fromHash)) setTab(fromHash);
  }, []);
  function selectTab(next: string) {
    setTab(next as TeamTab);
    window.history.replaceState(null, "", next === "overview" ? window.location.pathname : `#${next}`);
  }

  const remove = useMutation({
    mutationFn: () => deleteAgentTeam(teamId),
    onSuccess: () => {
      toast.success("Agent team deleted");
      void queryClient.invalidateQueries({ queryKey: queryKeys.agentTeams });
      router.push("/agent-teams");
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  const agent = useMemo(() => agentLookup(agents.data?.items ?? []), [agents.data]);
  const workflowAgentName = useMemo(() => (id: string) => agent(id).name, [agent]);
  const teamRuns = useMemo(
    // The server already filters by team; the check also covers data cached before it did.
    () => (runs.data ?? []).filter((run) => run.teamId === teamId),
    [runs.data, teamId],
  );
  const pendingCount = (queue.data ?? []).filter((item) => item.status === "pending").length;

  if (team.isLoading) {
    return (
      <div className="space-y-4">
        <Skeleton className="h-10 w-72" />
        <Skeleton className="h-64 rounded-xl" />
      </div>
    );
  }
  if (!team.data) {
    return (
      <LoadFailure
        noun="agent team"
        error={team.error}
        back={{ href: "/agent-teams", label: "Back to agent teams" }}
        onRetry={() => void team.refetch()}
        retrying={team.isFetching}
      />
    );
  }

  const data = team.data;
  const definition = data.definition;
  const mode = teamMode(data);
  const source = definition.workSource;
  const profile = (workerProfiles.data ?? []).find((item) => item.id === definition.workerProfileId);
  const teamModel = modelLabel(definition.model);
  const connectionNames = (definition.connectionIds ?? []).map(
    (id) => connections.data?.find((connection) => connection.id === id)?.name ?? id.slice(0, 8),
  );
  const mappingNames = (definition.envMappingIds ?? []).map(
    (id) => envMappings.data?.find((mapping) => mapping.id === id)?.name ?? id.slice(0, 8),
  );
  const lastRun = teamRuns[0];

  async function deleteTeam() {
    const confirmed = await confirm({
      title: `Delete ${data.name}?`,
      description: "The team and its queue are removed. Past runs and their executions are kept. This cannot be undone.",
      confirmLabel: "Delete agent team",
      destructive: true,
    });
    if (confirmed) remove.mutate();
  }

  return (
    <div className="space-y-6">
      <PageHeader
        back={{ href: "/agent-teams", label: "Agent teams" }}
        title={data.name}
        description={data.description || undefined}
        meta={
          <>
            <Badge variant="outline">{MODE_LABEL[mode]}</Badge>
            <span className="text-[13px] text-muted-foreground">Updated {relativeTime(data.updatedAt)}</span>
          </>
        }
        actions={
          <>
            <RunTeamDialog team={data} />
            <Button asChild variant="outline">
              <Link href={`/agent-teams/${data.id}/edit`}>
                <Pencil /> Edit
              </Link>
            </Button>
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button variant="outline" size="icon" aria-label="More actions">
                  <Ellipsis />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                <DropdownMenuItem onSelect={() => void exportResource("agent_teams", data.id, data.name)}>
                  <Download /> Export as YAML
                </DropdownMenuItem>
                <DropdownMenuSeparator />
                <DropdownMenuItem destructive onSelect={() => void deleteTeam()}>
                  <Trash2 /> Delete agent team
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </>
        }
      />

      {source?.enabled && data.lastPollError ? (
        <Callout tone="warning" title="The work source could not check for new issues">
          {data.lastPollError}
          {data.lastPollAt ? ` Last attempt ${relativeTime(data.lastPollAt)}.` : ""}
        </Callout>
      ) : null}

      <Tabs value={tab} onValueChange={selectTab}>
        <TabsList>
          <TabsTrigger value="overview">Overview</TabsTrigger>
          <TabsTrigger value="runs">
            Runs
            {teamRuns.length ? <span className="tabular text-xs text-muted-foreground">{teamRuns.length}</span> : null}
          </TabsTrigger>
          <TabsTrigger value="queue">
            Queue
            {pendingCount ? <span className="tabular text-xs text-muted-foreground">{pendingCount}</span> : null}
          </TabsTrigger>
        </TabsList>

        <TabsContent value="overview">
          {/* A workflow is a picture that needs the width; the facts follow below it. */}
          {mode === "workflow" && definition.workflow ? (
            <Card className="mb-5">
              <CardHeader>
                <CardTitle as="h2">Workflow</CardTitle>
                <p className="text-[13px] text-muted-foreground">{MODE_HINT[mode]}</p>
              </CardHeader>
              <CardContent>
                <WorkflowView workflow={definition.workflow} agentName={workflowAgentName} />
              </CardContent>
            </Card>
          ) : null}
          <div className="grid gap-5 lg:grid-cols-[minmax(0,1fr)_21rem] [&>*]:min-w-0">
            <div className="space-y-5">
              {mode === "workflow" && definition.workflow ? null : (
                <Card>
                  <CardHeader>
                    <CardTitle as="h2">Agents</CardTitle>
                    <p className="text-[13px] text-muted-foreground">{MODE_HINT[mode]}</p>
                  </CardHeader>
                  <CardContent>
                    <AgentSteps team={data} agent={agent} />
                  </CardContent>
                </Card>
              )}

              <Card>
                <CardHeader>
                  <CardTitle as="h2">Goal</CardTitle>
                </CardHeader>
                <CardContent>
                  {definition.instructions ? (
                    <p className="max-h-80 overflow-auto whitespace-pre-wrap text-sm leading-relaxed">
                      {definition.instructions}
                    </p>
                  ) : (
                    <p className="text-sm text-muted-foreground">No goal yet. Each run then needs its own task.</p>
                  )}
                </CardContent>
              </Card>

              {lastRun ? (
                <Card>
                  <CardHeader className="flex-row flex-wrap items-center justify-between gap-2 space-y-0">
                    <CardTitle as="h2">Latest run</CardTitle>
                    <Button size="sm" variant="ghost" onClick={() => selectTab("runs")}>
                      All runs
                    </Button>
                  </CardHeader>
                  <CardContent>
                    <Link
                      href={`/agent-teams/runs/${lastRun.id}`}
                      className="flex flex-wrap items-center gap-x-3 gap-y-1 rounded-md hover:text-primary"
                    >
                      <ExecutionStatusBadge status={lastRun.status} />
                      <span className="min-w-0 flex-1 truncate text-sm font-medium">
                        {String(lastRun.input.task ?? "") || "Agent team run"}
                      </span>
                      <span className="text-[13px] text-muted-foreground">{relativeTime(lastRun.createdAt)}</span>
                    </Link>
                  </CardContent>
                </Card>
              ) : null}
            </div>

            <div className="space-y-5">
              <Card>
                <CardHeader>
                  <CardTitle as="h2">Repository and work</CardTitle>
                </CardHeader>
                <CardContent>
                  <DescriptionList
                    items={[
                      { label: "Repository", value: definition.repository || "Chosen per run", mono: Boolean(definition.repository) },
                      definition.branch && { label: "Base branch", value: definition.branch, mono: true },
                      definition.repositories?.length
                        ? {
                            label: "Workspace",
                            value: definition.repositories.map((item) => item.repository).join(", "),
                            mono: true,
                          }
                        : null,
                      {
                        label: "Work source",
                        value: source?.enabled
                          ? `${source.provider === "gitlab" ? "GitLab" : "GitHub"}${
                              source.repository ?? source.project ?? source.groupId
                                ? `: ${source.repository ?? source.project ?? source.groupId}`
                                : ""
                            }${source.autoRun ? ", runs automatically" : ""}`
                          : "Off",
                      },
                      source?.enabled && source.labels?.length
                        ? { label: "Picks up labels", value: source.labels.join(", ") }
                        : null,
                    ]}
                  />
                </CardContent>
              </Card>

              <Card>
                <CardHeader>
                  <CardTitle as="h2">Worker and limits</CardTitle>
                </CardHeader>
                <CardContent>
                  <DescriptionList
                    items={[
                      { label: "Worker profile", value: profile?.name ?? (definition.workerProfileId ? "Unavailable" : "Any matching worker") },
                      profile && { label: "Starts worker", value: definition.autoLaunch ? "Automatically" : "No" },
                      { label: "Harness", value: definition.harness ? brandName(definition.harness) : "Each agent's own" },
                      { label: "Team model", value: teamModel ?? "Each agent's own", mono: Boolean(teamModel) },
                      { label: "Timeout", value: definition.timeoutSeconds ? `${definition.timeoutSeconds} s per step` : "Each agent's limit" },
                    ]}
                  />
                </CardContent>
              </Card>

              <Card>
                <CardHeader>
                  <CardTitle as="h2">Shared with every agent</CardTitle>
                </CardHeader>
                <CardContent className="space-y-4">
                  <div className="space-y-1.5">
                    <p className="text-[13px] text-muted-foreground">Skills</p>
                    <TagList
                      max={8}
                      empty="None"
                      items={(definition.skills ?? []).map((slug) => {
                        const skill = skills.data?.find((item) => item.slug === slug);
                        return { key: slug, label: skill?.name ?? slug, title: cleanDescription(skill?.description) || slug };
                      })}
                    />
                  </div>
                  <div className="space-y-1.5">
                    <p className="text-[13px] text-muted-foreground">Connections</p>
                    <TagList max={6} empty="None" items={connectionNames.map((name, i) => ({ key: `${name}-${i}`, label: name }))} />
                  </div>
                  <div className="space-y-1.5">
                    <p className="text-[13px] text-muted-foreground">Environment mappings</p>
                    <TagList max={6} empty="None" items={mappingNames.map((name, i) => ({ key: `${name}-${i}`, label: name }))} />
                  </div>
                </CardContent>
              </Card>

              <LinkedOutputs outputs={outputs.data ?? []} scope="team" id={teamId} />
            </div>
          </div>
        </TabsContent>

        <TabsContent value="runs">
          {runs.isLoading ? (
            <Skeleton className="h-40 rounded-xl" />
          ) : runs.isError ? (
            <Callout tone="danger" title="Runs could not be loaded">
              {errorMessage(runs.error)}
            </Callout>
          ) : !teamRuns.length ? (
            <EmptyState
              icon={<Play />}
              title="This team has not run yet"
              description="Start a run with the Run button, or add work to the queue."
            />
          ) : (
            <Card className="overflow-hidden">
              <ul>
                {teamRuns.map((run) => (
                  <li key={run.id} className="group relative border-b last:border-b-0">
                    <div className="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-4 gap-y-1 px-4 py-3.5 transition-colors group-hover:bg-muted/50 sm:px-5 md:grid-cols-[8rem_minmax(0,1fr)_6.5rem]">
                      <ExecutionStatusBadge
                        status={run.status}
                        className="col-start-2 row-start-1 justify-self-end md:col-start-auto md:justify-self-start"
                      />
                      <div className="col-start-1 row-start-1 min-w-0 md:col-start-auto">
                        <Link
                          href={`/agent-teams/runs/${run.id}`}
                          className="block truncate text-sm font-medium after:absolute after:inset-0"
                        >
                          {String(run.input.task ?? "") || "Agent team run"}
                        </Link>
                        {run.error ? <p className="line-clamp-1 text-[13px] text-destructive">{run.error}</p> : null}
                      </div>
                      <p className="tabular text-[13px] text-muted-foreground md:text-right">{relativeTime(run.createdAt)}</p>
                    </div>
                  </li>
                ))}
              </ul>
            </Card>
          )}
        </TabsContent>

        <TabsContent value="queue">
          <TeamQueue team={data} />
        </TabsContent>
      </Tabs>
    </div>
  );
}
