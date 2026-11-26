"use client";

export const dynamic = "force-dynamic";
export const dynamicParams = true;

import { LoadFailure } from "@/components/load-failure";
import Link from "next/link";
import { useParams, useRouter } from "next/navigation";
import { Suspense, useEffect, useMemo, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Ban, Bot, Check, Copy, RotateCw, Workflow, X } from "lucide-react";
import { toast } from "sonner";

import { executionTask, executionTitle } from "@/components/executions/execution-row";
import { ExecutionTimeline } from "@/components/executions/execution-timeline";
import { Button } from "@/components/ui/button";
import { Callout } from "@/components/ui/callout";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { useConfirm } from "@/components/ui/confirm";
import { DescriptionList } from "@/components/ui/description-list";
import { PageHeader } from "@/components/ui/page-header";
import { Segmented } from "@/components/ui/segmented";
import { Skeleton } from "@/components/ui/skeleton";
import { ExecutionStatusBadge } from "@/components/ui/status-badge";
import { api } from "@/lib/api";
import { copyText } from "@/lib/browser";
import { errorStatus, request } from "@/lib/http";
import {
  queryKeys,
  unwrap,
  useApprovals,
  useExecution,
  useExecutionEvents,
  useWorkers,
} from "@/lib/queries";
import { useExecutionStream } from "@/lib/use-execution-stream";
import { errorMessage } from "@orbit/api-client";
import {
  formatDateTime,
  formatDuration,
  formatTokens,
  isTerminalStatus,
  relativeTime,
  titleCase,
} from "@/lib/utils";
import { brandName, capabilityLabel, humanizeIdentifier } from "@/lib/presentation";
import { dispatchedModel, lastAgentMessage } from "@/lib/execution-events";
import { Markdown } from "@/components/ui/markdown";
import { ApprovalDetails } from "@/components/approvals/approval-details";
import type { ExecutionEventRecord, ExecutionRecord } from "@/lib/types";

/** Reasons the runtime stopped a worker container; the run's outcome is unknown. */
const WORKER_TERMINATION_CODES = ["worker_oom", "worker_evicted", "worker_exited", "worker_lost"];

/** Outcomes the platform decided on, worded for the person reading the run. */
const PLATFORM_ERROR_TEXT = new Map<string, string>([
  [
    "worker_did_not_start",
    "A worker took this run but never started it, so ORBIT ended it. This usually means the worker could not read the run, for example because it is older than the platform, or that its container never started, for example because its image could not be pulled. Nothing ran, so the run can be retried. Its logs and launches in Workers say more.",
  ],
  [
    "worker_oom",
    "The worker container was OOMKilled. Increase the worker profile memory limit, then check for side effects before running it again.",
  ],
  [
    "worker_evicted",
    "The worker pod was evicted by the cluster (node pressure or a drain). Check for side effects before running it again.",
  ],
  [
    "worker_exited",
    "The worker container exited unexpectedly. Check its logs in Workers and any side effects before running it again.",
  ],
  [
    "worker_lost",
    "The worker running this stopped responding, restarted or was lost, so the outcome is unknown. Check whether it made any changes before running it again.",
  ],
  [
    "harness_unresponsive",
    "The worker is still online but stopped reporting this run, so the outcome is unknown. Check whether it made any changes before running it again.",
  ],
]);

export default function ExecutionDetailPage() {
  // Event filters read the URL; keep the rest of the page prerenderable.
  return (
    <Suspense fallback={<DetailSkeleton />}>
      <ExecutionDetail />
    </Suspense>
  );
}

function ExecutionDetail() {
  const params = useParams<{ id: string }>();
  const executionId = params.id;
  const router = useRouter();
  const queryClient = useQueryClient();
  const confirm = useConfirm();
  const executionQuery = useExecution(executionId);
  const eventsQuery = useExecutionEvents(executionId);
  // The stream continues where the fetched page ends instead of replaying all of it.
  // If that fetch fails the stream still starts, from the beginning.
  const fetchedThrough = eventsQuery.data?.items.at(-1)?.seq ?? 0;
  const live = useExecutionStream(executionId, {
    enabled: !eventsQuery.isLoading,
    afterSeq: fetchedThrough,
  });
  const workers = useWorkers();

  // The stream's snapshot is only sent on connect and at the end; the polled record
  // catches changes in between. Show whichever is newer.
  const execution =
    live.execution && executionQuery.data
      ? Date.parse(executionQuery.data.updatedAt) > Date.parse(live.execution.updatedAt)
        ? executionQuery.data
        : live.execution
      : (live.execution ?? executionQuery.data);
  // Only a run that is waiting can have a pending approval: nothing to poll otherwise.
  const pendingApprovals = useApprovals("pending", {
    enabled: execution?.status === "awaiting_approval",
    // Asked for by execution: a long org-wide queue cannot hide this run's request.
    executionId,
  });
  const pendingApproval =
    execution?.status === "awaiting_approval"
      ? pendingApprovals.data?.find((approval) => approval.executionId === executionId)
      : undefined;
  const events = useMemo(() => {
    const merged = new Map<number, ExecutionEventRecord>();
    for (const event of eventsQuery.data?.items ?? []) merged.set(event.seq, event);
    for (const event of live.events) merged.set(event.seq, event);
    return [...merged.values()].sort((a, b) => a.seq - b.seq);
  }, [eventsQuery.data, live.events]);

  const lastSaid = useMemo(() => lastAgentMessage(events), [events]);
  const model = useMemo(() => dispatchedModel(events), [events]);

  const cancel = useMutation({
    mutationFn: async () =>
      unwrap(
        await api.POST("/api/v1/executions/{execution_id}/cancel", {
          params: { path: { execution_id: executionId } },
        }),
      ),
    onSuccess: () => {
      toast.success("Cancellation requested");
      void queryClient.invalidateQueries({ queryKey: queryKeys.execution(executionId) });
      void queryClient.invalidateQueries({ queryKey: queryKeys.executions() });
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  // The API re-queues the execution with its original input and harness.
  const rerun = useMutation({
    mutationFn: async () =>
      request<{ id: string }>(`/api/v1/executions/${executionId}/rerun`, { method: "POST" }),
    onSuccess: (created) => {
      toast.success("Execution queued");
      void queryClient.invalidateQueries({ queryKey: queryKeys.executions() });
      router.push(`/executions/${created.id}`);
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  const decideApproval = useMutation({
    mutationFn: async ({
      approvalId,
      decision,
    }: {
      approvalId: string;
      decision: "approved" | "rejected";
    }) =>
      unwrap(
        await api.POST(
          decision === "approved"
            ? "/api/v1/approvals/{approval_id}/approve"
            : "/api/v1/approvals/{approval_id}/reject",
          {
            params: { path: { approval_id: approvalId } },
            body: {},
          },
        ),
      ),
    onSuccess: (_, variables) => {
      toast.success(variables.decision === "approved" ? "Approved" : "Rejected");
      void queryClient.invalidateQueries({ queryKey: queryKeys.approvalLists });
      void queryClient.invalidateQueries({ queryKey: queryKeys.execution(executionId) });
      void queryClient.invalidateQueries({ queryKey: queryKeys.dashboard });
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  const liveStatus = live.execution?.status;
  useEffect(() => {
    if (liveStatus && isTerminalStatus(liveStatus)) {
      void queryClient.invalidateQueries({ queryKey: queryKeys.execution(executionId) });
      void queryClient.invalidateQueries({ queryKey: queryKeys.executions() });
      void queryClient.invalidateQueries({ queryKey: queryKeys.dashboard });
    }
  }, [liveStatus, executionId, queryClient]);

  if (executionQuery.isLoading && !execution) return <DetailSkeleton />;

  if (!execution) {
    return (
      <LoadFailure
        noun="execution"
        error={executionQuery.error}
        status={errorStatus(executionQuery.error) ?? live.fatal?.status}
        message={executionQuery.error ? errorMessage(executionQuery.error) : live.fatal?.message}
        back={{ href: "/executions", label: "Back to executions" }}
        onRetry={() => void executionQuery.refetch()}
        retrying={executionQuery.isFetching}
      />
    );
  }

  const active = !isTerminalStatus(execution.status);
  const task = executionTask(execution);
  const usage = (execution.usage && typeof execution.usage === "object" ? execution.usage : {}) as {
    inputTokens?: number;
    outputTokens?: number;
  };
  const errorCode = (execution.error as { code?: string } | null)?.code;
  const errorText = !execution.error
    ? null
    : (PLATFORM_ERROR_TEXT.get(errorCode ?? "") ??
      String((execution.error as { message?: string }).message ?? "Unknown error"));

  const errorInfo = (execution.error ?? {}) as {
    kind?: string;
    retryable?: boolean;
    workerName?: string;
    restartCount?: number;
    terminatedAt?: string;
    exitCode?: number;
  };
  const terminatedByRuntime = WORKER_TERMINATION_CODES.includes(errorCode ?? "");
  const lostByWorker = terminatedByRuntime || errorCode === "harness_unresponsive";
  const terminationMeta = terminatedByRuntime
    ? [
        errorInfo.workerName ? `Worker/pod ${errorInfo.workerName}` : null,
        typeof errorInfo.exitCode === "number" ? `Exit code ${errorInfo.exitCode}` : null,
        typeof errorInfo.restartCount === "number" ? `Restarts ${errorInfo.restartCount}` : null,
        errorInfo.terminatedAt ? `Terminated ${relativeTime(errorInfo.terminatedAt)}` : null,
      ]
        .filter(Boolean)
        .join(" · ")
    : "";
  const errorMeta =
    execution.error && execution.status !== "cancelled"
    ? [
        // "execution_failed" only repeats the status already in the title.
        errorInfo.kind && errorInfo.kind !== `execution_${execution.status}`
          ? humanizeIdentifier(errorInfo.kind)
          : null,
        errorInfo.retryable === true
          ? "Running it again may succeed."
          : errorInfo.retryable === false && !lostByWorker
            ? "Running it again unchanged will fail the same way."
            : null,
        lostByWorker ? "Check for side effects before using Run again." : null,
      ]
        .filter(Boolean)
        .join(". ")
    : "";

  const waitedForApproval = events.some((event) => /^approval requested/.test(event.message));

  const timeline = (
    <ExecutionTimeline
      events={events}
      active={active}
      connected={live.connected}
      status={execution.status}
      pendingApproval={Boolean(pendingApproval)}
    />
  );

  // A run that ends without a result (timeout, cancel) still said something on the way.
  const partial = !execution.output && !active ? lastSaid : null;
  const output = (
    <Card className="min-w-0 overflow-hidden">
      <DataPanel
        title={partial ? "Last message from the agent" : "Output"}
        value={execution.output ?? partial}
        markdown
        empty={active ? "The answer appears here when the agent finishes." : "This execution returned no output."}
      />
    </Card>
  );

  return (
    <div className="space-y-6">
      <PageHeader
        back={{ href: "/executions", label: "Executions" }}
        title={executionTitle(execution)}
        description={task ? <span className="line-clamp-3">{task}</span> : "No task recorded"}
        meta={
          <>
            <ExecutionStatusBadge status={pendingApproval && active ? "awaiting_approval" : execution.status} />
            <Link
              href={`/agents/${execution.agentId}`}
              className="inline-flex items-center gap-1.5 text-[13px] font-medium text-foreground underline-offset-4 hover:underline"
            >
              <Bot className="h-3.5 w-3.5 text-muted-foreground" /> {execution.agentName}
            </Link>
            <span className="text-[13px] text-muted-foreground">
              Started {relativeTime(execution.startedAt ?? execution.createdAt)}
            </span>
          </>
        }
        actions={
          active ? (
            <Button
              variant="outline"
              className="text-destructive hover:border-destructive/50 hover:bg-destructive/10"
              loading={cancel.isPending}
              onClick={async () => {
                const confirmed = await confirm({
                  title: "Cancel this execution?",
                  description: "The harness is told to stop. Work already done is not rolled back.",
                  confirmLabel: "Cancel execution",
                  cancelLabel: "Keep running",
                  destructive: true,
                });
                if (confirmed) cancel.mutate();
              }}
            >
              <Ban /> Cancel
            </Button>
          ) : execution.teamRunId ? (
            // A team step ran with its team's connections and environment; on its own it
            // would not be the same run. The team run is what can be repeated.
            <Button asChild variant="outline">
              <Link href={`/agent-teams/runs/${execution.teamRunId}`}>
                <Workflow /> Open team run
              </Link>
            </Button>
          ) : task ? (
            <Button variant="outline" loading={rerun.isPending} onClick={() => rerun.mutate()}>
              <RotateCw /> Run again
            </Button>
          ) : null
        }
      />

      {pendingApproval ? (
        <Callout
          tone="warning"
          title="Paused: waiting for approval"
          action={
            <div className="flex gap-2">
              <Button
                size="sm"
                variant="outline"
                className="text-destructive hover:border-destructive/50 hover:bg-destructive/10"
                loading={
                  decideApproval.isPending &&
                  decideApproval.variables?.approvalId === pendingApproval.id &&
                  decideApproval.variables?.decision === "rejected"
                }
                disabled={decideApproval.isPending}
                onClick={() =>
                  decideApproval.mutate({
                    approvalId: pendingApproval.id,
                    decision: "rejected",
                  })
                }
              >
                <X /> Reject
              </Button>
              <Button
                size="sm"
                loading={
                  decideApproval.isPending &&
                  decideApproval.variables?.approvalId === pendingApproval.id &&
                  decideApproval.variables?.decision === "approved"
                }
                disabled={decideApproval.isPending}
                onClick={() =>
                  decideApproval.mutate({
                    approvalId: pendingApproval.id,
                    decision: "approved",
                  })
                }
              >
                <Check /> Approve
              </Button>
            </div>
          }
        >
          The agent wants to{" "}
          <span className="font-medium text-foreground">
            {capabilityLabel(
              (pendingApproval.requestedAction as { capability?: string })?.capability,
            ).toLowerCase()}
          </span>
          . Nothing happens until someone approves or rejects it
          {pendingApproval.expiresAt ? `, and the request expires ${relativeTime(pendingApproval.expiresAt)}` : ""}.
          <div className="mt-3">
            <ApprovalDetails approval={pendingApproval} />
          </div>
        </Callout>
      ) : null}

      {errorText ? (
        <Callout
          tone={execution.status === "cancelled" ? "neutral" : "danger"}
          title={`Execution ${titleCase(execution.status).toLowerCase()}`}
        >
          {execution.status === "timed_out" ? (
            <p>
              It ran past the agent&apos;s {execution.timeoutSeconds} s limit
              {waitedForApproval
                ? ". Time spent waiting for an approval is not counted. Raise the timeout in Agent Studio or give it a smaller task."
                : ". Raise the timeout in Agent Studio or give it a smaller task."}
            </p>
          ) : execution.status === "cancelled" ? (
            <p>It was stopped on request. Work done before that is not rolled back.</p>
          ) : (
            <>
              <p className="break-words text-foreground">{errorText}</p>
              {terminationMeta ? <p className="mt-1 text-[13px]">{terminationMeta}</p> : null}
              {errorMeta ? <p className="mt-1">{errorMeta}</p> : null}
            </>
          )}
        </Callout>
      ) : null}

      {execution.status === "queued" ? (
        <Callout
          tone="info"
          title="Waiting for a worker"
          action={
            <Button asChild size="sm" variant="outline">
              <Link href="/workers">Workers</Link>
            </Button>
          }
        >
          {queuedHint(execution, workers.data ?? [], workers.isLoading)}
        </Callout>
      ) : null}

      {execution.status === "dispatched" && silentSince(execution, events) ? (
        <Callout
          tone="warning"
          title="Picked up, but not started"
          action={
            <Button asChild size="sm" variant="outline">
              <Link href="/workers">Workers</Link>
            </Button>
          }
        >
          A worker took this run {relativeTime(silentSince(execution, events)!)} and has not reported since.
          ORBIT ends it after 10 minutes without activity, or a minute after you cancel it. Its logs in Workers usually say why, for example a worker older than the platform.
        </Callout>
      ) : null}

      <div className="grid gap-5 lg:grid-cols-[minmax(0,1fr)_21rem]">
        <div className="min-w-0 space-y-5">
          {/* The result leads once there is one; while running, the timeline does. */}
          {active || !execution.output ? (
            <>
              {timeline}
              {output}
            </>
          ) : (
            <>
              {output}
              {timeline}
            </>
          )}
        </div>

        <div className="min-w-0 space-y-5">
          <Card>
            <CardHeader>
              <CardTitle as="h2">Details</CardTitle>
            </CardHeader>
            <CardContent>
              <DescriptionList
                items={[
                  { label: "Trigger", value: titleCase(execution.triggerSource) },
                  { label: "Harness", value: brandName(execution.harnessKind) },
                  model && { label: "Model", value: model, mono: true },
                  {
                    label: "Duration",
                    value: execution.startedAt
                      ? formatDuration(execution.startedAt, execution.finishedAt)
                      : "Not started",
                  },
                  {
                    label: "Tokens",
                    value:
                      usage.inputTokens === undefined && usage.outputTokens === undefined
                        ? active
                          ? "Reported at the end"
                          : "Not reported"
                        : `${formatTokens(usage.inputTokens)} in, ${formatTokens(usage.outputTokens)} out`,
                  },
                  { label: "Created", value: formatDateTime(execution.createdAt) },
                  { label: "Started", value: formatDateTime(execution.startedAt) },
                  { label: "Finished", value: formatDateTime(execution.finishedAt) },
                  { label: "Timeout", value: `${execution.timeoutSeconds} s` },
                  execution.workerId && { label: "Worker", value: execution.workerId, mono: true },
                  execution.workerBootId && {
                    label: "Worker boot",
                    value: execution.workerBootId,
                    mono: true,
                  },
                  execution.lastExecutionHeartbeatAt && {
                    label: "Last execution heartbeat",
                    value: formatDateTime(execution.lastExecutionHeartbeatAt),
                  },
                  execution.executionLeaseUntil && {
                    label: "Lease until",
                    value: formatDateTime(execution.executionLeaseUntil),
                  },
                  execution.sessionRef && { label: "Session", value: execution.sessionRef, mono: true },
                  execution.idempotencyKey && {
                    label: "Idempotency key",
                    value: execution.idempotencyKey,
                    mono: true,
                  },
                ]}
              />
              <button
                type="button"
                className="mt-4 flex w-full items-center justify-between gap-2 rounded-md border bg-subtle px-2.5 py-1.5 text-left font-mono text-xs text-muted-foreground transition-colors hover:border-border-strong hover:text-foreground"
                onClick={async () => {
                  if (await copyText(execution.id)) toast.success("Execution ID copied");
                  else toast.error("Copying is not available in this browser");
                }}
              >
                <span className="truncate">{execution.id}</span>
                <Copy className="h-3.5 w-3.5 shrink-0" />
                <span className="sr-only">Copy execution ID</span>
              </button>
            </CardContent>
          </Card>

          <Card className="overflow-hidden">
            <DataPanel title="Input" value={execution.input} preferText={task || undefined} empty="No input." />
          </Card>

          {execution.error ? (
            <Card className="overflow-hidden">
              <DataPanel title="Error detail" value={execution.error} rawOnly empty="" />
            </Card>
          ) : null}
        </div>
      </div>

      {live.fatal ? (
        <p className="text-[13px] text-warning">
          Live updates stopped ({live.fatal.status === 404 ? "stream not found" : live.fatal.status === 403 ? "no access to the stream" : live.fatal.message}).
          Reload the page to try again.
        </p>
      ) : live.error ? (
        <p className="text-[13px] text-warning">Reconnecting, live updates are interrupted: {live.error}</p>
      ) : null}
    </div>
  );
}


function DetailSkeleton() {
  return (
    <div className="space-y-6">
      <Skeleton className="h-5 w-24" />
      <Skeleton className="h-10 w-80" />
      <div className="grid gap-5 lg:grid-cols-[minmax(0,1fr)_21rem]">
        <Skeleton className="h-96 rounded-xl" />
        <Skeleton className="h-72 rounded-xl" />
      </div>
    </div>
  );
}

/** A payload shown as readable text first, with the raw JSON one click away. */
function DataPanel({
  title,
  value,
  preferText,
  rawOnly,
  markdown,
  empty,
}: {
  title: string;
  value: unknown;
  preferText?: string;
  rawOnly?: boolean;
  /** Render the text as Markdown (agent answers) instead of plain text. */
  markdown?: boolean;
  empty: string;
}) {
  const [view, setView] = useState<"text" | "json">(rawOnly ? "json" : "text");
  const hasValue = value !== null && value !== undefined;
  const json = hasValue ? JSON.stringify(value, null, 2) : "";
  const text = preferText ?? renderOutput(value);

  return (
    <>
      <CardHeader className="flex-row items-center justify-between gap-3">
        <CardTitle as="h2">{title}</CardTitle>
        {hasValue ? (
          <div className="flex items-center gap-1.5">
            {rawOnly ? null : (
              <Segmented
                label={`${title} format`}
                size="sm"
                value={view}
                onChange={setView}
                options={[
                  { value: "text", label: markdown ? "Formatted" : "Text" },
                  { value: "json", label: "JSON" },
                ]}
              />
            )}
            <Button
              size="icon-sm"
              variant="ghost"
              aria-label={`Copy ${title.toLowerCase()}`}
              onClick={async () => {
                if (await copyText(view === "json" ? json : text)) toast.success(`${title} copied`);
                else toast.error("Copying is not available in this browser");
              }}
            >
              <Copy />
            </Button>
          </div>
        ) : null}
      </CardHeader>
      <CardContent>
        {!hasValue ? (
          <p className="text-sm text-muted-foreground">{empty}</p>
        ) : view === "json" ? (
          <pre className="code-block max-h-[520px]">{json}</pre>
        ) : markdown ? (
          <div className="max-h-[640px] overflow-auto">
            <Markdown>{text}</Markdown>
          </div>
        ) : (
          <div className="max-h-[520px] overflow-auto whitespace-pre-wrap break-words text-sm leading-relaxed">
            {text}
          </div>
        )}
      </CardContent>
    </>
  );
}

/** When a dispatched run has gone quiet for over two minutes, the time it went quiet. */
function silentSince(execution: ExecutionRecord, events: ExecutionEventRecord[]): string | null {
  const last = events.at(-1)?.createdAt ?? execution.startedAt ?? execution.createdAt;
  if (!last) return null;
  return Date.now() - Date.parse(last) > 120_000 ? last : null;
}

function queuedHint(
  execution: ExecutionRecord,
  workers: Array<{ live: boolean; harnesses: string[]; target: string }>,
  loading: boolean,
): string {
  const need = `It needs a ${brandName(execution.harnessKind)} worker${
    execution.placement && execution.placement !== "auto" ? ` on ${execution.placement}` : ""
  }.`;
  if (loading) return `${need} Checking which workers are online.`;
  const liveWorkers = workers.filter((worker) => worker.live);
  const matching = liveWorkers.filter(
    (worker) =>
      worker.harnesses.includes(execution.harnessKind) &&
      (!execution.placement || execution.placement === "auto" || worker.target === execution.placement),
  );
  if (matching.length > 0) {
    return `${need} ${matching.length} matching worker${matching.length === 1 ? " is" : "s are"} online and busy; it starts when a slot frees up.`;
  }
  if (liveWorkers.length === 0) return `${need} No workers are online. Launch one from Workers.`;
  return `${need} None of the ${liveWorkers.length} online worker${liveWorkers.length === 1 ? "" : "s"} can run it (${liveWorkers
    .map((worker) => `${worker.target}: ${worker.harnesses.join(", ")}`)
    .join("; ")}).`;
}

function renderOutput(value: unknown): string {
  if (typeof value === "string") return value;
  if (value === null || value === undefined) return "";
  if (typeof value !== "object") return String(value);

  // Prefer the field a person would read.
  const obj = value as Record<string, unknown>;
  for (const key of ["text", "output", "result", "message", "summary", "content", "answer", "response"]) {
    if (typeof obj[key] === "string" && (obj[key] as string).trim()) {
      return obj[key] as string;
    }
  }
  if (obj.output && typeof obj.output === "object") {
    const nested = renderOutput(obj.output);
    if (nested !== JSON.stringify(obj.output)) return nested;
  }
  if (Array.isArray(value)) {
    return value.map((item) => (typeof item === "string" ? item : renderOutput(item))).join("\n\n");
  }
  return JSON.stringify(value, null, 2);
}
