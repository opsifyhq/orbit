"use client";

import Link from "next/link";
import { ArrowRight, Bot, Boxes, Play, Plus, Timer } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Callout } from "@/components/ui/callout";
import { Card } from "@/components/ui/card";
import { EmptyState } from "@/components/ui/empty-state";
import { PageHeader } from "@/components/ui/page-header";
import { Section } from "@/components/ui/section";
import { Skeleton } from "@/components/ui/skeleton";
import { StatusGlyph } from "@/components/ui/status";
import { AgentStatusBadge } from "@/components/ui/status-badge";
import { StatStrip } from "@/components/dashboard/stat-strip";
import { UsageChart } from "@/components/dashboard/usage-chart";
import { ExecutionRow } from "@/components/executions/execution-row";
import { useAgents, useDashboard, useDefaultHarness } from "@/lib/queries";
import { errorMessage } from "@orbit/api-client";
import { relativeTime } from "@/lib/utils";

function plural(count: number, noun: string) {
  return `${count} ${noun}${count === 1 ? "" : "s"}`;
}

export default function OverviewPage() {
  const dashboard = useDashboard();
  // Archived agents are not "your agents": ask for the current ones, not six of anything.
  const agents = useAgents({ limit: 6, status: "current" });
  const harnessDefault = useDefaultHarness();
  const summary = dashboard.data?.summary;
  const usage =
    ((dashboard.data?.usage ?? {}) as { days?: Array<{ day: string; executions: number }> }).days ??
    [];
  const firstRun = summary ? summary.totalAgents === 0 : false;
  const currentAgents = agents.data?.items ?? [];

  return (
    <div className="space-y-7">
      <PageHeader
        title="Overview"
        description="What your agents are doing, and what needs you."
        actions={
          <>
            <Button asChild variant="outline">
              <Link href="/agents/new">
                <Plus /> New agent
              </Link>
            </Button>
            {/* The sidebar carries Start work on desktop; phones need it here. */}
            <Button asChild className="lg:hidden">
              <Link href="/work">
                <Play /> Start work
              </Link>
            </Button>
          </>
        }
      />

      {dashboard.isError ? (
        <Callout tone="danger" title="The overview could not be loaded">
          {errorMessage(dashboard.error)}
        </Callout>
      ) : null}

      {summary && summary.pendingApprovals > 0 ? (
        <Callout
          tone="warning"
          title={`${plural(summary.pendingApprovals, "action")} waiting for your approval`}
          action={
            <Button asChild size="sm">
              <Link href="/approvals">
                Review <ArrowRight />
              </Link>
            </Button>
          }
        >
          The executions that asked are paused until someone decides.
        </Callout>
      ) : null}

      {dashboard.isLoading || !summary ? (
        <Skeleton className="h-[212px] rounded-xl lg:h-[108px]" />
      ) : (
        <StatStrip
          stats={[
            {
              label: "Running now",
              value: summary.runningExecutions,
              tone: summary.runningExecutions > 0 ? "info" : "default",
              glyph:
                summary.runningExecutions > 0 ? (
                  <StatusGlyph shape="orbiting" tone="info" className="h-5 w-5" />
                ) : null,
              detail: summary.runningExecutions > 0 ? "Live executions" : "Nothing in flight",
              href: "/executions?status=running",
            },
            {
              label: "Executions, last 24 h",
              value: summary.executions24h,
              detail: `${plural(summary.enabledTriggers, "trigger")} enabled`,
              href: "/executions",
            },
            {
              label: "Failed, last 24 h",
              value: summary.failedExecutions24h,
              tone: summary.failedExecutions24h > 0 ? "danger" : "default",
              detail: summary.failedExecutions24h > 0 ? "Open to see why" : "No failures",
              href: "/executions?status=failed",
            },
            {
              label: "Agents",
              value: summary.totalAgents,
              detail: [
                `${summary.activeAgents} active`,
                summary.draftAgents ? `${summary.draftAgents} draft` : null,
                summary.pausedAgents ? `${summary.pausedAgents} paused` : null,
              ]
                .filter(Boolean)
                .join(", "),
              href: "/agents",
            },
          ]}
        />
      )}

      {harnessDefault.known && !harnessDefault.defaultHarness ? (
        <Callout
          tone="info"
          title="No default harness is set"
          action={
            <Button asChild size="sm" variant="outline">
              <Link href="/settings#platform">Choose a default</Link>
            </Button>
          }
        >
          Agents do not carry a harness of their own, so every run asks for one. Set an organization
          default once and runs can start without choosing.
        </Callout>
      ) : null}

      {firstRun ? <GettingStarted /> : null}

      <div className="grid gap-7 lg:grid-cols-[minmax(0,1fr)_20rem] lg:gap-8">
        <Section
          title="Recent executions"
          actions={
            <Button asChild variant="ghost" size="sm">
              <Link href="/executions">
                All executions <ArrowRight />
              </Link>
            </Button>
          }
        >
          <Card className="overflow-hidden">
            {dashboard.isLoading ? (
              <div>
                {Array.from({ length: 5 }).map((_, index) => (
                  <div key={index} className="border-b px-5 py-3.5 last:border-b-0">
                    <Skeleton className="h-9" />
                  </div>
                ))}
              </div>
            ) : summary?.recentExecutions.length ? (
              summary.recentExecutions.map((execution) => (
                <ExecutionRow key={execution.id} execution={execution} />
              ))
            ) : (
              <EmptyState
                icon={<Timer />}
                title="No executions yet"
                description="Start work with an agent, or add a schedule trigger, and its executions appear here."
                action={
                  <Button asChild size="sm">
                    <Link href="/work">
                      <Play /> Start work
                    </Link>
                  </Button>
                }
                className="border-0"
              />
            )}
          </Card>
        </Section>

        <div className="space-y-7">
          <Card className="p-4 sm:p-5">
            <UsageChart days={usage} />
          </Card>

          <Section
            title="Agents"
            actions={
              <Button asChild variant="ghost" size="sm">
                <Link href="/agents">
                  All agents <ArrowRight />
                </Link>
              </Button>
            }
          >
            <Card className="overflow-hidden">
              {agents.isLoading ? (
                <div className="space-y-2 p-4">
                  {Array.from({ length: 3 }).map((_, index) => (
                    <Skeleton key={index} className="h-9" />
                  ))}
                </div>
              ) : currentAgents.length ? (
                currentAgents.slice(0, 5).map((agent) => (
                  <Link
                    key={agent.id}
                    href={`/agents/${agent.id}`}
                    className="flex items-center gap-3 border-b px-4 py-2.5 transition-colors last:border-b-0 hover:bg-muted/50 focus-visible:outline-offset-[-2px]"
                  >
                    <AgentStatusBadge status={agent.status} hideLabel className="h-4 w-4" />
                    <span className="min-w-0 flex-1">
                      <span className="block truncate text-sm font-medium">{agent.name}</span>
                      <span className="block truncate text-xs text-muted-foreground">
                        Updated {relativeTime(agent.updatedAt)}
                      </span>
                    </span>
                  </Link>
                ))
              ) : (
                <p className="p-4 text-sm text-muted-foreground">No agents yet.</p>
              )}
            </Card>
          </Section>
        </div>
      </div>
    </div>
  );
}

/** First-run path. Numbered because the order matters: nothing runs without an agent. */
function GettingStarted() {
  const steps = [
    {
      icon: Bot,
      title: "Create an agent",
      body: "Give it instructions, a harness and an autonomy level. It starts as a draft you can test.",
      href: "/agents/new",
      cta: "New agent",
    },
    {
      icon: Boxes,
      title: "Connect what it needs",
      body: "Add a model provider and any connectors. Credentials are encrypted and never shown again.",
      href: "/models",
      cta: "Model providers",
    },
    {
      icon: Play,
      title: "Start work",
      body: "Hand the agent a task and watch the execution stream in live.",
      href: "/work",
      cta: "Start work",
    },
  ];
  return (
    <Section title="Get your first agent running">
      <ol className="grid gap-3 md:grid-cols-3">
        {steps.map((step, index) => (
          <li key={step.title} className="flex flex-col gap-3 rounded-xl border bg-card p-4 sm:p-5">
            <span className="tabular text-[13px] text-muted-foreground">Step {index + 1}</span>
            <div className="space-y-1">
              <p className="flex items-center gap-2 text-[15px] font-semibold">
                <step.icon className="h-4 w-4 text-primary" /> {step.title}
              </p>
              <p className="text-[13px] text-muted-foreground">{step.body}</p>
            </div>
            <Button asChild variant="outline" size="sm" className="mt-auto self-start">
              <Link href={step.href}>
                {step.cta} <ArrowRight />
              </Link>
            </Button>
          </li>
        ))}
      </ol>
    </Section>
  );
}
