"use client";

import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Ellipsis, Megaphone, Pencil, Plus, Send, Trash2 } from "lucide-react";
import { toast } from "sonner";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Callout } from "@/components/ui/callout";
import { Card } from "@/components/ui/card";
import { CheckboxField } from "@/components/ui/checkbox";
import { useConfirm } from "@/components/ui/confirm";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { EmptyState } from "@/components/ui/empty-state";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { PageHeader } from "@/components/ui/page-header";
import { Select } from "@/components/ui/select";
import { Skeleton } from "@/components/ui/skeleton";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { useAllAgents, useAgentTeams } from "@/lib/queries";
import { errorMessage } from "@orbit/api-client";
import {
  createOutput,
  deleteOutput,
  listOutputs,
  testOutput,
  updateOutput,
  type OutputInput,
  type OutputView,
} from "@/lib/outputs";
import { relativeTime, titleCase } from "@/lib/utils";

const EVENTS = ["succeeded", "failed", "cancelled"] as const;

const KIND_LABEL: Record<OutputInput["kind"], string> = {
  slack: "Slack",
  mattermost: "Mattermost",
  webhook: "Webhook",
};

type Form = {
  name: string;
  kind: OutputInput["kind"];
  agentId: string;
  teamId: string;
  events: string[];
  webhookUrl: string;
  template: string;
  enabled: boolean;
};

const EMPTY: Form = {
  name: "",
  kind: "slack",
  agentId: "",
  teamId: "",
  events: [...EVENTS],
  webhookUrl: "",
  template: "",
  enabled: true,
};

export default function OutputsPage() {
  const queryClient = useQueryClient();
  const confirm = useConfirm();
  const outputs = useQuery({ queryKey: ["outputs"], queryFn: listOutputs, staleTime: 15_000 });
  const agents = useAllAgents();
  const teams = useAgentTeams();
  const [open, setOpen] = useState(false);
  const [editing, setEditing] = useState<OutputView | null>(null);
  const [form, setForm] = useState<Form>(EMPTY);

  const save = useMutation({
    mutationFn: async () => {
      const body: OutputInput = {
        name: form.name.trim(),
        kind: form.kind,
        agentId: form.agentId || null,
        teamId: form.teamId || null,
        events: form.events,
        webhookUrl: form.webhookUrl.trim() || undefined,
        template: form.template.trim() || null,
        enabled: form.enabled,
      };
      return editing ? updateOutput(editing.id, body) : createOutput(body);
    },
    onSuccess: () => {
      toast.success(editing ? "Output updated" : "Output created");
      setOpen(false);
      setEditing(null);
      void queryClient.invalidateQueries({ queryKey: ["outputs"] });
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  const remove = useMutation({
    mutationFn: deleteOutput,
    onSuccess: () => {
      toast.success("Output deleted");
      void queryClient.invalidateQueries({ queryKey: ["outputs"] });
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  const test = useMutation({
    mutationFn: testOutput,
    onSuccess: () => toast.success("Test notification sent"),
    onError: (error) =>
      toast.error(`Test failed. The destination could not be reached: ${errorMessage(error)}`),
  });

  // Same update call as the edit form, with every field carried over unchanged. The
  // webhook URL is omitted, which keeps the stored one.
  const toggle = useMutation({
    mutationFn: ({ output, enabled }: { output: OutputView; enabled: boolean }) =>
      updateOutput(output.id, {
        name: output.name,
        kind: output.kind,
        agentId: output.agentId ?? null,
        teamId: output.teamId ?? null,
        events: output.events,
        template: output.template ?? null,
        enabled,
      }),
    onSuccess: (_result, { enabled }) => {
      toast.success(enabled ? "Output enabled" : "Output disabled");
      void queryClient.invalidateQueries({ queryKey: ["outputs"] });
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  function openCreate() {
    setEditing(null);
    setForm(EMPTY);
    setOpen(true);
  }

  function openEdit(output: OutputView) {
    setEditing(output);
    setForm({
      name: output.name,
      kind: output.kind,
      agentId: output.agentId ?? "",
      teamId: output.teamId ?? "",
      events: output.events,
      webhookUrl: "",
      template: output.template ?? "",
      enabled: output.enabled,
    });
    setOpen(true);
  }

  async function deleteOutputWithConfirm(output: OutputView) {
    const confirmed = await confirm({
      title: `Delete ${output.name}?`,
      description: "Notifications stop and the stored webhook URL is removed. This cannot be undone.",
      confirmLabel: "Delete output",
      destructive: true,
    });
    if (confirmed) remove.mutate(output.id);
  }

  const scopeLabel = (output: OutputView) =>
    output.teamId
      ? `Team: ${teams.data?.find((team) => team.id === output.teamId)?.name ?? "Unknown team"}`
      : output.agentId
        ? `Agent: ${agents.data?.items.find((agent) => agent.id === output.agentId)?.name ?? "Unknown agent"}`
        : "All agents";

  const missingWebhook = (outputs.data ?? []).filter((output) => !output.hasWebhook);
  const webhookLabel =
    form.kind === "webhook" ? "Webhook URL" : `${KIND_LABEL[form.kind]} incoming webhook URL`;

  return (
    <div className="space-y-6">
      <PageHeader
        title="Outputs"
        description="Send a message to Slack, Mattermost or a webhook when a run finishes."
        actions={
          <Button onClick={openCreate}>
            <Plus /> New output
          </Button>
        }
      />

      {missingWebhook.length ? (
        <Callout tone="warning" title="Some outputs have no webhook URL">
          {missingWebhook.map((output) => output.name).join(", ")} cannot send until you add one.
        </Callout>
      ) : null}

      {outputs.isLoading ? (
        <Card className="overflow-hidden">
          {Array.from({ length: 3 }).map((_, index) => (
            <div key={index} className="border-b px-5 py-4 last:border-b-0">
              <Skeleton className="h-10" />
            </div>
          ))}
        </Card>
      ) : outputs.isError ? (
        <Callout tone="danger" title="Could not load outputs">
          {errorMessage(outputs.error)}
        </Callout>
      ) : !outputs.data?.length ? (
        <EmptyState
          icon={<Megaphone />}
          title="No outputs yet"
          description="Add an output to be told in Slack, Mattermost or a webhook when an agent or team run succeeds, fails or is cancelled."
          action={
            <Button variant="outline" onClick={openCreate}>
              <Plus /> New output
            </Button>
          }
        />
      ) : (
        <Card className="overflow-hidden">
          {outputs.data.map((output) => (
            <div
              key={output.id}
              className="flex flex-col gap-3 border-b px-4 py-4 last:border-b-0 sm:px-5 md:flex-row md:items-center md:gap-5"
            >
              <div className="min-w-0 flex-1 space-y-1.5">
                <div className="flex flex-wrap items-center gap-2">
                  <p className="min-w-0 truncate text-sm font-medium">{output.name}</p>
                  <Badge variant="outline">{KIND_LABEL[output.kind]}</Badge>
                  {!output.hasWebhook ? <Badge variant="warning">No webhook URL</Badge> : null}
                </div>
                <p className="text-[13px] text-muted-foreground">
                  {scopeLabel(output)}
                  <span className="text-muted-foreground/70">, updated {relativeTime(output.updatedAt)}</span>
                </p>
                <div className="flex flex-wrap items-center gap-1.5">
                  <span className="text-[13px] text-muted-foreground">Sends when a run is</span>
                  {output.events.length ? (
                    output.events.map((event) => (
                      <Badge key={event} variant="secondary">
                        {titleCase(event).toLowerCase()}
                      </Badge>
                    ))
                  ) : (
                    <span className="text-[13px] text-muted-foreground">never (no events chosen)</span>
                  )}
                </div>
              </div>
              <div className="flex shrink-0 items-center gap-2">
                <label className="mr-1 flex items-center gap-2 text-[13px] text-muted-foreground">
                  <Switch
                    checked={output.enabled}
                    disabled={toggle.isPending && toggle.variables?.output.id === output.id}
                    onCheckedChange={(enabled) => toggle.mutate({ output, enabled })}
                    aria-label={`Send notifications to ${output.name}`}
                  />
                  {output.enabled ? "On" : "Off"}
                </label>
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() => test.mutate(output.id)}
                  loading={test.isPending && test.variables === output.id}
                  disabled={!output.hasWebhook}
                  title={output.hasWebhook ? undefined : "Add a webhook URL first"}
                >
                  <Send /> Send test
                </Button>
                <Button size="sm" variant="outline" onClick={() => openEdit(output)}>
                  <Pencil /> Edit
                </Button>
                <DropdownMenu>
                  <DropdownMenuTrigger asChild>
                    <Button size="icon-sm" variant="ghost" aria-label={`More actions for ${output.name}`}>
                      <Ellipsis />
                    </Button>
                  </DropdownMenuTrigger>
                  <DropdownMenuContent align="end">
                    <DropdownMenuItem destructive onSelect={() => void deleteOutputWithConfirm(output)}>
                      <Trash2 /> Delete output
                    </DropdownMenuItem>
                  </DropdownMenuContent>
                </DropdownMenu>
              </div>
            </div>
          ))}
        </Card>
      )}

      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="max-w-2xl">
          <DialogHeader>
            <DialogTitle>{editing ? "Edit output" : "New output"}</DialogTitle>
            <DialogDescription>
              Choose where the message goes, what it covers and when it is sent.
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-4">
            <div className="grid gap-4 sm:grid-cols-2">
              <Field label="Name" htmlFor="output-name">
                <Input
                  id="output-name"
                  value={form.name}
                  placeholder="Engineering alerts"
                  onChange={(event) => setForm({ ...form, name: event.target.value })}
                />
              </Field>
              <Field label="Destination" htmlFor="output-kind">
                <Select
                  id="output-kind"
                  value={form.kind}
                  onChange={(event) =>
                    setForm({ ...form, kind: event.target.value as OutputInput["kind"] })
                  }
                >
                  <option value="slack">Slack</option>
                  <option value="mattermost">Mattermost</option>
                  <option value="webhook">Webhook</option>
                </Select>
              </Field>
            </div>

            <Field
              label={webhookLabel}
              htmlFor="output-webhook"
              hint={
                editing?.hasWebhook
                  ? "Stored encrypted and never shown again. Leave blank to keep the current URL."
                  : form.kind === "webhook"
                    ? "Stored encrypted. ORBIT sends JSON with the event, execution and text."
                    : `Stored encrypted. Create it from your ${KIND_LABEL[form.kind]} integrations.`
              }
            >
              <Input
                id="output-webhook"
                type="password"
                autoComplete="off"
                value={form.webhookUrl}
                placeholder={editing?.hasWebhook ? "Keep the current URL" : "https://…"}
                onChange={(event) => setForm({ ...form, webhookUrl: event.target.value })}
              />
            </Field>

            <Field
              label="Applies to"
              htmlFor="output-scope"
              hint="An agent sends one message per execution. A team sends one message when its whole run finishes."
            >
              <Select
                id="output-scope"
                value={
                  form.teamId ? `team:${form.teamId}` : form.agentId ? `agent:${form.agentId}` : ""
                }
                onChange={(event) => {
                  const value = event.target.value;
                  if (value.startsWith("team:")) {
                    setForm({ ...form, teamId: value.slice(5), agentId: "" });
                  } else if (value.startsWith("agent:")) {
                    setForm({ ...form, agentId: value.slice(6), teamId: "" });
                  } else {
                    setForm({ ...form, agentId: "", teamId: "" });
                  }
                }}
              >
                <option value="">All agents</option>
                <optgroup label="One agent">
                  {(agents.data?.items ?? []).map((agent) => (
                    <option key={agent.id} value={`agent:${agent.id}`}>
                      {agent.name}
                    </option>
                  ))}
                </optgroup>
                <optgroup label="One agent team">
                  {(teams.data ?? []).map((team) => (
                    <option key={team.id} value={`team:${team.id}`}>
                      {team.name}
                    </option>
                  ))}
                </optgroup>
              </Select>
            </Field>

            <fieldset className="space-y-1.5">
              <legend className="text-[13px] font-medium leading-none">Send when a run is</legend>
              <div className="grid gap-2 pt-1 sm:grid-cols-3">
                {EVENTS.map((event) => (
                  <CheckboxField
                    key={event}
                    title={titleCase(event)}
                    checked={form.events.includes(event)}
                    onChange={(changed) =>
                      setForm({
                        ...form,
                        events: changed.target.checked
                          ? [...form.events, event]
                          : form.events.filter((item) => item !== event),
                      })
                    }
                  />
                ))}
              </div>
              {form.events.length === 0 ? (
                <p className="text-[13px] text-destructive" role="alert">
                  Choose at least one event.
                </p>
              ) : null}
            </fieldset>

            <Field
              label="Note"
              optional
              htmlFor="output-template"
              hint="Added to the message after the run summary."
            >
              <Textarea
                id="output-template"
                className="min-h-20"
                value={form.template}
                onChange={(event) => setForm({ ...form, template: event.target.value })}
              />
            </Field>

            <CheckboxField
              title="Enabled"
              description="Turn off to keep the output without sending anything."
              checked={form.enabled}
              onChange={(event) => setForm({ ...form, enabled: event.target.checked })}
            />
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setOpen(false)}>
              Cancel
            </Button>
            <Button
              onClick={() => save.mutate()}
              loading={save.isPending}
              disabled={
                !form.name.trim() ||
                form.events.length === 0 ||
                // A new output needs somewhere to send to; an existing one keeps its URL.
                (!editing?.hasWebhook && !form.webhookUrl.trim())
              }
            >
              {editing ? "Save changes" : "Create output"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
