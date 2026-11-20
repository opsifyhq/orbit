"use client";

import { useState } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { useQueryClient } from "@tanstack/react-query";
import { Ellipsis, ListTodo, Play, Plus } from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Callout } from "@/components/ui/callout";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { EmptyState } from "@/components/ui/empty-state";
import { Field } from "@/components/ui/field";
import { Skeleton } from "@/components/ui/skeleton";
import { ExecutionStatusBadge } from "@/components/ui/status-badge";
import { Textarea } from "@/components/ui/textarea";
import { queryKeys, useTeamQueue } from "@/lib/queries";
import {
  cancelTeamQueueItem,
  enqueueTeamItems,
  requeueTeamQueueItem,
  retryTeamQueueItem,
  runTeamQueueItem,
  type TeamQueueItem,
} from "@/lib/teams";
import type { AgentTeam } from "@/lib/types";
import { relativeTime } from "@/lib/utils";
import { errorMessage } from "@orbit/api-client";

/** Parse "key" or "key|repository" lines into queue items. */
function parseItems(text: string) {
  return text
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => {
      const [externalKey, repository] = line.split("|").map((value) => value.trim());
      return { externalKey, repository: repository || undefined, payload: {} };
    });
}

/** The team's work queue, shown on the team page: add items, run them, follow them up. */
export function TeamQueue({ team }: { team: AgentTeam }) {
  const router = useRouter();
  const queryClient = useQueryClient();
  const queue = useTeamQueue(team.id);
  const [text, setText] = useState("");
  const [busy, setBusy] = useState<string | null>(null);

  const items = queue.data ?? [];
  const pending = items.filter((item) => item.status === "pending");
  const source = team.definition.workSource;

  async function refresh() {
    await queryClient.invalidateQueries({ queryKey: queryKeys.teamQueue(team.id) });
    void queryClient.invalidateQueries({ queryKey: queryKeys.teamRuns });
  }

  /** Runs one queue action; resolves to whether it succeeded. The list is refreshed either way. */
  async function act(key: string, action: () => Promise<unknown>, success: string): Promise<boolean> {
    setBusy(key);
    try {
      await action();
      toast.success(success);
      return true;
    } catch (error) {
      toast.error(errorMessage(error));
      return false;
    } finally {
      // A failure can still have changed the queue (some items started before one failed).
      await refresh().catch(() => undefined);
      setBusy(null);
    }
  }

  async function add() {
    const parsed = parseItems(text);
    if (!parsed.length) return;
    const added = await act(
      "add",
      () => enqueueTeamItems(team.id, parsed),
      `${parsed.length === 1 ? "Item" : `${parsed.length} items`} added`,
    );
    // Keep what was typed when the server refused it.
    if (added) setText("");
  }

  async function runOne(item: TeamQueueItem) {
    setBusy(item.id);
    try {
      const detail = await runTeamQueueItem(team.id, item.id);
      await refresh();
      router.push(`/agent-teams/runs/${detail.run.id}`);
    } catch (error) {
      toast.error(errorMessage(error));
      setBusy(null);
    }
  }

  async function runPending() {
    await act(
      "pending",
      async () => {
        let started = 0;
        for (const item of pending) {
          try {
            await runTeamQueueItem(team.id, item.id);
            started += 1;
          } catch (error) {
            // Say how far it got: the earlier items are already running.
            throw new Error(
              `${started} of ${pending.length} started, then: ${errorMessage(error)}`,
            );
          }
        }
      },
      `${pending.length === 1 ? "1 item" : `${pending.length} items`} started`,
    );
  }

  return (
    <div className="grid gap-5 lg:grid-cols-[minmax(0,1fr)_20rem] [&>*]:min-w-0">
      <Card className="overflow-hidden">
        <CardHeader className="flex-row flex-wrap items-center justify-between gap-2 space-y-0">
          <CardTitle as="h2">
            In the queue <span className="tabular ml-1 text-sm font-normal text-muted-foreground">{items.length}</span>
          </CardTitle>
          <Button size="sm" onClick={() => void runPending()} loading={busy === "pending"} disabled={!pending.length || Boolean(busy)}>
            <Play /> Run {pending.length} pending
          </Button>
        </CardHeader>
        {queue.isLoading ? (
          <CardContent>
            <Skeleton className="h-14" />
          </CardContent>
        ) : queue.isError ? (
          <CardContent>
            <Callout tone="danger" title="The queue could not be loaded">
              {errorMessage(queue.error)}
            </Callout>
          </CardContent>
        ) : !items.length ? (
          <CardContent>
            <EmptyState
              icon={<ListTodo />}
              title="The queue is empty"
              description={
                source?.enabled
                  ? "The work source adds matching issues here as it finds them."
                  : "Add items here, or turn on a work source in the team settings to fill it automatically."
              }
            />
          </CardContent>
        ) : (
          <ul className="divide-y border-t">
            {items.map((item) => (
              <li key={item.id} className="flex flex-col gap-2 px-4 py-3 sm:flex-row sm:items-center sm:gap-3 sm:px-5">
                <div className="min-w-0 flex-1 space-y-0.5">
                  <p className="break-all font-mono text-[13px]">{item.externalKey}</p>
                  <p className="break-all text-xs text-muted-foreground">
                    {item.repository ?? "Team repository"}
                    {`, updated ${relativeTime(item.updatedAt)}`}{item.attempts > 1 ? `, attempt ${item.attempts} of ${item.maxAttempts}` : ""}
                  </p>
                  {item.error ? <p className="break-words text-[13px] text-destructive">{item.error}</p> : null}
                </div>
                <div className="flex shrink-0 items-center gap-2">
                  <ExecutionStatusBadge status={item.status} />
                  {item.teamRunId ? (
                    <Button asChild size="sm" variant="ghost">
                      <Link href={`/agent-teams/runs/${item.teamRunId}`}>Open run</Link>
                    </Button>
                  ) : null}
                  {item.status === "pending" ? (
                    <Button
                      size="sm"
                      variant="outline"
                      loading={busy === item.id}
                      disabled={Boolean(busy)}
                      onClick={() => void runOne(item)}
                    >
                      <Play /> Run
                    </Button>
                  ) : null}
                  {item.status === "failed" || item.status === "cancelled" || item.status === "running" ? (
                    <DropdownMenu>
                      <DropdownMenuTrigger asChild>
                        <Button size="icon-sm" variant="ghost" aria-label={`More actions for ${item.externalKey}`}>
                          <Ellipsis />
                        </Button>
                      </DropdownMenuTrigger>
                      <DropdownMenuContent align="end">
                        {item.status === "failed" || item.status === "cancelled" ? (
                          <>
                            <DropdownMenuItem
                              disabled={Boolean(busy)}
                              onSelect={() => void act(item.id, () => retryTeamQueueItem(team.id, item.id), "Queued for retry")}
                            >
                              Retry
                            </DropdownMenuItem>
                            <DropdownMenuItem
                              disabled={Boolean(busy)}
                              onSelect={() => void act(item.id, () => requeueTeamQueueItem(team.id, item.id), "Requeued")}
                            >
                              Requeue
                            </DropdownMenuItem>
                          </>
                        ) : null}
                        {item.status === "running" ? (
                          <DropdownMenuItem
                            destructive
                            disabled={Boolean(busy)}
                            onSelect={() => void act(item.id, () => cancelTeamQueueItem(team.id, item.id), "Item cancelled")}
                          >
                            Cancel item
                          </DropdownMenuItem>
                        ) : null}
                      </DropdownMenuContent>
                    </DropdownMenu>
                  ) : null}
                </div>
              </li>
            ))}
          </ul>
        )}
      </Card>

      <Card>
        <CardHeader>
          <CardTitle as="h2">Add work</CardTitle>
        </CardHeader>
        <CardContent className="space-y-3">
          <Field label="Items" htmlFor="queue-items" hint="One per line: a key, or key|repository. Items with the same key are added once.">
            <Textarea
              id="queue-items"
              className="min-h-28 font-mono text-base md:text-xs"
              spellCheck={false}
              value={text}
              onChange={(event) => setText(event.target.value)}
              placeholder={"opsifyhq/orbit#6|opsifyhq/orbit\nopsifyhq/orbit#7"}
            />
          </Field>
          <Button
            size="sm"
            variant="outline"
            className="w-full"
            onClick={() => void add()}
            loading={busy === "add"}
            disabled={!text.trim() || Boolean(busy)}
          >
            <Plus /> Add to queue
          </Button>
        </CardContent>
      </Card>
    </div>
  );
}
