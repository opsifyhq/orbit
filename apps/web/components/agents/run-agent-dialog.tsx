"use client";

import { useRouter } from "next/navigation";
import { useState } from "react";
import { Play } from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Disclosure } from "@/components/ui/disclosure";
import { Select } from "@/components/ui/select";
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
import { Textarea } from "@/components/ui/textarea";
import { api } from "@/lib/api";
import { randomId } from "@/lib/browser";
import { unwrap, useDefaultHarness, useHarnesses } from "@/lib/queries";
import { errorMessage } from "@orbit/api-client";

export function RunAgentDialog({
  agentId,
  agentName,
  disabled,
  defaultTask,
  agentHarness,
}: {
  agentId: string;
  agentName: string;
  disabled?: boolean;
  defaultTask?: string;
  /** The harness stored on the agent, if it has one (older agents do). */
  agentHarness?: string | null;
}) {
  const router = useRouter();
  const [open, setOpen] = useState(false);
  const [task, setTask] = useState(
    defaultTask?.trim() || "Follow your configured instructions.",
  );
  const [payloadText, setPayloadText] = useState("{}");
  const [idempotencyKey, setIdempotencyKey] = useState(() => randomId());
  const [submitting, setSubmitting] = useState(false);
  const [harness, setHarness] = useState("");
  const harnesses = useHarnesses();
  const { defaultHarness, known: defaultKnown } = useDefaultHarness();
  const harnessName = (kind: string) =>
    harnesses.data?.find((item) => item.kind === kind)?.name ?? kind;
  // Harness is chosen per run. Without one on the agent or an organization
  // default, the run cannot start, so the choice is required up front.
  const fallbackHarness = agentHarness || defaultHarness;
  const mustChooseHarness = defaultKnown && !fallbackHarness && !harness;

  async function run() {
    if (!task.trim()) {
      toast.error("Enter a task for the agent");
      return;
    }
    let payload: unknown = null;
    try {
      payload = payloadText.trim() ? JSON.parse(payloadText) : null;
    } catch {
      toast.error("Payload must be valid JSON");
      return;
    }
    setSubmitting(true);
    try {
      const execution = unwrap(
        await api.POST("/api/v1/agents/{agent_id}/executions", {
          params: { path: { agent_id: agentId } },
          body: {
            task,
            payload,
            idempotencyKey,
            harnessOverride: harness || undefined,
          },
          headers: { "idempotency-key": idempotencyKey },
        }),
      );
      toast.success("Execution queued");
      setOpen(false);
      router.push(`/executions/${execution.id}`);
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        if (next) setIdempotencyKey(randomId());
      }}
    >
      <DialogTrigger asChild>
        <Button disabled={disabled}>
          <Play /> Run agent
        </Button>
      </DialogTrigger>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Run {agentName}</DialogTitle>
          <DialogDescription>
            The run is queued and picked up by a worker. Your task is sent alongside the
            agent&apos;s system instructions, not in place of them.
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-4">
          <Field label="Task" htmlFor="run-task" hint="What the agent should do this time.">
            <Textarea
              id="run-task"
              className="min-h-[130px]"
              value={task}
              onChange={(event) => setTask(event.target.value)}
            />
          </Field>
          <Field
            label="Harness"
            htmlFor="run-harness"
            hint={
              fallbackHarness
                ? "The runtime that executes this run. Changing it here does not change the agent."
                : "This agent has no harness of its own and no organization default is set, so pick one for this run. A default can be set in Settings."
            }
          >
            <Select
              id="run-harness"
              value={harness}
              onChange={(event) => setHarness(event.target.value)}
              aria-invalid={mustChooseHarness || undefined}
            >
              <option value="">
                {agentHarness
                  ? `Agent's harness (${harnessName(agentHarness)})`
                  : defaultHarness
                    ? `Organization default (${harnessName(defaultHarness)})`
                    : defaultKnown
                      ? "Choose a harness"
                      : "Default harness"}
              </option>
              {(harnesses.data ?? [])
                .filter((item) => item.enabled && item.implementationStatus === "available")
                .map((item) => (
                  <option key={item.kind} value={item.kind}>
                    {item.name}
                  </option>
                ))}
            </Select>
          </Field>
          <Disclosure title="Advanced" summary="Payload and retry key">
              <Field
                label="Payload"
                htmlFor="run-payload"
                optional
                hint={
                  <>
                    JSON passed to connectors. X reads use <code className="font-mono">payload.query</code>,
                    then the connection&apos;s <code className="font-mono">config.query</code>, then the
                    task text.
                  </>
                }
              >
                <Textarea
                  id="run-payload"
                  className="min-h-[80px] font-mono text-base md:text-xs"
                  value={payloadText}
                  onChange={(event) => setPayloadText(event.target.value)}
                  placeholder='{"query":"#ORBIT"}'
                />
              </Field>
              <p className="text-[13px] text-muted-foreground">
                Retries of this run reuse key <span className="font-mono">{idempotencyKey.slice(0, 8)}</span>,
                so it is never queued twice.
              </p>
          </Disclosure>
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={() => setOpen(false)}>
            Cancel
          </Button>
          <Button onClick={run} loading={submitting} disabled={mustChooseHarness}>
            <Play /> Run agent
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
