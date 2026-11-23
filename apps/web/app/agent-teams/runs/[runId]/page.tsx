"use client";

import { LoadFailure } from "@/components/load-failure";
import { useCallback, useMemo, useState } from "react";
import Link from "next/link";
import { useParams } from "next/navigation";
import { ExternalLink } from "lucide-react";

import { StepNumber, agentLookup, teamMode } from "@/components/teams/team-parts";
import { WorkflowView } from "@/components/teams/workflow/workflow-view";
import type { Workflow } from "@/lib/workflow";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Callout } from "@/components/ui/callout";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { DescriptionList } from "@/components/ui/description-list";
import { PageHeader } from "@/components/ui/page-header";
import { Skeleton } from "@/components/ui/skeleton";
import { ExecutionStatusBadge } from "@/components/ui/status-badge";
import { useAgentTeam, useAllAgents, useTeamRun } from "@/lib/queries";
import { cn, formatDateTime, relativeTime } from "@/lib/utils";

const DONE_STEP = new Set(["succeeded", "completed"]);

function duration(start?: string | null, end?: string | null): string | null {
  if (!start) return null;
  const ms = (end ? new Date(end).getTime() : Date.now()) - new Date(start).getTime();
  if (!Number.isFinite(ms) || ms < 0) return null;
  const seconds = Math.round(ms / 1000);
  if (seconds < 60) return `${seconds} s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes} min ${seconds % 60} s`;
  return `${Math.floor(minutes / 60)} h ${minutes % 60} min`;
}

/** One team run: progress, each step and its execution. Refreshes until the run ends. */
export default function TeamRunPage() {
  const params = useParams<{ runId: string }>();
  const run = useTeamRun(params.runId);
  const team = useAgentTeam(run.data?.run.teamId ?? "");
  const agents = useAllAgents();
  const agentName = useMemo(() => agentLookup(agents.data?.items ?? []), [agents.data]);
  const workflowAgentName = useMemo(() => (id: string) => agentName(id).name, [agentName]);
  // Selecting a step on the graph points at its latest row in the list.
  const [highlighted, setHighlighted] = useState<number | null>(null);
  const showStep = useCallback((stepIndex: number) => {
    setHighlighted(stepIndex);
    document.getElementById(`team-step-${stepIndex}`)?.scrollIntoView({ block: "center", behavior: "smooth" });
  }, []);

  if (run.isLoading) {
    return (
      <div className="space-y-4">
        <Skeleton className="h-10 w-80" />
        <Skeleton className="h-64 rounded-xl" />
      </div>
    );
  }
  if (!run.data) {
    return (
      <LoadFailure
        noun="run"
        error={run.error}
        back={{ href: "/agent-teams/runs", label: "Back to runs" }}
        onRetry={() => void run.refetch()}
        retrying={run.isFetching}
      />
    );
  }

  const { run: detail, steps } = run.data;
  const task = String(detail.input.task ?? "") || "Agent team run";
  const repository = (detail.input.payload as { repository?: string } | undefined)?.repository;
  const done = steps.filter((step) => DONE_STEP.has(step.status)).length;
  // A pipeline creates each step when the previous one finishes. Show the agents still to
  // come from the team definition, so the whole line-up and the real total are visible.
  // A workflow run draws the graph it was started with, even if the team changed since.
  const snapshot = detail.input.definition as { mode?: string; workflow?: Workflow | null } | undefined;
  const workflow = snapshot?.mode === "workflow" ? (snapshot.workflow ?? null) : null;
  const planned =
    team.data && teamMode(team.data) === "pipeline" && !detail.finishedAt ? (team.data.definition.steps ?? []) : [];
  const upcoming = planned.slice(steps.length);
  const totalSteps = steps.length + upcoming.length;
  const teamName = team.data?.name ?? "Agent team";
  const total = duration(detail.createdAt, detail.finishedAt);

  return (
    <div className="space-y-6">
      <PageHeader
        back={{ href: "/agent-teams/runs", label: "Team runs" }}
        title={<span className="line-clamp-3">{task}</span>}
        meta={
          <>
            <ExecutionStatusBadge status={detail.status} />
            <span className="text-[13px] text-muted-foreground">
              Started {relativeTime(detail.createdAt)}
              {total ? `, ${detail.finishedAt ? "took" : "running for"} ${total}` : ""}
            </span>
          </>
        }
      />

      {detail.status === "cancelled" ? (
        <Callout tone="neutral" title="The run was stopped">
          {detail.error === "cancelled by operator" ? "Someone cancelled it before it finished." : detail.error}
        </Callout>
      ) : detail.error ? (
        <Callout tone="danger" title="The run failed">
          {detail.error}
        </Callout>
      ) : null}

      {workflow ? (
        <Card>
          <CardHeader>
            <CardTitle as="h2">Workflow</CardTitle>
            <p className="text-[13px] text-muted-foreground">
              Coloured steps have run; a dashed step or line was not reached{detail.finishedAt ? "" : " yet"}. Select a
              step to find it in the list below.
            </p>
          </CardHeader>
          <CardContent>
            <WorkflowView
              workflow={workflow}
              agentName={workflowAgentName}
              steps={steps}
              failed={detail.status === "failed"}
              onSelectStep={showStep}
            />
          </CardContent>
        </Card>
      ) : null}

      <div className="grid gap-5 lg:grid-cols-[minmax(0,1fr)_20rem] [&>*]:min-w-0">
        <Card>
          <CardHeader className="gap-3">
            <div className="flex flex-wrap items-baseline justify-between gap-2">
              <CardTitle as="h2">{workflow ? "Steps, in the order they started" : "Steps"}</CardTitle>
              <p className="tabular text-[13px] text-muted-foreground">
                {done} of {totalSteps} {totalSteps === 1 ? "step" : "steps"} done
              </p>
            </div>
            <div
              hidden={Boolean(workflow)}
              role="progressbar"
              aria-label="Steps done"
              aria-valuemin={0}
              aria-valuemax={totalSteps}
              aria-valuenow={done}
              className="h-1.5 overflow-hidden rounded-full bg-muted"
            >
              <div
                className="h-full bg-primary transition-[width]"
                style={{ width: `${totalSteps ? (done / totalSteps) * 100 : 0}%` }}
              />
            </div>
          </CardHeader>
          <CardContent className="p-0 sm:p-0">
            {steps.length ? (
              <ol className="divide-y border-t">
                {steps.map((step) => {
                  const took = duration(step.startedAt, step.finishedAt);
                  return (
                    <li
                      key={step.id}
                      id={`team-step-${step.stepIndex}`}
                      className={cn(
                        "flex scroll-mt-24 flex-wrap items-center gap-x-3 gap-y-2 px-4 py-3.5 transition-colors sm:px-5",
                        highlighted === step.stepIndex && "bg-accent/60",
                      )}
                    >
                      <div className="flex min-w-[min(100%,14rem)] flex-1 items-start gap-3">
                        <StepNumber n={step.stepIndex + 1} className="mt-0.5" />
                        <div className="min-w-0 space-y-0.5">
                          <p className="flex flex-wrap items-center gap-2 text-sm font-medium">
                            <Link href={`/agents/${step.agentId}`} className="min-w-0 break-words hover:text-primary hover:underline">
                              {step.name}
                            </Link>
                            {workflow ? (
                              <>
                                {step.attempt > 1 ? <Badge variant="secondary">Run {step.attempt}</Badge> : null}
                                {step.outcome ? <Badge>{step.outcome}</Badge> : null}
                              </>
                            ) : step.stepIndex === 0 ? (
                              <Badge>Primary</Badge>
                            ) : null}
                          </p>
                          <p className="text-xs text-muted-foreground empty:hidden">
                            {[
                              workflow ? agentName(step.agentId).name : null,
                              took ? `${step.finishedAt ? "Took" : "Running for"} ${took}` : null,
                              step.skills.length ? `${step.skills.length} ${step.skills.length === 1 ? "skill" : "skills"}` : null,
                            ]
                              .filter(Boolean)
                              .join(" · ")}
                          </p>
                          {step.error ? <p className="break-words text-[13px] text-destructive">{step.error}</p> : null}
                        </div>
                      </div>
                      <div className="ml-9 flex items-center gap-2 sm:ml-auto">
                        <ExecutionStatusBadge status={step.status} />
                        {step.executionId ? (
                          <Button asChild size="sm" variant="outline">
                            <Link href={`/executions/${step.executionId}`}>
                              Open execution <ExternalLink />
                            </Link>
                          </Button>
                        ) : null}
                      </div>
                    </li>
                  );
                })}
                {upcoming.map((step, index) => (
                  <li key={`upcoming-${index}`} className="flex items-center gap-3 px-4 py-3.5 text-muted-foreground sm:px-5">
                    <StepNumber n={steps.length + index + 1} className="border-dashed bg-transparent" />
                    <span className="min-w-0 flex-1 truncate text-sm">{agentName(step.agentId).name}</span>
                    <span className="text-xs">Up next</span>
                  </li>
                ))}
              </ol>
            ) : (
              <p className="border-t px-5 py-4 text-sm text-muted-foreground">No steps have been created yet.</p>
            )}
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle as="h2">Details</CardTitle>
          </CardHeader>
          <CardContent>
            <DescriptionList
              items={[
                {
                  label: "Team",
                  value: (
                    <Link href={`/agent-teams/${detail.teamId}`} className="text-primary hover:underline">
                      {teamName}
                    </Link>
                  ),
                },
                repository ? { label: "Repository", value: repository, mono: true } : null,
                { label: "Started", value: formatDateTime(detail.createdAt) },
                detail.finishedAt ? { label: "Finished", value: formatDateTime(detail.finishedAt) } : null,
                detail.input.autoApprove === true ? { label: "Approvals", value: "Auto-approved" } : null,
              ]}
            />
          </CardContent>
        </Card>
      </div>
    </div>
  );
}
