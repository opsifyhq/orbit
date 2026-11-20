"use client";

export const dynamic = "force-dynamic";

import { LoadFailure } from "@/components/load-failure";
import Link from "next/link";
import { useParams, useRouter } from "next/navigation";
import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Archive,
  Copy,
  Download,
  Ellipsis,
  Pause,
  Pencil,
  Play,
  RotateCcw,
  Trash2,
  Zap,
} from "lucide-react";
import { toast } from "sonner";

import { RunAgentDialog } from "@/components/agents/run-agent-dialog";
import { ExecutionListHeader, ExecutionRow } from "@/components/executions/execution-row";
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
import { AgentStatusBadge } from "@/components/ui/status-badge";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { api } from "@/lib/api";
import { errorStatus, request, requestRaw } from "@/lib/http";
import { copyText } from "@/lib/browser";
import { agentSpecSchema } from "@orbit/contracts";
import { queryKeys, unwrap, useAgent, useAgentVersions, useExecutions, useSkills } from "@/lib/queries";
import { errorMessage } from "@orbit/api-client";
import { formatDateTime, relativeTime, titleCase } from "@/lib/utils";
import { autonomyName, brandName, capabilityLabel, cronDescription } from "@/lib/presentation";

const AGENT_TABS = ["overview", "executions", "versions", "configuration"] as const;
type AgentTab = (typeof AGENT_TABS)[number];

export default function AgentDetailPage() {
  const params = useParams<{ id: string }>();
  const agentId = params.id;
  const router = useRouter();
  const queryClient = useQueryClient();
  const confirm = useConfirm();
  const agent = useAgent(agentId);
  const skillList = useSkills();
  const versions = useAgentVersions(agentId);
  const executions = useExecutions({ agentId, limit: 20 });

  // The open tab lives in the URL hash so a link or reload returns to it.
  const [tab, setTab] = useState<AgentTab>("overview");
  useEffect(() => {
    const fromHash = window.location.hash.slice(1) as AgentTab;
    if (AGENT_TABS.includes(fromHash)) setTab(fromHash);
  }, []);
  function selectTab(next: string) {
    setTab(next as AgentTab);
    window.history.replaceState(null, "", next === "overview" ? window.location.pathname : `#${next}`);
  }

  const runtime = useQuery({
    queryKey: ["agent-runtime", agentId],
    queryFn: () =>
      request<{
        id: string;
        status: string;
        lastHeartbeatAt: string;
        lastExecutionId?: string | null;
        lastError?: string | null;
        consecutiveFailures?: number;
      } | null>(`/api/v1/agents/${agentId}/runtime`).catch((error) => {
        // No runtime is the normal case (404); anything else must not look like it.
        if (errorStatus(error) === 404) return null;
        throw error;
      }),
    // Only a running 24/7 agent changes on its own; an idle one is refreshed by Start.
    refetchInterval: (query) => (query.state.data ? 10_000 : false),
    retry: false,
  });
  const startRuntime = useMutation({
    mutationFn: () =>
      request(`/api/v1/agents/${agentId}/runtime`, {
        method: "POST",
        body: JSON.stringify({}),
      }),
    onSuccess: () => {
      toast.success("24/7 runtime started");
      void queryClient.invalidateQueries({ queryKey: ["agent-runtime", agentId] });
    },
    onError: (error) => toast.error(errorMessage(error)),
  });
  const stopRuntime = useMutation({
    mutationFn: () =>
      request(`/api/v1/agents/${agentId}/runtime/stop`, { method: "POST" }),
    onSuccess: () => {
      toast.success("24/7 runtime stopping");
      void queryClient.invalidateQueries({ queryKey: ["agent-runtime", agentId] });
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  const lifecycle = useMutation({
    // Restoring an archived agent is an activation; it keeps its own name in the UI.
    mutationFn: async (action: "activate" | "pause" | "restore") =>
      unwrap(
        await api.POST(
          action === "pause"
            ? "/api/v1/agents/{agent_id}/pause"
            : "/api/v1/agents/{agent_id}/activate",
          { params: { path: { agent_id: agentId } } },
        ),
      ),
    onSuccess: (_, action) => {
      toast.success(
        action === "pause" ? "Agent paused" : action === "restore" ? "Agent restored" : "Agent activated",
      );
      void queryClient.invalidateQueries({ queryKey: queryKeys.agent(agentId) });
      void queryClient.invalidateQueries({ queryKey: queryKeys.agents() });
      void queryClient.invalidateQueries({ queryKey: queryKeys.dashboard });
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  const remove = useMutation({
    mutationFn: async (hard: boolean) =>
      unwrap(
        await api.DELETE("/api/v1/agents/{agent_id}", {
          params: { path: { agent_id: agentId }, query: { hard } },
        }),
      ),
    onSuccess: (_, hard) => {
      toast.success(hard ? "Agent deleted" : "Agent archived");
      void queryClient.invalidateQueries({ queryKey: queryKeys.agents() });
      router.push("/agents");
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  if (agent.isLoading) {
    return (
      <div className="space-y-6">
        <Skeleton className="h-5 w-20" />
        <Skeleton className="h-10 w-72" />
        <Skeleton className="h-9 w-full max-w-md" />
        <Skeleton className="h-72 rounded-xl" />
      </div>
    );
  }

  if (!agent.data) {
    return (
      <LoadFailure
        noun="agent"
        error={agent.error}
        back={{ href: "/agents", label: "Back to agents" }}
        onRetry={() => void agent.refetch()}
        retrying={agent.isFetching}
      />
    );
  }

  const data = agent.data;
  const parsedSpec = agentSpecSchema.safeParse(data.spec);
  if (!parsedSpec.success) {
    return (
      <div className="space-y-6">
        <PageHeader title={data.name} back={{ href: "/agents", label: "Agents" }} />
        <EmptyState
          title="This agent's configuration cannot be shown"
          description={parsedSpec.error.issues
            .map((issue) => `${issue.path.join(".")}: ${issue.message}`)
            .join("; ")}
          action={
            <Button asChild>
              <Link href={`/agents/${data.id}/edit`}>Open in Agent Studio</Link>
            </Button>
          }
        />
      </div>
    );
  }

  const spec = parsedSpec.data;
  const autonomy = spec.autonomy;
  const archived = data.status === "archived";
  const defaultTask = (data.spec as unknown as { runDefaults?: { task?: string } }).runDefaults?.task;
  const yaml = data.definitionYaml || "";

  async function downloadYaml() {
    // Fetched (not linked) so the request carries the session token and goes
    // through the same origin as every other API call.
    try {
      // Same-origin proxy: `NEXT_PUBLIC_ORBIT_API_URL` may be browser-unreachable (#94).
      const response = await requestRaw(`/api/v1/agents/${data.id}/export`, { sameOrigin: true });
      const url = URL.createObjectURL(await response.blob());
      const link = document.createElement("a");
      link.href = url;
      link.download = `${data.slug || "agent"}.yaml`;
      link.click();
      URL.revokeObjectURL(url);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : "Export failed");
    }
  }

  async function archiveAgent() {
    const confirmed = await confirm({
      title: `Archive ${data.name}?`,
      description:
        "Triggers stop, it can no longer run or be edited, and it moves to the Archived filter. Its versions and execution history are kept, and you can restore it later.",
      confirmLabel: "Archive agent",
      destructive: true,
    });
    if (confirmed) remove.mutate(false);
  }

  async function deleteAgent() {
    const confirmed = await confirm({
      title: `Delete ${data.name} permanently?`,
      description: "This removes the agent and its whole execution history. It cannot be undone.",
      confirmLabel: "Delete agent",
      destructive: true,
    });
    if (confirmed) remove.mutate(true);
  }

  return (
    <div className="space-y-6">
      <PageHeader
        back={{ href: "/agents", label: "Agents" }}
        title={data.name}
        description={data.description || undefined}
        meta={
          <>
            <AgentStatusBadge status={data.status} />
            <span className="text-[13px] text-muted-foreground">
              Version {data.currentVersion}, updated {relativeTime(data.updatedAt)}
            </span>
          </>
        }
        actions={
          <>
            {!archived ? (
              <RunAgentDialog
                agentId={data.id}
                agentName={data.name}
                defaultTask={defaultTask}
                agentHarness={spec.harness?.type}
              />
            ) : null}
            {data.status !== "active" || runtime.isError ? null : (
              runtime.data ? (
                <Button
                  variant="outline"
                  loading={stopRuntime.isPending}
                  onClick={() => stopRuntime.mutate()}
                >
                  Stop 24/7
                </Button>
              ) : (
                <Button
                  variant="outline"
                  loading={startRuntime.isPending}
                  onClick={() => startRuntime.mutate()}
                >
                  Start 24/7
                </Button>
              )
            )}
            {!archived ? (
              <Button asChild variant="outline">
                <Link href={`/agents/${data.id}/edit`}>
                  <Pencil /> Edit
                </Link>
              </Button>
            ) : null}
            {data.status === "active" ? (
              <Button variant="outline" onClick={() => lifecycle.mutate("pause")} loading={lifecycle.isPending}>
                <Pause /> Pause
              </Button>
            ) : data.status === "paused" ? (
              <Button variant="outline" onClick={() => lifecycle.mutate("activate")} loading={lifecycle.isPending}>
                <Zap /> Resume
              </Button>
            ) : archived ? (
              <Button onClick={() => lifecycle.mutate("restore")} loading={lifecycle.isPending}>
                <RotateCcw /> Restore agent
              </Button>
            ) : null}
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button variant="outline" size="icon" aria-label="More actions">
                  <Ellipsis />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                <DropdownMenuItem onSelect={() => void downloadYaml()}>
                  <Download /> Export as YAML
                </DropdownMenuItem>
                <DropdownMenuSeparator />
                {archived ? (
                  <DropdownMenuItem destructive onSelect={() => void deleteAgent()}>
                    <Trash2 /> Delete permanently
                  </DropdownMenuItem>
                ) : (
                  <DropdownMenuItem destructive onSelect={() => void archiveAgent()}>
                    <Archive /> Archive agent
                  </DropdownMenuItem>
                )}
              </DropdownMenuContent>
            </DropdownMenu>
          </>
        }
      />

      {data.status === "draft" ? (
        <Callout
          tone="info"
          title="This agent is a draft"
          action={
            <Button size="sm" onClick={() => lifecycle.mutate("activate")} loading={lifecycle.isPending}>
              <Zap /> Activate
            </Button>
          }
        >
          You can run it by hand to test it. Its triggers only fire once it is active.
        </Callout>
      ) : null}

      {archived ? (
        <Callout tone="neutral" title="This agent is archived">
          It cannot run or be edited until you restore it. Its execution history is still available, and its
          definition can be exported and used to create a new agent.
        </Callout>
      ) : null}

      {runtime.isError ? (
        <Callout tone="warning" title="Could not check whether this agent is running 24/7">
          {errorMessage(runtime.error)}
        </Callout>
      ) : null}
      {runtime.data ? (
        <Callout
          tone={runtime.data.status === "running" ? "info" : runtime.data.status === "stopping" ? "warning" : "neutral"}
          title={`24/7 runtime · ${titleCase(runtime.data.status)}`}
          action={
            runtime.data.lastExecutionId ? (
              <Button asChild size="sm" variant="outline">
                <Link href={`/executions/${runtime.data.lastExecutionId}`}>Open last run</Link>
              </Button>
            ) : undefined
          }
        >
          Heartbeat {relativeTime(runtime.data.lastHeartbeatAt)}
          {runtime.data.lastError ? ` · ${runtime.data.lastError}` : ""}
          {runtime.data.consecutiveFailures
            ? ` · ${runtime.data.consecutiveFailures} consecutive failed ${runtime.data.consecutiveFailures === 1 ? "run" : "runs"}, retrying with backoff`
            : ""}. This runtime creates
          the next execution when the previous one finishes and resumes its session.
        </Callout>
      ) : null}

      <Tabs value={tab} onValueChange={selectTab}>
        <TabsList>
          <TabsTrigger value="overview">Overview</TabsTrigger>
          <TabsTrigger value="executions">
            Executions
            {executions.data?.total ? (
              <span className="tabular text-xs text-muted-foreground">{executions.data.total}</span>
            ) : null}
          </TabsTrigger>
          <TabsTrigger value="versions">
            Versions
            {versions.data?.length ? (
              <span className="tabular text-xs text-muted-foreground">{versions.data.length}</span>
            ) : null}
          </TabsTrigger>
          <TabsTrigger value="configuration">Definition</TabsTrigger>
        </TabsList>

        <TabsContent value="overview">
          <div className="grid gap-5 lg:grid-cols-[minmax(0,1fr)_21rem]">
            <div className="space-y-5">
              <Card>
                <CardHeader>
                  <CardTitle as="h2">Instructions</CardTitle>
                </CardHeader>
                <CardContent className="space-y-4">
                  {spec.instructions.system ? (
                    <p className="max-h-96 overflow-auto whitespace-pre-wrap text-sm leading-relaxed">
                      {spec.instructions.system}
                    </p>
                  ) : (
                    <p className="text-sm text-muted-foreground">No instructions yet.</p>
                  )}
                  {spec.instructions.constraints.length > 0 ? (
                    <div className="border-t pt-4">
                      <p className="mb-2 text-[13px] font-medium">Constraints</p>
                      <ul className="list-outside list-disc space-y-1 pl-5 text-[13px] text-muted-foreground">
                        {spec.instructions.constraints.map((constraint) => (
                          <li key={constraint}>{constraint}</li>
                        ))}
                      </ul>
                    </div>
                  ) : null}
                </CardContent>
              </Card>

              <Card>
                <CardHeader>
                  <CardTitle as="h2">Triggers</CardTitle>
                </CardHeader>
                <CardContent>
                  {data.triggers.length === 0 ? (
                    <p className="text-sm text-muted-foreground">
                      This agent only runs when someone starts it. Add a schedule or event trigger in{" "}
                      {archived ? (
                        "Agent Studio"
                      ) : (
                        <Link href={`/agents/${data.id}/edit`} className="font-medium text-primary underline-offset-4 hover:underline">
                          Agent Studio
                        </Link>
                      )}
                      .
                    </p>
                  ) : (
                    <ul className="divide-y">
                      {data.triggers.map((trigger) => (
                        <li key={trigger.id} className="flex flex-wrap items-center gap-x-3 gap-y-1 py-2.5 first:pt-0 last:pb-0">
                          <Badge variant="secondary">{titleCase(trigger.kind)}</Badge>
                          <span className="min-w-0 flex-1 text-sm">
                            <span className="block truncate">
                              {trigger.kind === "schedule"
                                ? cronDescription(
                                    String((trigger.config as { cron?: string }).cron ?? ""),
                                    (trigger.config as { timezone?: string | null }).timezone,
                                  )
                                : trigger.kind === "event"
                                  ? String((trigger.config as { subject?: string }).subject ?? "")
                                  : trigger.kind === "webhook"
                                    ? "Inbound webhook"
                                    : "Started by hand"}
                            </span>
                            {(trigger.config as { task?: string }).task ? (
                              <span className="mt-0.5 line-clamp-2 block text-[13px] text-muted-foreground">
                                {(trigger.config as { task?: string }).task}
                              </span>
                            ) : null}
                          </span>
                          <span className="text-[13px] text-muted-foreground">
                            {trigger.enabled
                              ? trigger.nextRunAt
                                ? `Next ${formatDateTime(trigger.nextRunAt)}`
                                : "Enabled"
                              : "Disabled"}
                          </span>
                        </li>
                      ))}
                    </ul>
                  )}
                </CardContent>
              </Card>
            </div>

            <div className="space-y-5">
              <Card>
                <CardHeader>
                  <CardTitle as="h2">Runtime</CardTitle>
                </CardHeader>
                <CardContent>
                  <DescriptionList
                    items={[
                      {
                        label: "Harness",
                        value: spec.harness ? brandName(spec.harness.type) : "Chosen per run",
                      },
                      spec.harness && {
                        label: "Harness session",
                        value: spec.harness.executionMode === "on_demand" ? "New for each run" : "Kept alive between runs",
                      },
                      spec.harness && { label: "Placement", value: titleCase(spec.harness.placement) },
                      { label: "Models managed by", value: titleCase(spec.model.management) },
                      { label: "Provider", value: spec.model.provider ?? "Harness default" },
                      { label: "Model", value: spec.model.model ?? "Harness default" },
                      {
                        label: "Skills",
                        value: spec.skills.length ? (
                          <span className="inline-flex flex-wrap justify-end gap-x-2 gap-y-0.5">
                            {spec.skills.map((slug) => {
                              const skill = skillList.data?.find((item) => item.slug === slug);
                              return skill ? (
                                <Link key={slug} href={`/skills/${skill.id}`} className="text-primary underline-offset-4 hover:underline">
                                  {skill.name}
                                </Link>
                              ) : (
                                <span key={slug} className="text-warning" title="This skill no longer exists">
                                  {slug} (missing)
                                </span>
                              );
                            })}
                          </span>
                        ) : (
                          "None"
                        ),
                      },
                    ]}
                  />
                </CardContent>
              </Card>

              <Card>
                <CardHeader>
                  <CardTitle as="h2">Autonomy and limits</CardTitle>
                </CardHeader>
                <CardContent>
                  <DescriptionList
                    items={[
                      { label: "Autonomy", value: `${autonomyName(autonomy.level)} (level ${autonomy.level})` },
                      { label: "Approval", value: titleCase(autonomy.approvalMode) },
                      {
                        label: "Rate limit",
                        value: autonomy.maxExecutionsPerHour
                          ? `${autonomy.maxExecutionsPerHour} per hour`
                          : "Unlimited",
                      },
                      { label: "Concurrent runs", value: String(spec.resources.maxConcurrentExecutions) },
                      { label: "Timeout", value: `${spec.resources.timeoutSeconds} s` },
                    ]}
                  />
                </CardContent>
              </Card>

              <Card>
                <CardHeader>
                  <CardTitle as="h2">Connectors</CardTitle>
                </CardHeader>
                <CardContent>
                  {spec.connectors.length === 0 ? (
                    <p className="text-sm text-muted-foreground">No external services granted.</p>
                  ) : (
                    <ul className="space-y-3">
                      {spec.connectors.map((connector) => (
                        <li key={connector.connector}>
                          <p className="text-sm font-medium">{brandName(connector.connector)}</p>
                          <div className="mt-1.5 flex flex-wrap gap-1">
                            {connector.capabilities.length ? (
                              connector.capabilities.map((capability) => (
                                <Badge key={capability} variant="outline" title={capability}>
                                  {capabilityLabel(capability)}
                                </Badge>
                              ))
                            ) : (
                              <span className="text-[13px] text-muted-foreground">Read-only defaults</span>
                            )}
                          </div>
                        </li>
                      ))}
                    </ul>
                  )}
                </CardContent>
              </Card>
            </div>
          </div>
        </TabsContent>

        <TabsContent value="executions">
          {executions.isLoading ? (
            <Card className="overflow-hidden">
              {Array.from({ length: 3 }).map((_, index) => (
                <div key={index} className="border-b px-5 py-3.5 last:border-b-0">
                  <Skeleton className="h-9" />
                </div>
              ))}
            </Card>
          ) : executions.data?.items.length ? (
            <Card className="overflow-hidden">
              <ExecutionListHeader showAgent={false} />
              {executions.data.items.map((execution) => (
                <ExecutionRow key={execution.id} execution={execution} showAgent={false} />
              ))}
            </Card>
          ) : (
            <EmptyState
              icon={<Play />}
              title="This agent has not run yet"
              description="Run it with a task to see the timeline, output and usage here."
              action={
                archived ? undefined : (
                  <RunAgentDialog
                agentId={data.id}
                agentName={data.name}
                defaultTask={defaultTask}
                agentHarness={spec.harness?.type}
              />
                )
              }
            />
          )}
        </TabsContent>

        <TabsContent value="versions">
          <Card className="overflow-hidden">
            {versions.isLoading ? (
              <div className="p-5">
                <Skeleton className="h-9" />
              </div>
            ) : (
              <ul>
                {(versions.data ?? []).map((version) => (
                  <li
                    key={version.id}
                    className="flex flex-wrap items-center justify-between gap-x-4 gap-y-1 border-b px-4 py-3 last:border-b-0 sm:px-5"
                  >
                    <div className="flex items-center gap-2">
                      <span className="text-sm font-medium">Version {version.version}</span>
                      {version.version === data.currentVersion ? <Badge>Current</Badge> : null}
                    </div>
                    <div className="flex items-center gap-4 text-[13px] text-muted-foreground">
                      <span className="font-mono text-xs" title={`Checksum ${version.checksum}`}>
                        {version.checksum.slice(0, 12)}
                      </span>
                      <span className="tabular">{formatDateTime(version.createdAt)}</span>
                    </div>
                  </li>
                ))}
              </ul>
            )}
          </Card>
          <p className="mt-3 text-[13px] text-muted-foreground">
            Every save creates a new immutable version. Past executions keep the version they ran with.
          </p>
        </TabsContent>

        <TabsContent value="configuration">
          <Card>
            <CardHeader className="flex-row items-center justify-between gap-3">
              <div>
                <CardTitle as="h2">Definition</CardTitle>
                <p className="mt-0.5 text-[13px] text-muted-foreground">
                  Portable YAML, schema <span className="font-mono text-xs">orbit.io/v1</span>
                </p>
              </div>
              <div className="flex gap-2">
                <Button
                  variant="outline"
                  size="sm"
                  disabled={!yaml}
                  onClick={async () => {
                    if (await copyText(yaml)) toast.success("Definition copied");
                    else toast.error("Copying is not available in this browser");
                  }}
                >
                  <Copy /> Copy
                </Button>
                <Button variant="outline" size="sm" onClick={() => void downloadYaml()}>
                  <Download /> Download
                </Button>
              </div>
            </CardHeader>
            <CardContent>
              <pre className="code-block max-h-[560px]">{yaml || "# definition unavailable"}</pre>
            </CardContent>
          </Card>
        </TabsContent>
      </Tabs>
    </div>
  );
}
