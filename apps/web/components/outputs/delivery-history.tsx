"use client";

import Link from "next/link";
import { useQuery } from "@tanstack/react-query";

import { Callout } from "@/components/ui/callout";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { Status } from "@/components/ui/status";
import { deliveryReason, deliveryStatus } from "@/lib/output-meta";
import { listOutputDeliveries, type OutputDelivery } from "@/lib/outputs";
import { formatDateTime, relativeTime } from "@/lib/utils";
import { errorMessage } from "@orbit/api-client";

const LIMIT = 25;

function RunLabel({ delivery }: { delivery: OutputDelivery }) {
  const task = delivery.task?.trim();
  const label = task ? (delivery.agentName ? `${delivery.agentName}: ${task}` : task) : null;
  const href = delivery.executionId
    ? `/executions/${delivery.executionId}`
    : delivery.teamRunId
      ? `/agent-teams/runs/${delivery.teamRunId}`
      : null;
  if (!href) {
    return <span className="text-muted-foreground">{label ?? "A run you cannot see"}</span>;
  }
  return (
    <Link href={href} className="font-medium text-foreground underline-offset-4 hover:underline">
      {label ?? (delivery.teamRunId ? "Team run" : "Run")}
    </Link>
  );
}

/**
 * The output's recent deliveries: whether each message arrived and, if not, why. Test
 * messages are sent directly and do not appear here.
 */
export function DeliveryHistory({ outputId }: { outputId: string }) {
  const deliveries = useQuery({
    queryKey: ["outputs", outputId, "deliveries"],
    queryFn: () => listOutputDeliveries(outputId, LIMIT),
    // Follow a delivery that is still queued, sending or waiting to retry.
    refetchInterval: (query) =>
      query.state.data?.some((item) => item.status === "pending" || item.status === "sending") ? 5_000 : 30_000,
  });

  return (
    <Card>
      <CardHeader>
        <CardTitle>Recent deliveries</CardTitle>
        <p className="text-[13px] text-muted-foreground">
          One per finished run that matches. ORBIT retries server errors and timeouts up to five times;
          other refusals are not retried. Test messages are not listed.
        </p>
      </CardHeader>
      <CardContent>
        {deliveries.isLoading ? (
          <div className="space-y-2">
            {Array.from({ length: 3 }).map((_, index) => (
              <Skeleton key={index} className="h-12" />
            ))}
          </div>
        ) : deliveries.isError ? (
          <Callout tone="danger" title="Deliveries could not be loaded">
            {errorMessage(deliveries.error)}
          </Callout>
        ) : !deliveries.data?.length ? (
          <p className="text-[13px] text-muted-foreground">
            Nothing sent yet. A message goes out when a matching run finishes; runs that finished before
            this output existed, or while it was off, are not sent.
          </p>
        ) : (
          <>
            <ul className="divide-y">
              {deliveries.data.map((delivery) => {
                const reason = deliveryReason(delivery);
                const retrying = delivery.status === "pending" && delivery.attempts > 0;
                return (
                  <li key={delivery.id} className="flex flex-col gap-1.5 py-3 first:pt-0 last:pb-0 sm:flex-row sm:items-start sm:gap-4">
                    <div className="w-28 shrink-0">
                      <Status meta={deliveryStatus(delivery)} />
                    </div>
                    <div className="min-w-0 flex-1 space-y-0.5 text-[13px]">
                      <p className="line-clamp-2">
                        <RunLabel delivery={delivery} />
                      </p>
                      {reason && delivery.status !== "skipped" ? (
                        <p className="break-words text-xs text-muted-foreground" title={delivery.error ?? undefined}>
                          {delivery.error?.trim() || reason}
                        </p>
                      ) : null}
                      {delivery.status === "skipped" && delivery.error ? (
                        <p className="text-xs text-muted-foreground">Not sent: {delivery.error}</p>
                      ) : null}
                      <p className="tabular text-xs text-muted-foreground">
                        {delivery.attempts === 1 ? "1 attempt" : `${delivery.attempts} attempts`}
                        {retrying && delivery.nextAttemptAt ? ` · next try ${relativeTime(delivery.nextAttemptAt)}` : ""}
                      </p>
                    </div>
                    <p
                      className="tabular shrink-0 text-xs text-muted-foreground sm:text-right"
                      title={formatDateTime(delivery.updatedAt)}
                    >
                      {relativeTime(delivery.updatedAt)}
                    </p>
                  </li>
                );
              })}
            </ul>
            {deliveries.data.length === LIMIT ? (
              <p className="mt-3 text-xs text-muted-foreground">Showing the latest {LIMIT}.</p>
            ) : null}
          </>
        )}
      </CardContent>
    </Card>
  );
}
