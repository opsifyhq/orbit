"use client";

import Link from "next/link";
import { useMemo, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { ArrowUpRight, Check, ShieldCheck, X } from "lucide-react";
import { toast } from "sonner";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Callout } from "@/components/ui/callout";
import { Card } from "@/components/ui/card";
import { EmptyState } from "@/components/ui/empty-state";
import { PageHeader } from "@/components/ui/page-header";
import { Section } from "@/components/ui/section";
import { Skeleton } from "@/components/ui/skeleton";
import { api } from "@/lib/api";
import { APPROVALS_PAGE_LIMIT, queryKeys, unwrap, useApprovals } from "@/lib/queries";
import { errorMessage } from "@orbit/api-client";
import { formatDateTime, relativeTime, titleCase } from "@/lib/utils";
import { capabilityLabel } from "@/lib/presentation";
import type { ApprovalWithExecution } from "@/lib/types";
import { ApprovalDetails } from "@/components/approvals/approval-details";

type ApprovalDecision = "approved" | "rejected";

export default function ApprovalsPage() {
  const pending = useApprovals("pending", { limit: APPROVALS_PAGE_LIMIT });
  // Decisions are requested on their own, so a long pending queue cannot crowd them out.
  // History only changes when someone decides; it does not need the pending list's pace.
  const decided = useApprovals("decided", { refetchInterval: 30_000, limit: 30 });
  const queryClient = useQueryClient();
  // One entry per approval being decided: each card owns its own in-flight state, so
  // deciding a second card cannot re-enable the first while its request is still out.
  const [deciding, setDeciding] = useState<Record<string, ApprovalDecision>>({});

  const history = useMemo(
    () => (decided.data ?? []).filter((item) => item.status !== "pending"),
    [decided.data],
  );
  const pendingCapped = (pending.data?.length ?? 0) >= APPROVALS_PAGE_LIMIT;

  const decide = useMutation({
    mutationFn: async ({
      approval,
      decision,
    }: {
      approval: ApprovalWithExecution;
      decision: ApprovalDecision;
    }) =>
      unwrap(
        await api.POST(
          decision === "approved"
            ? "/api/v1/approvals/{approval_id}/approve"
            : "/api/v1/approvals/{approval_id}/reject",
          {
            params: { path: { approval_id: approval.id } },
            body: {},
          },
        ),
      ),
    onMutate: ({ approval, decision }) =>
      setDeciding((current) => ({ ...current, [approval.id]: decision })),
    onSuccess: (_, variables) => {
      toast.success(variables.decision === "approved" ? "Approved" : "Rejected");
    },
    onError: (error) => toast.error(errorMessage(error)),
    // Also after a failure: "already decided" or "expired" means the card is stale.
    onSettled: (_data, _error, variables) => {
      setDeciding((current) => {
        const next = { ...current };
        delete next[variables.approval.id];
        return next;
      });
      // Every approvals list: pending, decided and per-execution.
      void queryClient.invalidateQueries({ queryKey: queryKeys.approvalLists });
      void queryClient.invalidateQueries({ queryKey: queryKeys.dashboard });
      void queryClient.invalidateQueries({
        queryKey: queryKeys.execution(variables.approval.executionId),
      });
    },
  });

  return (
    <div className="space-y-7">
      <PageHeader
        title="Approvals"
        description="When an agent wants to change something outside ORBIT, it stops and asks here. You see the exact call before it happens."
      />

      <Section title="Waiting for a decision" count={pending.data?.length}>
        {pending.isLoading ? (
          <Skeleton className="h-40 rounded-xl" />
        ) : pending.isError ? (
          <Callout tone="danger" title="Approvals could not be loaded">
            {errorMessage(pending.error)}
          </Callout>
        ) : pending.data?.length ? (
          <>
          {pendingCapped ? (
            <Callout
              tone="info"
              title={`Showing the newest ${APPROVALS_PAGE_LIMIT} requests`}
              className="mb-3"
            >
              More approvals are waiting. Older ones appear here as these are decided.
            </Callout>
          ) : null}
          <ul className="space-y-3">
            {pending.data.map((approval) => (
              <li key={approval.id}>
                <ApprovalCard
                  approval={approval}
                  pending={approval.id in deciding}
                  pendingDecision={deciding[approval.id]}
                  onApprove={() => decide.mutate({ approval, decision: "approved" })}
                  onReject={() => decide.mutate({ approval, decision: "rejected" })}
                />
              </li>
            ))}
          </ul>
          </>
        ) : (
          <EmptyState
            icon={<ShieldCheck />}
            title="Nothing is waiting for you"
            description="An agent that prepares writes for approval pauses here before it changes, posts or sends anything outside ORBIT."
          />
        )}
      </Section>

      <Section title="Recent decisions">
        {decided.isLoading ? (
          <Skeleton className="h-24 rounded-xl" />
        ) : history.length ? (
          <Card className="overflow-hidden">
            <ul>
              {history.map((approval) => (
                <li
                  key={approval.id}
                  className="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-4 gap-y-1 border-b px-4 py-3 last:border-b-0 sm:grid-cols-[6.5rem_minmax(0,1fr)_auto] sm:px-5"
                >
                  <Badge
                    className="hidden w-fit sm:inline-flex"
                    variant={
                      approval.status === "approved"
                        ? "success"
                        : approval.status === "rejected"
                          ? "destructive"
                          : "outline"
                    }
                  >
                    {titleCase(approval.status)}
                  </Badge>
                  <div className="min-w-0">
                    <p className="truncate text-sm font-medium">{approvalTitle(approval)}</p>
                    <p className="truncate text-[13px] text-muted-foreground">
                      <span className="sm:hidden">{titleCase(approval.status)} · </span>
                      {approval.agentName}, {formatDateTime(approval.createdAt)}
                      {approval.comment ? `: “${approval.comment}”` : ""}
                    </p>
                  </div>
                  <Link
                    href={`/executions/${approval.executionId}`}
                    className="inline-flex items-center gap-1 text-[13px] font-medium text-primary underline-offset-4 hover:underline"
                  >
                    Execution <ArrowUpRight className="h-3.5 w-3.5" />
                  </Link>
                </li>
              ))}
            </ul>
          </Card>
        ) : (
          <p className="text-sm text-muted-foreground">No decisions have been made yet.</p>
        )}
      </Section>

    </div>
  );
}

function ApprovalCard({
  approval,
  pending,
  pendingDecision,
  onApprove,
  onReject,
}: {
  approval: ApprovalWithExecution;
  pending: boolean;
  pendingDecision?: ApprovalDecision;
  onApprove: () => void;
  onReject: () => void;
}) {
  // Past its deadline the platform refuses the action whatever is clicked here.
  const expired = approval.expiresAt ? new Date(approval.expiresAt).getTime() <= Date.now() : false;
  return (
    <Card className="border-warning/40">
      <div className="flex flex-col gap-4 p-4 sm:p-5">
        <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
          <div className="min-w-0">
            <p className="text-[15px] font-semibold">{approvalTitle(approval)}</p>
            <p className="mt-0.5 text-[13px] text-muted-foreground">
              Asked by{" "}
              <Link
                href={`/executions/${approval.executionId}`}
                className="font-medium text-foreground underline-offset-4 hover:underline"
              >
                {approval.agentName}
              </Link>{" "}
              {relativeTime(approval.createdAt)}
              {approval.expiresAt ? (
                <>
                  .{" "}
                  {expired ? (
                    <span className="font-medium text-destructive" title={formatDateTime(approval.expiresAt)}>
                      Expired {relativeTime(approval.expiresAt)}; the action was refused
                    </span>
                  ) : (
                    <>
                      <span className="font-medium text-warning" title={formatDateTime(approval.expiresAt)}>
                        Expires {relativeTime(approval.expiresAt)}
                      </span>
                      ; after that the action is refused
                    </>
                  )}
                </>
              ) : null}
            </p>
          </div>
          <div className="flex shrink-0 gap-2">
            <Button
              variant="outline"
              className="flex-1 text-destructive hover:border-destructive/50 hover:bg-destructive/10 sm:flex-none"
              onClick={onReject}
              loading={pending && pendingDecision === "rejected"}
              disabled={pending || expired}
            >
              <X /> Reject
            </Button>
            <Button
              className="flex-1 sm:flex-none"
              onClick={onApprove}
              loading={pending && pendingDecision === "approved"}
              disabled={pending || expired}
            >
              <Check /> Approve
            </Button>
          </div>
        </div>
        <ApprovalDetails approval={approval} />
      </div>
    </Card>
  );
}

function approvalTitle(approval: ApprovalWithExecution): string {
  const action = (approval.requestedAction ?? {}) as {
    capability?: string;
    arguments?: Record<string, unknown>;
  };
  const args = action.arguments ?? {};
  switch (action.capability) {
    case "repo.commit":
      return `Commit changes${args.message ? `: ${String(args.message).slice(0, 80)}` : ""}`;
    case "repo.push":
      return `Push branch${args.branch ? ` ${args.branch}` : ""}`;
    case "pulls.create":
      return `Open pull request${args.title ? `: ${String(args.title).slice(0, 80)}` : ""}`;
    case "issues.comment":
      return `Comment on issue${args.number ? ` #${args.number}` : ""}`;
    case "issues.update":
      return `Update issue${args.number ? ` #${args.number}` : ""}`;
    case "issues.create":
      return `Create issue${args.title ? `: ${String(args.title).slice(0, 80)}` : ""}`;
    default:
      return capabilityOf(approval);
  }
}


function capabilityOf(approval: ApprovalWithExecution): string {
  const action = (approval.requestedAction ?? {}) as { capability?: string };
  return capabilityLabel(action.capability);
}
