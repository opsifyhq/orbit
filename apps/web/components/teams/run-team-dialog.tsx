"use client";

import { useState } from "react";
import { useRouter } from "next/navigation";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Play } from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Callout } from "@/components/ui/callout";
import { CheckboxField } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { autonomyName } from "@/lib/presentation";
import { queryKeys, useHarnesses } from "@/lib/queries";
import { startTeamRun } from "@/lib/teams";
import type { AgentTeam } from "@/lib/types";
import { errorMessage } from "@orbit/api-client";

/** Start a run of a team. On success it opens the run page, which follows the steps live. */
export function RunTeamDialog({
  team,
  size,
  variant,
}: {
  team: AgentTeam;
  size?: "sm" | "default";
  variant?: "default" | "outline";
}) {
  const router = useRouter();
  const queryClient = useQueryClient();
  const harnesses = useHarnesses();
  const [open, setOpen] = useState(false);
  const [task, setTask] = useState("");
  const [repository, setRepository] = useState("");
  const [harness, setHarness] = useState("");
  const [autoApprove, setAutoApprove] = useState(false);
  const fixedAutonomy = team.definition.autonomy?.mode === "fixed";
  const fixedLevel = team.definition.autonomy?.level ?? 2;

  function reset(next: boolean) {
    if (next) {
      setTask(team.definition.instructions ?? "");
      setRepository(team.definition.repository ?? "");
      setHarness("");
      setAutoApprove(false);
    }
    setOpen(next);
  }

  const run = useMutation({
    mutationFn: () => startTeamRun(team.id, task.trim(), repository, autoApprove, harness || undefined),
    onSuccess: (result) => {
      toast.success("Run started");
      setOpen(false);
      void queryClient.invalidateQueries({ queryKey: queryKeys.teamRuns });
      void queryClient.invalidateQueries({ queryKey: queryKeys.executions() });
      router.push(`/agent-teams/runs/${result.run.id}`);
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  return (
    <Dialog open={open} onOpenChange={reset}>
      <DialogTrigger asChild>
        <Button size={size} variant={variant}>
          <Play /> Run
        </Button>
      </DialogTrigger>
      <DialogContent className="max-w-2xl">
        <DialogHeader>
          <DialogTitle>Run {team.name}</DialogTitle>
          <DialogDescription>
            The primary agent receives the task. Later agents receive the previous output.
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-4">
          <Field label="Task for this run" htmlFor="run-task">
            <Textarea
              id="run-task"
              className="min-h-24"
              value={task}
              onChange={(event) => setTask(event.target.value)}
              placeholder={team.definition.instructions || "Team goal"}
            />
          </Field>
          <div className="grid gap-4 sm:grid-cols-2">
            <Field
              label="Repository"
              htmlFor="run-repo"
              optional
              hint="Leave blank to use the team's repository or the connection's default."
            >
              <Input
                id="run-repo"
                className="font-mono"
                value={repository}
                onChange={(event) => setRepository(event.target.value)}
                placeholder="owner/repo"
              />
            </Field>
            <Field label="Harness" htmlFor="run-harness">
              <Select id="run-harness" value={harness} onChange={(event) => setHarness(event.target.value)}>
                <option value="">Team or agent default</option>
                {(harnesses.data ?? [])
                  .filter((item) => item.enabled && item.implementationStatus === "available")
                  .map((item) => (
                    <option key={item.kind} value={item.kind}>
                      {item.name}
                    </option>
                  ))}
              </Select>
            </Field>
          </div>
          <div className="space-y-2">
            {fixedAutonomy ? (
              <Callout tone="neutral" title={`Team autonomy: ${autonomyName(fixedLevel)} (level ${fixedLevel})`}>
                This team runs every step and native subagent at the level configured on the team.
              </Callout>
            ) : (
              <CheckboxField
                title="Auto-approve approvals for this run"
                description="Only applies when the primary agent is Level 3 or higher. It never raises the agent's autonomy."
                checked={autoApprove}
                onChange={(event) => setAutoApprove(event.target.checked)}
              />
            )}
            {!fixedAutonomy && autoApprove ? (
              <Callout tone="warning" title="Auto-approve skips human review">
                Bound writes run without approval. Capability allowlists still apply. Use it only for trusted
                repositories and tested agents.
              </Callout>
            ) : null}
          </div>
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={() => setOpen(false)}>
            Cancel
          </Button>
          <Button
            onClick={() => run.mutate()}
            loading={run.isPending}
            disabled={!task.trim() && !team.definition.instructions}
          >
            <Play /> Start run
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
