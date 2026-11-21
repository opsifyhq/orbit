"use client";

import Link from "next/link";
import { useParams, useRouter } from "next/navigation";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Ellipsis, Pause, Pencil, Play, Send, Trash2 } from "lucide-react";
import { toast } from "sonner";

import { DeliveryHistory } from "@/components/outputs/delivery-history";
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
import { Status } from "@/components/ui/status";
import {
  OUTPUT_STATUS,
  deliveryReason,
  eventsLabel,
  kindLabel,
  outputInput,
  outputProblem,
  outputState,
} from "@/lib/output-meta";
import { deleteOutput, testOutput, updateOutput } from "@/lib/outputs";
import { useAgentTeams, useAllAgents, useOutputs } from "@/lib/queries";
import { formatDateTime, relativeTime } from "@/lib/utils";
import { errorMessage } from "@orbit/api-client";

export default function OutputPage() {
  const params = useParams<{ id: string }>();
  const outputId = params.id;
  const router = useRouter();
  const queryClient = useQueryClient();
  const confirm = useConfirm();
  const outputs = useOutputs();
  const agents = useAllAgents();
  const teams = useAgentTeams();
  const output = outputs.data?.find((item) => item.id === outputId);

  const test = useMutation({
    mutationFn: () => testOutput(outputId),
    onSuccess: () => toast.success("Test message sent"),
    onError: (error) => toast.error(`Test failed: ${errorMessage(error)}`),
  });
  const toggle = useMutation({
    mutationFn: (enabled: boolean) => updateOutput(outputId, outputInput(output!, { enabled })),
    onSuccess: (_result, enabled) => {
      toast.success(enabled ? "Output turned on" : "Output turned off");
      void queryClient.invalidateQueries({ queryKey: ["outputs"] });
    },
    onError: (error) => toast.error(errorMessage(error)),
  });
  const remove = useMutation({
    mutationFn: () => deleteOutput(outputId),
    onSuccess: async () => {
      toast.success("Output deleted");
      await queryClient.invalidateQueries({ queryKey: ["outputs"] });
      router.push("/outputs");
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  if (outputs.isLoading) {
    return (
      <div className="space-y-4">
        <Skeleton className="h-10 w-64" />
        <Skeleton className="h-96 rounded-xl" />
      </div>
    );
  }
  if (!output) {
    return (
      <EmptyState
        title="Output not found"
        description={outputs.error ? errorMessage(outputs.error) : "It may have been deleted, or you cannot see it."}
        action={
          <Button asChild>
            <Link href="/outputs">Back to outputs</Link>
          </Button>
        }
      />
    );
  }

  const data = output;
  const problem = outputProblem(data);
  const needsCredential = problem === "No webhook URL" || problem === "No bot token";
  const lastReason = data.lastDelivery ? deliveryReason(data.lastDelivery) : null;
  const agent = data.agentId ? agents.data?.items.find((item) => item.id === data.agentId) : undefined;
  const team = data.teamId ? teams.data?.find((item) => item.id === data.teamId) : undefined;
  const appliesTo = data.teamId ? (
    team ? (
      <Link href={`/agent-teams/${team.id}`} className="font-medium text-primary underline-offset-4 hover:underline">
        {team.name}
      </Link>
    ) : (
      "An agent team you cannot see"
    )
  ) : data.agentId ? (
    agent ? (
      <Link href={`/agents/${agent.id}`} className="font-medium text-primary underline-offset-4 hover:underline">
        {agent.name}
      </Link>
    ) : (
      "An agent you cannot see"
    )
  ) : (
    "Every agent"
  );

  async function confirmDelete() {
    const ok = await confirm({
      title: `Delete ${data.name}?`,
      description: "Messages stop and the stored credential is removed. This cannot be undone.",
      confirmLabel: "Delete output",
      destructive: true,
    });
    if (ok) remove.mutate();
  }

  return (
    <div className="space-y-6">
      <PageHeader
        back={{ href: "/outputs", label: "Outputs" }}
        title={data.name}
        description={`${kindLabel(data.kind)} · ${data.teamId ? "one message per team run" : "one message per run"}`}
        meta={
          <>
            <Status meta={OUTPUT_STATUS[outputState(data)]} />
            <span className="text-[13px] text-muted-foreground">Updated {relativeTime(data.updatedAt)}</span>
          </>
        }
        actions={
          <>
            <Button
              variant="outline"
              onClick={() => test.mutate()}
              loading={test.isPending}
              disabled={!data.hasWebhook}
              title={data.hasWebhook ? undefined : "Add a credential first"}
            >
              <Send /> Send test
            </Button>
            <Button asChild variant="outline">
              <Link href={`/outputs/${data.id}/edit`}>
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
                <DropdownMenuItem onSelect={() => toggle.mutate(!data.enabled)} disabled={toggle.isPending}>
                  {data.enabled ? (
                    <>
                      <Pause /> Turn off
                    </>
                  ) : (
                    <>
                      <Play /> Turn on
                    </>
                  )}
                </DropdownMenuItem>
                <DropdownMenuSeparator />
                <DropdownMenuItem destructive onSelect={() => void confirmDelete()}>
                  <Trash2 /> Delete output
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </>
        }
      />

      {problem ? (
        <Callout
          tone="warning"
          title={
            needsCredential
              ? `This output has no ${data.kind === "telegram" ? "bot token" : "webhook URL"}`
              : "No events are chosen"
          }
          action={
            <Button asChild size="sm" variant="outline">
              <Link href={`/outputs/${data.id}/edit#${needsCredential ? "destination" : "trigger"}`}>Fix</Link>
            </Button>
          }
        >
          It cannot send anything until this is fixed.
        </Callout>
      ) : outputState(data) === "failing" && data.lastDelivery ? (
        <Callout
          tone="danger"
          title={`The last message did not arrive${lastReason ? ` (${lastReason})` : ""}`}
          action={
            <Button asChild size="sm" variant="outline">
              <Link href={`/outputs/${data.id}/edit#destination`}>Check the destination</Link>
            </Button>
          }
        >
          {data.lastDelivery.status === "dead"
            ? "ORBIT retried five times and gave up."
            : "The destination refused it, so ORBIT did not retry."}{" "}
          Fix the destination credential or the receiving end, then use Send test.
        </Callout>
      ) : !data.enabled ? (
        <Callout
          tone="info"
          title="This output is off"
          action={
            <Button size="sm" variant="outline" onClick={() => toggle.mutate(true)} loading={toggle.isPending}>
              Turn on
            </Button>
          }
        >
          Runs that finish while it is off are not sent later.
        </Callout>
      ) : null}

      <div className="grid gap-6 lg:grid-cols-[minmax(0,1fr)_20rem] lg:items-start [&>*]:min-w-0">
        <div className="space-y-6">
          <Card>
            <CardHeader>
              <CardTitle>What it sends</CardTitle>
              <p className="text-[13px] text-muted-foreground">
                ORBIT writes a summary of the run: its task and up to 3,000 characters of its output or error.
              </p>
            </CardHeader>
            <CardContent className="space-y-4">
              <DescriptionList
                items={[
                  { label: "Applies to", value: appliesTo },
                  { label: "Sends when a run", value: eventsLabel(data.events) },
                ]}
              />
              <div className="space-y-1.5">
                <p className="text-[13px] font-medium">Note under the summary</p>
                {data.template ? (
                  <p className="whitespace-pre-wrap rounded-lg border bg-subtle p-3 text-[13px]">{data.template}</p>
                ) : (
                  <p className="text-[13px] text-muted-foreground">
                    None.{" "}
                    <Link href={`/outputs/${data.id}/edit#message`} className="font-medium text-primary underline-offset-4 hover:underline">
                      Add a note
                    </Link>
                  </p>
                )}
              </div>
            </CardContent>
          </Card>
          <DeliveryHistory outputId={data.id} />
        </div>

        <Card>
          <CardHeader>
            <CardTitle>Details</CardTitle>
          </CardHeader>
          <CardContent>
            <DescriptionList
              items={[
                { label: "Destination", value: kindLabel(data.kind) },
                {
                  label: data.kind === "telegram" ? "Bot token" : "Webhook URL",
                  value: data.hasWebhook ? "Stored, encrypted" : "Not set",
                },
                ...(data.kind === "telegram"
                  ? [
                      { label: "Chat id", value: data.chatId ?? "Not set" },
                      ...(data.messageThreadId ? [{ label: "Topic id", value: data.messageThreadId }] : []),
                    ]
                  : []),
                { label: "Created", value: formatDateTime(data.createdAt) },
                { label: "Updated", value: formatDateTime(data.updatedAt) },
              ]}
            />
          </CardContent>
        </Card>
      </div>
    </div>
  );
}
