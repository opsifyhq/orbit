"use client";

export const dynamic = "force-dynamic";

import { randomId } from "@/lib/browser";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Bot, CalendarClock, Play, Plug, Server, Sparkles, Workflow, Zap } from "lucide-react";
import { toast } from "sonner";

import { ScheduleEditor, type ScheduleValue } from "@/components/schedule-editor";
import { Badge } from "@/components/ui/badge";
import {
  SectionCard,
  SectionPills,
  SectionRail,
  sectionNeighbours,
  type NavSection,
} from "@/components/ui/section-nav";
import { Button } from "@/components/ui/button";
import { Callout } from "@/components/ui/callout";
import { Skeleton } from "@/components/ui/skeleton";
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
  useAllAgents,
  useAgentTeams,
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
import { brandName, autonomyName } from "@/lib/presentation";
import { browserTimeZone, cronError, describeSchedule } from "@/lib/schedule";
import { defaultWorkerImage } from "@/lib/worker-images";

type WhoType = "agent" | "team";
type When = "manual" | "schedule";
type SectionId = "who" | "what" | "when" | "where" | "then";

const SECTION_ORDER: SectionId[] = ["who", "what", "when", "where", "then"];
const SECTION_TITLES: Record<SectionId, string> = {
  who: "Who",
  what: "What",
  when: "When",
  where: "Where",
  then: "Then",
};
const SECTION_QUESTIONS: Record<SectionId, string> = {
  who: "Who does the work?",
  what: "What should be done?",
  when: "When should it start, and does it repeat?",
  where: "Where should it run?",
  then: "What happens after?",
};
const isSectionId = (value: string): value is SectionId => (SECTION_ORDER as string[]).includes(value);

interface Issue {
  section: SectionId;
  message: string;
}

function FormSection({
  id,
  active,
  go,
  children,
}: {
  id: SectionId;
  active: SectionId;
  go: (id: SectionId) => void;
  children: ReactNode;
}) {
  return (
    <SectionCard
      id={id}
      active={active}
      title={SECTION_TITLES[id]}
      description={SECTION_QUESTIONS[id]}
      onGo={go}
      {...sectionNeighbours(SECTION_ORDER, SECTION_TITLES, id)}
    >
      {children}
    </SectionCard>
  );
}

/**
 * Start work: the same one-section-at-a-time layout as Agent Studio and Team Studio.
 * The section rail doubles as the summary, problems show per section, and the start
 * button stays in view.
 */
export default function WorkPage() {
  const router = useRouter();
  const [when, setWhen] = useState<When>("manual");
  const [schedule, setSchedule] = useState<ScheduleValue>(() => ({ cron: "0 9 * * 1-5", timezone: undefined }));
  const [where, setWhere] = useState<"auto" | "local" | "docker" | "kubernetes">("auto");
  const [harness, setHarness] = useState("");
  const [whoType, setWhoType] = useState<WhoType>("agent");
  const [who, setWho] = useState("");
  const [name, setName] = useState("");
  const [task, setTask] = useState("");
  const [repository, setRepository] = useState("");
  const [autoApprove, setAutoApprove] = useState(false);
  const [longRunning, setLongRunning] = useState(false);

  // New schedules start in the viewer's own time zone (set after mount: the server
  // render cannot know it).
  useEffect(() => {
    const zone = browserTimeZone();
    setSchedule((current) => (current.timezone ? current : { ...current, timezone: zone === "UTC" ? undefined : zone }));
  }, []);

  // Drafts can be run by hand, so they are offered too (marked as drafts).
  const agents = useAllAgents({ status: "current" });
  const teams = useAgentTeams();
  const harnesses = useHarnesses();
  const workerProfiles = useWorkerProfiles();
  const workers = useWorkers();
  const outputs = useOutputs();

  const queryClient = useQueryClient();
  const [creating, setCreating] = useState(false);
  // Identifies this attempt to start work; renewed whenever what is being started changes.
  const runKey = useRef(randomId());
  useEffect(() => {
    runKey.current = randomId();
  }, [who, task, name, repository, harness]);

  /**
   * "On a schedule" adds a schedule trigger (cron, time zone and this task) to the agent
   * by saving a new version, the same thing Agent Studio does, so it shows up and can be
   * edited or removed there.
   */
  async function createSchedule() {
    const detail = unwrap(
      await api.GET("/api/v1/agents/{agent_id}", { params: { path: { agent_id: who } } }),
    );
    const spec = detail.spec as unknown as { triggers?: unknown[] };
    const trigger = {
      type: "schedule",
      cron: schedule.cron.trim(),
      ...(schedule.timezone ? { timezone: schedule.timezone } : {}),
      task: task.trim(),
    };
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
        triggers: [...(spec.triggers ?? []), trigger],
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
    if (issues.length) {
      setShowIssues(true);
      go(issues[0].section);
      toast.error(issues.length === 1 ? issues[0].message : "Fix the highlighted sections");
      return;
    }
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
            body: {
              task: task.trim(),
              payload: repository.trim() ? { repository: repository.trim() } : {},
              harnessOverride: harness || undefined,
              name: name.trim() || undefined,
              idempotencyKey: runKey.current,
            },
            // Same key on a retry or a double click: the API returns the first run.
            headers: { "idempotency-key": runKey.current },
          }),
        );
        const effectiveHarness = harness || selectedAgent?.harness || defaultHarness || "mock";
        const target = where === "kubernetes" ? "kubernetes" : where === "docker" ? "docker" : "local";

        // A worker for this harness is already online: it will pick the work up, and
        // starting another would only add an error about the launcher.
        // Decide on loaded data only: "not fetched yet" is not "no worker", and acting
        // on it would create a profile and launch a worker beside one that is online.
        const liveWorkers = workers.data ?? (await workers.refetch()).data;
        const profiles = workerProfiles.data ?? (await workerProfiles.refetch()).data;
        if (!liveWorkers || !profiles) {
          toast.warning("The work is queued. ORBIT could not check for workers, so none was started.");
          router.push(`/executions/${result.id}`);
          return;
        }
        const workerOnline = liveWorkers.some(
          (worker) => worker.live && worker.harnesses.includes(effectiveHarness),
        );
        if (workerOnline) {
          router.push(`/executions/${result.id}`);
          return;
        }

        // Reuse an enabled profile for this harness that covers this agent; a disabled
        // or out-of-scope profile would accept the work and then never run it.
        let profileId = profiles.find(
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
    (agent) => agent.status === "active" || agent.status === "draft",
  );
  // An agent with its own harness runs on it; other harnesses are not offered for it.
  const agentHarness = whoType === "agent" ? selectedAgent?.harness ?? null : null;
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
  const scheduleHarness = selectedAgent?.harness || defaultHarness;
  const scheduleProblem = scheduled ? cronError(schedule.cron, schedule.timezone) : null;

  // ------------------------------------------------------------------ validation
  const issues: Issue[] = [];
  if (!who) issues.push({ section: "who", message: `Choose the ${whoType === "agent" ? "agent" : "team"} that does the work.` });
  if (!task.trim()) issues.push({ section: "what", message: "Describe the task." });
  if (scheduleProblem) issues.push({ section: "when", message: scheduleProblem });
  if (scheduled && selectedAgent && defaultHarnessKnown && !scheduleHarness)
    issues.push({
      section: "when",
      message: "Set a default harness in Settings first: a schedule runs on the agent's harness or the default.",
    });
  if (!scheduled && needsHarness)
    issues.push({ section: "where", message: `Choose a harness: ${selectedAgent?.name} has none and no default is set.` });
  const [showIssues, setShowIssues] = useState(false);
  const issueCount = (id: SectionId) => (showIssues ? issues.filter((issue) => issue.section === id).length : 0);

  // ------------------------------------------------------------------ sections
  // One section on screen at a time; the URL hash remembers which.
  const [active, setActive] = useState<SectionId>("who");
  const [focusRequest, setFocusRequest] = useState(0);
  useEffect(() => {
    const sync = () => {
      const fromHash = window.location.hash.replace(/^#/, "");
      if (isSectionId(fromHash)) setActive(fromHash);
    };
    sync();
    window.addEventListener("hashchange", sync);
    return () => window.removeEventListener("hashchange", sync);
  }, []);
  useEffect(() => {
    if (!focusRequest) return;
    const node = document.getElementById(`section-${active}`);
    node?.scrollIntoView({ block: "start" });
    node
      ?.querySelector<HTMLElement>("[role=radio][aria-checked=true], input:not([type=hidden]), textarea, select")
      ?.focus({ preventScroll: true });
  }, [focusRequest, active]);
  function go(id: SectionId) {
    setActive(id);
    setFocusRequest((count) => count + 1);
    window.history.replaceState(null, "", `#${id}`);
  }

  // ------------------------------------------------------------------ summaries
  const whoName = (whoType === "agent" ? selectedAgent?.name : selectedTeam?.name) ?? null;
  const scheduleSummary = useMemo(() => describeSchedule(schedule.cron, schedule.timezone), [schedule]);
  const summary: Record<SectionId, { value: string; missing?: boolean }> = {
    who: { value: whoName ?? "Not chosen", missing: !whoName },
    what: { value: name.trim() || task.trim() || "No task yet", missing: !task.trim() },
    when: { value: scheduled ? scheduleSummary : "Now, started by you" },
    where: {
      value: scheduled
        ? `Any free worker${scheduleHarness ? `, ${brandName(scheduleHarness)}` : ""}`
        : `${where === "auto" ? "Anywhere" : titleCase(where)}${harness ? `, ${brandName(harness)}` : ""}`,
    },
    then: {
      value: visibleOutputs.length
        ? `${visibleOutputs.length} output${visibleOutputs.length === 1 ? "" : "s"}`
        : "No outputs",
    },
  };

  const sectionIssues = (id: SectionId) =>
    showIssues && issues.some((issue) => issue.section === id) ? (
      <Callout tone="danger" title="Fix before starting">
        <ul className="list-disc space-y-0.5 pl-4">
          {issues
            .filter((issue) => issue.section === id)
            .map((issue) => (
              <li key={issue.message}>{issue.message}</li>
            ))}
        </ul>
      </Callout>
    ) : null;

  const actionLabel = scheduled ? "Create schedule" : "Start work";
  const actionIcon = scheduled ? <CalendarClock /> : <Play />;
  const actionHint = scheduled
    ? "Adds the schedule to the agent as a new version. Nothing runs until it next fires."
    : "Starts an execution and opens its live timeline.";

  const navSections: NavSection<SectionId>[] = SECTION_ORDER.map((id) => ({
    id,
    title: SECTION_TITLES[id],
    value: summary[id].value,
    missing: summary[id].missing,
    problems: issueCount(id),
  }));
  const sectionNav = (
    <SectionRail label="Work sections" sections={navSections} active={active} onGo={go} wrapValues />
  );

  return (
    <div className="space-y-6 pb-28 lg:pb-0">
      <PageHeader
        title="Start work"
        description="Who does it, what they do, when it starts, where it runs and what happens after. Run it once now, or put it on a schedule."
      />

      {/* Phones and tablets: the sections as a scrollable row of steps. */}
      <SectionPills label="Work sections" sections={navSections} active={active} onGo={go} />

      <div className="grid gap-6 lg:grid-cols-[15rem_minmax(0,1fr)] lg:items-start">
        <aside className="hidden lg:sticky lg:top-6 lg:block">
          {sectionNav}
          <div className="mt-4 space-y-2 border-t pt-4">
            {showIssues && issues.length ? (
              <p className="text-[13px] text-destructive" role="status">
                {issues.length} {issues.length === 1 ? "issue needs" : "issues need"} fixing.
              </p>
            ) : null}
            <Button size="lg" className="w-full" loading={creating} onClick={() => void createWork()}>
              {actionIcon} {actionLabel}
            </Button>
            <p className="text-[13px] text-muted-foreground" aria-live="polite">
              {actionHint}
            </p>
          </div>
        </aside>

        <div className="min-w-0 max-w-3xl space-y-5">
          <FormSection id="who" active={active} go={go}>
            {sectionIssues("who")}
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
              agents.isLoading ? (
                <div className="grid gap-2 sm:grid-cols-2" aria-busy="true">
                  <Skeleton className="h-16 rounded-lg" />
                  <Skeleton className="h-16 rounded-lg" />
                </div>
              ) : agents.isError ? (
                <Callout tone="danger" title="Agents could not be loaded">
                  {errorMessage(agents.error)}
                </Callout>
              ) : agentOptions.length ? (
                <div role="radiogroup" aria-label="Agent" className="grid gap-2 sm:grid-cols-2">
                  {agentOptions.map((agent) => (
                    <ChoiceCard
                      key={agent.id}
                      selected={who === agent.id}
                      onSelect={() => {
                        setWho(agent.id);
                        // A harness picked for another agent may not apply to this one.
                        if (agent.harness && harness && agent.harness !== harness) setHarness("");
                      }}
                      title={agent.name}
                      description={`${agent.harness ? brandName(agent.harness) : "Any harness"}, ${autonomyName(agent.autonomyLevel).toLowerCase()}`}
                      trailing={agent.status === "draft" ? <Badge variant="secondary">Draft</Badge> : undefined}
                    />
                  ))}
                </div>
              ) : (
                <p className="text-sm text-muted-foreground">
                  There are no agents to run yet.{" "}
                  <Link href="/agents/new" className="font-medium text-primary underline-offset-4 hover:underline">
                    Create an agent
                  </Link>
                  .
                </p>
              )
            ) : teams.isLoading ? (
              <div className="grid gap-2 sm:grid-cols-2" aria-busy="true">
                <Skeleton className="h-16 rounded-lg" />
                <Skeleton className="h-16 rounded-lg" />
              </div>
            ) : teams.isError ? (
              <Callout tone="danger" title="Agent teams could not be loaded">
                {errorMessage(teams.error)}
              </Callout>
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
                <Link href="/agent-teams/new" className="font-medium text-primary underline-offset-4 hover:underline">
                  Create a team
                </Link>
                .
              </p>
            )}
          </FormSection>

          <FormSection id="what" active={active} go={go}>
            {sectionIssues("what")}
            <Field
              label="Task"
              htmlFor="work-task"
              hint={scheduled ? "Sent to the agent every time the schedule fires." : undefined}
            >
              <Textarea
                id="work-task"
                className="min-h-[120px]"
                value={task}
                onChange={(e) => setTask(e.target.value)}
                placeholder="Describe the outcome you want, as you would to a colleague."
              />
            </Field>
            {scheduled ? null : (
              <div className="grid gap-4 sm:grid-cols-2">
                <Field label="Name" optional htmlFor="work-name" hint="Shown in the executions list instead of the agent name.">
                  <Input id="work-name" value={name} onChange={(e) => setName(e.target.value)} placeholder="Evaluate article X" />
                </Field>
                <Field label="Repository" optional htmlFor="work-repo" hint="Checked out into the workspace when the agent has a GitHub or GitLab connector that allows it.">
                  <Input id="work-repo" className="font-mono" value={repository} onChange={(e) => setRepository(e.target.value)} placeholder="owner/repo" />
                </Field>
              </div>
            )}
            {!scheduled && repository.trim() && whoType === "agent" && selectedAgent && selectedAgent.connectorCount === 0 ? (
              <Callout tone="warning" title="This repository will not be checked out">
                {selectedAgent.name} has no connector, so it starts in an empty workspace and cannot
                reach {repository.trim()}. Add a GitHub or GitLab connector with “Check out repository”
                in Agent Studio first.
              </Callout>
            ) : null}
          </FormSection>

          <FormSection id="when" active={active} go={go}>
            {sectionIssues("when")}
            <div role="radiogroup" aria-label="When" className="grid gap-2 sm:grid-cols-2">
              <ChoiceCard
                selected={when === "manual"}
                onSelect={() => setWhen("manual")}
                title="Now"
                description="Runs once, as soon as you start it"
                icon={<Play />}
              />
              <ChoiceCard
                selected={when === "schedule"}
                onSelect={() => setWhen("schedule")}
                // A schedule belongs to one agent, so it is not offered for a team.
                disabled={whoType === "team"}
                title="On a schedule"
                description={whoType === "team" ? "Schedules are set per agent, not per team" : "Repeats, for example every weekday morning"}
                icon={<CalendarClock />}
              />
              {/* Shown so the options are known; events and connector messages cannot start work yet. */}
              <ChoiceCard selected={false} onSelect={() => undefined} disabled title="On an event" description="Not available yet" icon={<Zap />} />
              <ChoiceCard selected={false} onSelect={() => undefined} disabled title="From a connector" description="Not available yet, for example a Telegram message" icon={<Plug />} />
            </div>
            {scheduled ? (
              <>
                <ScheduleEditor id="work-schedule" value={schedule} onChange={setSchedule} />
                <Callout
                  tone={selectedAgent?.status === "draft" ? "warning" : "info"}
                  title={selectedAgent ? `This adds a schedule to ${selectedAgent.name}` : "This adds a schedule to the agent you choose"}
                >
                  The agent gets the task every time the schedule fires, until you remove the schedule
                  in Agent Studio.
                  {selectedAgent?.status === "draft" ? " This agent is still a draft: schedules only fire once it is activated." : ""}
                </Callout>
              </>
            ) : null}
          </FormSection>

          <FormSection id="where" active={active} go={go}>
            {sectionIssues("where")}
            {scheduled ? (
              <Callout tone="info" title="Scheduled runs go to any free worker">
                Each run uses {selectedAgent ? `${selectedAgent.name}'s` : "the agent's"} own harness
                {scheduleHarness ? ` (${brandName(scheduleHarness)})` : ""} or the organization default, and
                starts wherever a matching worker is free. Placement and harness are picked per run only
                for work you start now.
              </Callout>
            ) : (
              <>
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
                        agentHarness
                          ? `${selectedAgent?.name} runs on ${brandName(agentHarness)}`
                          : defaultHarness
                            ? `The agent's own harness, or the organization default (${titleCase(defaultHarness)})`
                            : "The harness set on the agent, if it has one"
                      }
                    />
                    {(harnesses.data ?? []).map((h) => (
                      <ChoiceCard
                        key={h.kind}
                        selected={harness === h.kind}
                        onSelect={() => setHarness(h.kind)}
                        disabled={h.implementationStatus !== "available" || Boolean(agentHarness && agentHarness !== h.kind)}
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
              </>
            )}
          </FormSection>

          <FormSection id="then" active={active} go={go}>
            {sectionIssues("then")}
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
          </FormSection>
        </div>
      </div>

      {/* Phones and tablets: the action stays within thumb reach. */}
      <div className="fixed inset-x-0 bottom-14 z-30 border-t bg-background/95 p-3 backdrop-blur lg:hidden">
        <div className="flex items-center justify-between gap-3">
          <p className="min-w-0 truncate text-[13px] text-muted-foreground">
            {[whoName, scheduled ? scheduleSummary : "Now"].filter(Boolean).join(" · ")}
          </p>
          <Button size="lg" className="shrink-0" loading={creating} onClick={() => void createWork()}>
            {actionIcon} {actionLabel}
          </Button>
        </div>
      </div>
    </div>
  );
}
