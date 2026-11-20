"use client";

export const dynamic = "force-dynamic";

import Link from "next/link";
import { useRouter } from "next/navigation";
import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Bot, CalendarClock, Play, Plug, Server, Sparkles, Workflow, Zap } from "lucide-react";
import { toast } from "sonner";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Callout } from "@/components/ui/callout";
import { Card } from "@/components/ui/card";
import { CheckboxField } from "@/components/ui/checkbox";
import { ChoiceCard } from "@/components/ui/choice-card";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { PageHeader } from "@/components/ui/page-header";
import { Segmented } from "@/components/ui/segmented";
import { ImplementationBadge } from "@/components/ui/status-badge";
import { StatusGlyph } from "@/components/ui/status";
import { Textarea } from "@/components/ui/textarea";
import {
  queryKeys,
  unwrap,
  useAgents,
  useAgentTeams,
  useConnectors,
  useDefaultHarness,
  useHarnesses,
  useOutputs,
  useWorkerProfiles,
  useWorkers,
} from "@/lib/queries";
import { api } from "@/lib/api";
import { toAgentDefinitionBody } from "@/lib/agent-body";
import { errorMessage } from "@orbit/api-client";
import { cn, titleCase } from "@/lib/utils";
import { brandName, autonomyName, cronDescription } from "@/lib/presentation";
import { defaultWorkerImage } from "@/lib/worker-images";

type WhoType = "agent" | "team";

export default function WorkPage() {
  const router = useRouter();
  const [when, setWhen] = useState<"manual" | "schedule" | "event" | "connector">("manual");
  const [cron, setCron] = useState("*/30 * * * *");
  const [subject, setSubject] = useState("connector.telegram.message");
  const [connectorSlug, setConnectorSlug] = useState("");
  const [where, setWhere] = useState<"auto" | "local" | "docker" | "kubernetes">("auto");
  const [harness, setHarness] = useState("");
  const [whoType, setWhoType] = useState<WhoType>("agent");
  const [who, setWho] = useState("");
  const [name, setName] = useState("");
  const [task, setTask] = useState("");
  const [repository, setRepository] = useState("");
  const [autoApprove, setAutoApprove] = useState(false);
  const [longRunning, setLongRunning] = useState(false);

  // Drafts can be run by hand, so they are offered too (marked as drafts).
  const agents = useAgents({ limit: 100 });
  const teams = useAgentTeams();
  const harnesses = useHarnesses();
  const workerProfiles = useWorkerProfiles();
  const workers = useWorkers();
  const connectors = useConnectors();
  const outputs = useOutputs();

  const queryClient = useQueryClient();
  const [creating, setCreating] = useState(false);

  /**
   * "On a schedule" adds a schedule trigger (cron + this task) to the agent by saving
   * a new version, the same thing Agent Studio does, so it shows up and can be
   * edited or removed there.
   */
  async function createSchedule() {
    const detail = unwrap(
      await api.GET("/api/v1/agents/{agent_id}", { params: { path: { agent_id: who } } }),
    );
    const spec = detail.spec as unknown as { triggers?: unknown[] };
    const definition = {
      apiVersion: "orbit.io/v1",
      kind: "Agent",
      metadata: {
        name: detail.name,
        description: detail.description,
        labels: detail.labels ?? {},
      },
      spec: {
        ...spec,
        triggers: [...(spec.triggers ?? []), { type: "schedule", cron: cron.trim(), task: task.trim() }],
      },
    };
    unwrap(
      await api.PATCH("/api/v1/agents/{agent_id}", {
        params: { path: { agent_id: who } },
        body: { definition: toAgentDefinitionBody(definition as never) },
      }),
    );
    void queryClient.invalidateQueries({ queryKey: queryKeys.agent(who) });
    void queryClient.invalidateQueries({ queryKey: queryKeys.agents() });
    toast.success("Schedule created");
    router.push(`/agents/${who}`);
  }

  async function createWork() {
    if (!who || !task.trim()) return;
    setCreating(true);
    try {
      if (scheduled) {
        await createSchedule();
        return;
      }
      if (whoType === "agent") {
        const result = unwrap(
          await api.POST("/api/v1/agents/{agent_id}/executions", {
            params: { path: { agent_id: who } },
            body: { task: task.trim(), payload: repository.trim() ? { repository: repository.trim() } : {}, harnessOverride: harness || undefined, name: name.trim() || undefined },
          }),
        );
        const effectiveHarness = harness || selectedAgent?.harness || defaultHarness || "mock";
        const target = where === "kubernetes" ? "kubernetes" : where === "docker" ? "docker" : "local";

        // A worker for this harness is already online: it will pick the work up, and
        // starting another would only add an error about the launcher.
        const workerOnline = (workers.data ?? []).some(
          (worker) => worker.live && worker.harnesses.includes(effectiveHarness),
        );
        if (workerOnline) {
          router.push(`/executions/${result.id}`);
          return;
        }

        // Reuse an enabled profile for this harness that covers this agent; a disabled
        // or out-of-scope profile would accept the work and then never run it.
        let profileId = (workerProfiles.data ?? []).find(
          (p) =>
            p.harnessKind === effectiveHarness &&
            p.enabled &&
            (p.agentScope !== "selected" || p.agentIds.includes(who)),
        )?.id;
        if (!profileId) {
          try {
            const created = unwrap(
              await api.POST("/api/v1/worker-profiles", {
                body: {
                  name: `Auto: ${effectiveHarness}`,
                  harnessKind: effectiveHarness,
                  target,
                  executionDriver: "process",
                  image: defaultWorkerImage(effectiveHarness),
                  imageTag: "latest",
                  agentIds: [who],
                  skillSlugs: [],
                  env: {},
                  resources: {},
                },
              }),
            );
            profileId = created.id;
            unwrap(
              await api.POST("/api/v1/worker-profiles/{profile_id}/enable", {
                params: { path: { profile_id: created.id } },
              }),
            );
            toast.success("Worker profile created");
          } catch (error) {
            toast.error(errorMessage(error));
          }
        }

        if (profileId) {
          try {
            unwrap(
              await api.POST("/api/v1/worker-profiles/{profile_id}/launch", {
                params: { path: { profile_id: profileId } },
              }),
            );
          } catch {
            toast.warning(
              `No ${brandName(effectiveHarness)} worker is online and ORBIT could not start one. The work is queued until a worker connects.`,
            );
          }
        }
        router.push(`/executions/${result.id}`);
      } else {
        const { startTeamRun } = await import("@/lib/teams");
        const result = await startTeamRun(who, task.trim(), repository.trim() || undefined, false, harness || undefined);
        toast.success("Team run started");
        router.push(`/agent-teams/runs/${result.run.id}`);
      }
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      setCreating(false);
    }
  }

  const { defaultHarness, known: defaultHarnessKnown } = useDefaultHarness();
  const selectedAgent = agents.data?.items.find((a) => a.id === who);
  const selectedTeam = teams.data?.find((t) => t.id === who);
  const visibleOutputs = (outputs.data ?? []).filter((output) => {
    if (!output.enabled) return false;
    if (whoType === "team") return output.teamId === who;
    return !output.teamId && (!output.agentId || output.agentId === who);
  });
  const harnessProfiles = (workerProfiles.data ?? []).filter((p) => p.harnessKind === harness);
  const liveForHarness = (workers.data ?? []).filter(
    (worker) => worker.live && worker.harnesses.includes(harness),
  ).length;
  const agentOptions = (agents.data?.items ?? []).filter(
    (agent) =>
      (agent.status === "active" || agent.status === "draft") &&
      (!harness || !agent.harness || agent.harness === harness),
  );
  // Harness is chosen per run: with none on the agent and no organization default,
  // "Agent default" cannot resolve and the API would refuse the execution.
  const needsHarness =
    whoType === "agent" &&
    Boolean(selectedAgent) &&
    !harness &&
    !selectedAgent?.harness &&
    defaultHarnessKnown &&
    !defaultHarness;
  const scheduled = when === "schedule" && whoType === "agent";
  const cronValid = cron.trim().split(/\s+/).length === 5;
  const scheduleHarness = selectedAgent?.harness || defaultHarness;
  const missing = scheduled && !cronValid
    ? "Enter a cron expression with five fields"
    : scheduled && selectedAgent && defaultHarnessKnown && !scheduleHarness
      ? "Set a default harness in Settings first: a schedule runs on the agent's harness or the default"
      : !who
    ? `Choose the ${whoType === "agent" ? "agent" : "team"} that does the work`
    : needsHarness
      ? `Choose a harness under Where: ${selectedAgent?.name} has none and no default is set`
      : !task.trim()
        ? "Describe the task"
        : null;

  const whenSummary =
    when === "manual"
      ? "Now, started by you"
      : when === "schedule"
        ? cronDescription(cron)
        : when === "event"
          ? `On event ${subject || "(no subject)"}`
          : `From connector ${connectorSlug || "(none chosen)"}`;

  return (
    <div className="space-y-6">
      <PageHeader
        title="Start work"
        description="Five questions define a piece of work: when it starts, where it runs, who does it, what they do, and what happens after."
      />

      <div className="grid gap-6 lg:grid-cols-[minmax(0,1fr)_20rem] lg:items-start">
        <div className="space-y-4">
          <Step number={1} title="When" question="When should this work start?">
            <div role="radiogroup" aria-label="Trigger type" className="grid gap-2 sm:grid-cols-2 xl:grid-cols-4">
              {([
                { value: "manual", label: "Now", desc: "Start as soon as you create it", icon: <Play /> },
                { value: "schedule", label: "On a schedule", desc: "Repeat it, for example every morning", icon: <CalendarClock /> },
                { value: "event", label: "On an event", desc: "Not available yet", icon: <Zap /> },
                { value: "connector", label: "From a connector", desc: "Not available yet", icon: <Plug /> },
              ] as const).map((opt) => (
                <ChoiceCard
                  key={opt.value}
                  selected={when === opt.value}
                  onSelect={() => setWhen(opt.value)}
                  // Events and connector messages cannot start work yet; a schedule
                  // belongs to one agent, so it is not offered for a team.
                  disabled={
                    opt.value === "event" ||
                    opt.value === "connector" ||
                    (opt.value === "schedule" && whoType === "team")
                  }
                  title={opt.label}
                  description={opt.desc}
                  icon={opt.icon}
                />
              ))}
            </div>
            <p className="text-[13px] text-muted-foreground">
              Starting on an event or a connector message is not available yet.
              {whoType === "team" ? " Schedules are set per agent, so they are not offered for a team." : ""}
            </p>
            {when === "schedule" ? (
              <Field
                label="Cron expression"
                htmlFor="work-cron"
                hint={`UTC, five fields. Reads as: ${cronDescription(cron)}.`}
              >
                <Input
                  id="work-cron"
                  className="font-mono sm:max-w-xs"
                  value={cron}
                  onChange={(e) => setCron(e.target.value)}
                  placeholder="*/30 * * * *"
                />
              </Field>
            ) : null}
            {scheduled ? (
              <Callout
                tone={selectedAgent?.status === "draft" ? "warning" : "info"}
                title={
                  selectedAgent
                    ? `This adds a schedule to ${selectedAgent.name}`
                    : "This adds a schedule to the agent you choose"
                }
              >
                The agent gets the task below every time the schedule fires, until you remove the
                schedule on its page. It runs on the agent&apos;s own harness or the organization
                default, wherever a matching worker is free.
                {selectedAgent?.status === "draft"
                  ? " This agent is still a draft: schedules only fire once it is activated."
                  : ""}
              </Callout>
            ) : null}
            {when === "event" ? (
              <Field label="Event subject" htmlFor="work-subject">
                <Input
                  id="work-subject"
                  className="font-mono sm:max-w-md"
                  value={subject}
                  onChange={(e) => setSubject(e.target.value)}
                  placeholder="connector.telegram.message"
                />
              </Field>
            ) : null}
            {when === "connector" ? (
              <div role="radiogroup" aria-label="Connector" className="grid gap-2 sm:grid-cols-2">
                {(connectors.data ?? []).map((c) => (
                  <ChoiceCard
                    key={c.slug}
                    selected={connectorSlug === c.slug}
                    onSelect={() => setConnectorSlug(c.slug)}
                    title={c.name}
                    description={`${Array.isArray(c.capabilities) ? c.capabilities.length : 0} capabilities`}
                    icon={<Plug />}
                  />
                ))}
              </div>
            ) : null}
          </Step>

          <Step number={2} title="Where" question="Where should it run?">
            <div role="radiogroup" aria-label="Placement" className="grid gap-2 sm:grid-cols-2 xl:grid-cols-4">
              {([
                { value: "auto", label: "Anywhere", desc: "Any worker that matches", icon: <Sparkles /> },
                { value: "local", label: "Local", desc: "A worker on this host", icon: <Server /> },
                { value: "docker", label: "Docker", desc: "A Docker worker", icon: <Server /> },
                { value: "kubernetes", label: "Kubernetes", desc: "A Kubernetes worker", icon: <Server /> },
              ] as const).map((opt) => (
                <ChoiceCard
                  key={opt.value}
                  selected={where === opt.value}
                  onSelect={() => setWhere(opt.value)}
                  title={opt.label}
                  description={opt.desc}
                  icon={opt.icon}
                />
              ))}
            </div>
            <div className="space-y-1.5">
              <p className="text-[13px] font-medium">Harness</p>
              <div role="radiogroup" aria-label="Harness" className="grid gap-2 sm:grid-cols-2">
                <ChoiceCard
                  selected={harness === ""}
                  onSelect={() => setHarness("")}
                  title="Agent default"
                  description={
                    defaultHarness
                      ? `The agent's own harness, or the organization default (${titleCase(defaultHarness)})`
                      : "The harness set on the agent, if it has one"
                  }
                />
                {(harnesses.data ?? []).map((h) => (
                  <ChoiceCard
                    key={h.kind}
                    selected={harness === h.kind}
                    onSelect={() => setHarness(h.kind)}
                    disabled={h.implementationStatus !== "available"}
                    title={h.name}
                    trailing={<ImplementationBadge status={h.implementationStatus} />}
                  />
                ))}
              </div>
            </div>
            {harness ? (
              liveForHarness ? (
                <p className="flex items-center gap-2 text-[13px] text-muted-foreground">
                  <StatusGlyph shape="solid" tone="success" />
                  {liveForHarness === 1 ? (/^[aeiou]/i.test(brandName(harness)) ? "An" : "A") : liveForHarness} {brandName(harness)} worker{liveForHarness === 1 ? " is" : "s are"} online and will pick this up.
                </p>
              ) : harnessProfiles.length ? (
                <div className="space-y-1.5">
                  <p className="text-[13px] font-medium">Worker profiles for {brandName(harness)}</p>
                  <ul className="divide-y rounded-lg border bg-card">
                    {harnessProfiles.map((p) => (
                      <li key={p.id} className={cn("flex items-center justify-between gap-3 px-3 py-2 text-sm", !p.enabled && "text-muted-foreground")}>
                        <span className="truncate font-medium">{p.name}</span>
                        <span className="shrink-0 text-[13px] text-muted-foreground">
                          {p.enabled ? "Enabled" : "Disabled"}, {p.target}, {p.agentIds.length} agent{p.agentIds.length === 1 ? "" : "s"}
                        </span>
                      </li>
                    ))}
                  </ul>
                </div>
              ) : (
                <Callout
                  tone="warning"
                  title={`No worker profile for ${brandName(harness)}`}
                  action={
                    <Button asChild size="sm" variant="outline">
                      <Link href={`/workers?create=1&harness=${harness}&placement=${where}`}>Create profile</Link>
                    </Button>
                  }
                >
                  A worker profile sets the image, credentials and resources for this harness. One is
                  created for you when you start the work, or you can set it up yourself first.
                </Callout>
              )
            ) : null}
          </Step>

          <Step number={3} title="Who" question="Who does the work?">
            <Segmented
              label="Executor type"
              value={whoType}
              onChange={(value) => {
                setWhoType(value);
                setWho("");
                if (value === "team" && when === "schedule") setWhen("manual");
              }}
              options={[
                { value: "agent", label: <><Bot className="h-3.5 w-3.5" /> One agent</> },
                { value: "team", label: <><Workflow className="h-3.5 w-3.5" /> An agent team</> },
              ]}
            />
            {whoType === "agent" ? (
              agentOptions.length ? (
                <div role="radiogroup" aria-label="Agent" className="grid gap-2 sm:grid-cols-2">
                  {agentOptions.map((agent) => (
                    <ChoiceCard
                      key={agent.id}
                      selected={who === agent.id}
                      onSelect={() => setWho(agent.id)}
                      title={agent.name}
                      description={`${agent.harness ? brandName(agent.harness) : "Any harness"}, ${autonomyName(agent.autonomyLevel).toLowerCase()}`}
                      trailing={agent.status === "draft" ? <Badge variant="secondary">Draft</Badge> : undefined}
                    />
                  ))}
                </div>
              ) : (
                <p className="text-sm text-muted-foreground">
                  {harness && agents.data?.items.length
                    ? `No active agent uses the ${brandName(harness)} harness. Pick another harness above, or `
                    : "There are no agents to run yet. "}
                  <Link href="/agents/new" className="font-medium text-primary underline-offset-4 hover:underline">
                    {harness && agents.data?.items.length ? "create one" : "Create an agent"}
                  </Link>
                  .
                </p>
              )
            ) : (teams.data ?? []).length ? (
              <div role="radiogroup" aria-label="Agent team" className="grid gap-2 sm:grid-cols-2">
                {(teams.data ?? []).map((team) => (
                  <ChoiceCard
                    key={team.id}
                    selected={who === team.id}
                    onSelect={() => setWho(team.id)}
                    title={team.name}
                    description={`${titleCase(team.definition.mode ?? "pipeline")}, ${team.definition.steps?.length ?? 0} agents`}
                  />
                ))}
              </div>
            ) : (
              <p className="text-sm text-muted-foreground">
                There are no agent teams yet.{" "}
                <Link href="/agent-teams" className="font-medium text-primary underline-offset-4 hover:underline">
                  Create a team
                </Link>
                .
              </p>
            )}
          </Step>

          <Step number={4} title="What" question="What should be done?">
            <Field label="Task" htmlFor="work-task">
              <Textarea
                id="work-task"
                className="min-h-[120px]"
                value={task}
                onChange={(e) => setTask(e.target.value)}
                placeholder="Describe the outcome you want, as you would to a colleague."
              />
            </Field>
            <div className="grid gap-4 sm:grid-cols-2">
              <Field label="Name" optional htmlFor="work-name" hint="Shown in the executions list instead of the agent name.">
                <Input id="work-name" value={name} onChange={(e) => setName(e.target.value)} placeholder="Evaluate article X" />
              </Field>
              <Field label="Repository" optional htmlFor="work-repo" hint="Checked out into the workspace when the agent has a GitHub or GitLab connector that allows it.">
                <Input id="work-repo" className="font-mono" value={repository} onChange={(e) => setRepository(e.target.value)} placeholder="owner/repo" />
              </Field>
            </div>
            {repository.trim() && whoType === "agent" && selectedAgent && selectedAgent.connectorCount === 0 ? (
              <Callout tone="warning" title="This repository will not be checked out">
                {selectedAgent.name} has no connector, so it starts in an empty workspace and cannot
                reach {repository.trim()}. Add a GitHub or GitLab connector with “Check out repository”
                in Agent Studio first.
              </Callout>
            ) : null}
          </Step>

          <Step number={5} title="Then" question="What happens after?">
            <div className="grid gap-2 sm:grid-cols-2">
              <CheckboxField
                checked={autoApprove}
                onChange={(e) => setAutoApprove(e.target.checked)}
                disabled
                title="Auto-approve writes"
                description="Not available from here yet. Whether writes need approval follows the agent's autonomy level."
              />
              <CheckboxField
                checked={longRunning}
                onChange={(e) => setLongRunning(e.target.checked)}
                disabled
                title="Keep the agent running"
                description="Not available from here yet. Each piece of work runs once and then stops."
              />
            </div>
            <div className="rounded-lg border bg-card p-3">
              <div className="flex items-center justify-between gap-2">
                <p className="text-sm font-medium">Outputs</p>
                <Link href="/outputs" className="text-[13px] font-medium text-primary underline-offset-4 hover:underline">
                  Manage outputs
                </Link>
              </div>
              {outputs.isLoading ? (
                <p className="mt-1.5 text-[13px] text-muted-foreground">Loading outputs</p>
              ) : visibleOutputs.length ? (
                <ul className="mt-2 divide-y text-[13px]">
                  {visibleOutputs.map((output) => (
                    <li key={output.id} className="flex items-center justify-between gap-3 py-1.5">
                      <span className="truncate font-medium">{output.name}</span>
                      <span className="shrink-0 text-muted-foreground">
                        {titleCase(output.kind)} on {output.events.join(", ")}
                      </span>
                    </li>
                  ))}
                </ul>
              ) : (
                <p className="mt-1.5 text-[13px] text-muted-foreground">
                  {who
                    ? `Nothing is notified when this ${whoType === "agent" ? "agent" : "team"} finishes. Add a Slack, Mattermost or webhook output to change that.`
                    : "Choose who does the work to see where results are sent."}
                </p>
              )}
            </div>
          </Step>
        </div>

        <Card className="p-4 sm:p-5 lg:sticky lg:top-6">
          <h2 className="text-[15px] font-semibold">Summary</h2>
          <dl className="mt-3 divide-y text-[13px]">
            <SummaryRow label="When" value={whenSummary} />
            <SummaryRow
              label="Where"
              value={
                scheduled
                  ? `Any free worker, ${scheduleHarness ? brandName(scheduleHarness) : "default harness"}`
                  : `${where === "auto" ? "Anywhere" : titleCase(where)}${harness ? `, ${brandName(harness)}` : ""}`
              }
            />
            <SummaryRow
              label="Who"
              value={(whoType === "agent" ? selectedAgent?.name : selectedTeam?.name) ?? null}
              placeholder="Not chosen"
            />
            <SummaryRow label="What" value={name.trim() || task.trim() || null} placeholder="No task yet" />
            <SummaryRow
              label="Then"
              value={[
                autoApprove ? "Auto-approve writes" : "Ask before writes",
                longRunning ? "keep running" : null,
                visibleOutputs.length ? `${visibleOutputs.length} output${visibleOutputs.length === 1 ? "" : "s"}` : null,
              ]
                .filter(Boolean)
                .join(", ")}
            />
          </dl>
          <Button
            className="mt-4 w-full"
            size="lg"
            disabled={Boolean(missing) || creating}
            loading={creating}
            onClick={() => void createWork()}
          >
            {scheduled ? <CalendarClock /> : <Play />} {scheduled ? "Create schedule" : "Start work"}
          </Button>
          <p className="mt-2 text-center text-[13px] text-muted-foreground" aria-live="polite">
            {missing ??
              (scheduled
                ? "Adds the schedule to the agent as a new version. Nothing runs until it next fires."
                : "Starts an execution and opens its live timeline.")}
          </p>
        </Card>
      </div>
    </div>
  );
}

function Step({
  number,
  title,
  question,
  children,
}: {
  number: number;
  title: string;
  question: string;
  children: React.ReactNode;
}) {
  return (
    <Card className="p-4 sm:p-5">
      <section aria-labelledby={`work-step-${number}`} className="grid gap-4 sm:grid-cols-[2.25rem_minmax(0,1fr)]">
        <span
          aria-hidden
          className="font-display tabular flex h-9 w-9 items-center justify-center rounded-full border border-border-strong text-[15px] text-muted-foreground"
        >
          {number}
        </span>
        <div className="min-w-0 space-y-4">
          <h2 id={`work-step-${number}`} className="flex flex-wrap items-baseline gap-x-2 pt-1.5">
            <span className="text-[15px] font-semibold">{title}</span>
            <span className="text-[13px] font-normal text-muted-foreground">{question}</span>
          </h2>
          {children}
        </div>
      </section>
    </Card>
  );
}

function SummaryRow({
  label,
  value,
  placeholder,
}: {
  label: string;
  value: string | null;
  placeholder?: string;
}) {
  return (
    <div className="grid grid-cols-[3.5rem_minmax(0,1fr)] gap-3 py-2 first:pt-0 last:pb-0">
      <dt className="text-muted-foreground">{label}</dt>
      <dd className={cn("line-clamp-3 break-words font-medium", !value && "font-normal text-muted-foreground")}>
        {value || placeholder}
      </dd>
    </div>
  );
}
