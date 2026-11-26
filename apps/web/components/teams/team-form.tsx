"use client";

import { useUnsavedChanges } from "@/lib/use-unsaved-changes";
import { useEffect, useRef, useState, type ReactNode } from "react";
import Link from "next/link";
import { ArrowDown, ArrowUp, ChevronLeft, Plus, X } from "lucide-react";
import { toast } from "sonner";

import {
  ResourceAccessFields,
  type ResourceAccessHandle,
  type ResourceSubmit,
} from "@/components/resource-access-fields";
import { SkillPicker } from "@/components/skill-picker";
import {
  SectionCard,
  SectionPills,
  SectionRail,
  sectionNeighbours,
  type NavSection,
} from "@/components/ui/section-nav";
import { Button } from "@/components/ui/button";
import { Callout } from "@/components/ui/callout";
import { Card, CardContent, CardHeader } from "@/components/ui/card";
import { Checkbox, CheckboxField } from "@/components/ui/checkbox";
import { ChoiceCard } from "@/components/ui/choice-card";
import { Disclosure } from "@/components/ui/disclosure";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { AUTONOMY_LEVELS } from "@orbit/contracts";
import { brandName } from "@/lib/presentation";
import {
  useAllAgents,
  useConnections,
  useEnvMappings,
  useHarnesses,
  useSkills,
  useWorkerProfiles,
} from "@/lib/queries";
import type { TeamAutonomy, TeamInput, TeamModel, TeamStepInput } from "@/lib/teams";
import { autonomyDraft, teamAutonomyInput, type TeamApprovalChoice } from "@/lib/team-autonomy";
import type { AgentTeam, TeamRepository, WorkSource } from "@/lib/types";
import { cn } from "@/lib/utils";

import {
  agentLabel,
  cleanWorkflow,
  workflowFromSteps,
  workflowIssues,
  type AgentListState,
  type Workflow,
} from "@/lib/workflow";

import { MODE_HINT, MODE_LABEL, StepNumber, modelLabel, type TeamMode } from "./team-parts";
import { WorkflowEditor } from "./workflow/workflow-editor";

type SectionId = "goal" | "agents" | "workspace" | "skills" | "access" | "runtime" | "sharing";

const SECTION_TITLES: Record<SectionId, string> = {
  goal: "Goal",
  agents: "Agents",
  workspace: "Repository and work",
  skills: "Skills",
  access: "Connections and environment",
  runtime: "Worker and limits",
  sharing: "Access control",
};

const SECTION_ORDER: SectionId[] = ["goal", "agents", "workspace", "skills", "access", "runtime", "sharing"];

const isSectionId = (value: string): value is SectionId => (SECTION_ORDER as string[]).includes(value);

const DEFAULT_GOAL =
  "Pick up the next GitHub issue labelled agent-ready, implement it, test it and request review. Repeat until no ready issues remain.";

/** Everything the form edits. Fields without a control (none, today) still round-trip. */
interface Draft {
  name: string;
  description: string;
  instructions: string;
  mode: TeamMode;
  steps: TeamStepInput[];
  /** The graph for mode "workflow". Kept while another mode is chosen, so switching back loses nothing. */
  workflow: Workflow | null;
  model: TeamModel | null;
  harness: string;
  repository: string;
  branch: string;
  repositories: TeamRepository[];
  workSource: WorkSource | null;
  skills: string[];
  connectionIds: string[];
  envMappingIds: string[];
  workerProfileId: string;
  autoLaunch: boolean;
  timeout: string;
  autonomyMode: "follow_primary" | "fixed";
  autonomyLevel: string;
  autonomyApproval: TeamApprovalChoice;
}

function draftFrom(team?: AgentTeam): Draft {
  const definition = team?.definition;
  return {
    name: team?.name ?? "Coding team",
    description: team?.description ?? "",
    instructions: team ? (definition?.instructions ?? "") : DEFAULT_GOAL,
    mode: definition?.mode ?? (team ? "pipeline" : "native"),
    steps: definition?.steps?.length
      ? definition.steps.map((step) => ({ agentId: step.agentId, model: step.model ?? null }))
      : [{ agentId: "" }, { agentId: "" }],
    workflow: definition?.workflow ?? null,
    model: definition?.model ?? null,
    harness: definition?.harness ?? "",
    repository: definition?.repository ?? "",
    branch: definition?.branch ?? "",
    repositories: definition?.repositories ?? [],
    workSource: definition?.workSource ?? null,
    skills: definition?.skills ?? [],
    connectionIds: definition?.connectionIds ?? [],
    envMappingIds: definition?.envMappingIds ?? [],
    workerProfileId: definition?.workerProfileId ?? "",
    autoLaunch: definition?.autoLaunch ?? false,
    timeout: definition?.timeoutSeconds ? String(definition.timeoutSeconds) : "",
    autonomyMode: autonomyDraft(definition?.autonomy).mode,
    autonomyLevel: autonomyDraft(definition?.autonomy).level,
    autonomyApproval: autonomyDraft(definition?.autonomy).approval,
  };
}

/** A model override with nothing in it is no override. */
function cleanModel(model: TeamModel | null | undefined): TeamModel | null {
  if (!model) return null;
  const provider = model.provider?.trim() || null;
  const name = model.model?.trim() || null;
  if (!provider && !name) return null;
  return { ...model, provider, model: name };
}

function toInput(draft: Draft): TeamInput {
  const workflow = draft.workflow ? cleanWorkflow(draft.workflow) : null;
  return {
    // A workflow names its agents on its steps; the API derives the agent list from it.
    workflow: draft.mode === "workflow" ? workflow : workflow && !workflowIssues(workflow).length ? workflow : null,
    name: draft.name.trim(),
    description: draft.description,
    mode: draft.mode,
    instructions: draft.instructions,
    repository: draft.repository.trim() || null,
    branch: draft.branch.trim() || null,
    workSource: draft.workSource,
    repositories: draft.repositories,
    workerProfileId: draft.workerProfileId || null,
    autoLaunch: draft.autoLaunch,
    skills: draft.skills,
    model: cleanModel(draft.model),
    harness: draft.harness || null,
    connectionIds: draft.connectionIds,
    envMappingIds: draft.envMappingIds,
    timeoutSeconds: draft.timeout ? Number(draft.timeout) : null,
    autonomy: teamAutonomyInput(draft.autonomyMode, draft.autonomyLevel, draft.autonomyApproval) satisfies TeamAutonomy | null,
    steps:
      draft.mode === "workflow"
        ? []
        : draft.steps.map((step) => ({ agentId: step.agentId, model: cleanModel(step.model) })),
  };
}

interface Issue {
  section: SectionId;
  message: string;
}

function FormSection({
  id,
  active,
  go,
  description,
  children,
}: {
  id: SectionId;
  active: SectionId;
  go: (id: SectionId) => void;
  description: string;
  children: ReactNode;
}) {
  return (
    <SectionCard
      id={id}
      active={active}
      title={SECTION_TITLES[id]}
      description={description}
      onGo={go}
      {...sectionNeighbours(SECTION_ORDER, SECTION_TITLES, id)}
    >
      {children}
    </SectionCard>
  );
}

/** Pick ids from a list with checkboxes; scrolls past a handful of options. */
function CheckList({
  options,
  value,
  onChange,
  empty,
}: {
  options: Array<{ id: string; title: string; description?: string }>;
  value: string[];
  onChange: (next: string[]) => void;
  empty: ReactNode;
}) {
  if (!options.length) return <p className="text-[13px] text-muted-foreground">{empty}</p>;
  return (
    <div className="grid max-h-60 gap-2 overflow-y-auto sm:grid-cols-2">
      {options.map((option) => (
        <CheckboxField
          key={option.id}
          title={option.title}
          description={option.description}
          checked={value.includes(option.id)}
          onChange={(event) =>
            onChange(event.target.checked ? [...value, option.id] : value.filter((id) => id !== option.id))
          }
        />
      ))}
    </div>
  );
}

/**
 * Team Studio: the same calm, one-section-at-a-time form as Agent Studio. The section
 * nav doubles as a summary of the team, problems are shown per section, and Save stays
 * in view. Every field of the team definition round-trips, including model overrides.
 */
export function TeamForm({
  team,
  submitLabel,
  submitting,
  onSubmit,
  cancelHref,
}: {
  team?: AgentTeam;
  submitLabel: string;
  submitting?: boolean;
  onSubmit: ResourceSubmit<TeamInput>;
  cancelHref: string;
}) {
  const agents = useAllAgents();
  const skills = useSkills();
  const connections = useConnections();
  const envMappings = useEnvMappings();
  const workerProfiles = useWorkerProfiles();
  const harnesses = useHarnesses();

  const [draft, setDraft] = useState<Draft>(() => draftFrom(team));
  const [accessDirty, setAccessDirty] = useState(false);
  useUnsavedChanges({ draft, accessDirty });
  const accessRef = useRef<ResourceAccessHandle>(null);
  const set = <K extends keyof Draft>(key: K, value: Draft[K]) => setDraft((current) => ({ ...current, [key]: value }));
  const patchWorkSource = (patch: Partial<WorkSource>) =>
    setDraft((current) => ({ ...current, workSource: current.workSource ? { ...current.workSource, ...patch } : null }));
  const setStep = (index: number, patch: Partial<TeamStepInput>) =>
    setDraft((current) => ({
      ...current,
      steps: current.steps.map((step, i) => (i === index ? { ...step, ...patch } : step)),
    }));
  const moveStep = (index: number, by: -1 | 1) =>
    setDraft((current) => {
      const steps = [...current.steps];
      const target = index + by;
      if (target < 0 || target >= steps.length) return current;
      [steps[index], steps[target]] = [steps[target], steps[index]];
      return { ...current, steps };
    });

  const agentOptions = (agents.data?.items ?? []).filter((agent) => agent.status !== "archived");
  // The editor gets archived agents too, so a step that still names one says "Archived agent".
  const workflowAgents = (agents.data?.items ?? []).map((agent) => ({ ...agent, archived: agent.status === "archived" }));
  const agentsState: AgentListState = agents.isError ? "error" : agents.isPending ? "loading" : "ready";
  const agentById = new Map((agents.data?.items ?? []).map((agent) => [agent.id, agent]));
  const primary = agentById.get(draft.steps[0]?.agentId ?? "");
  const isWorkflow = draft.mode === "workflow";
  const workflow = draft.workflow ?? { nodes: [], edges: [] };
  const workflowProblems = isWorkflow ? workflowIssues(workflow) : [];

  /** Choosing Workflow for the first time turns the agents chosen so far into a line of steps. */
  function chooseMode(mode: TeamMode) {
    setDraft((current) => {
      if (mode !== "workflow" || current.workflow?.nodes.length) return { ...current, mode };
      const chosenSteps = current.steps.filter((step) => step.agentId);
      return {
        ...current,
        mode,
        workflow: workflowFromSteps(
          chosenSteps.map((step) => ({ agentId: step.agentId, name: agentById.get(step.agentId)?.name })),
        ),
      };
    });
  }

  // ------------------------------------------------------------------ validation
  const issues: Issue[] = [];
  if (!draft.name.trim()) issues.push({ section: "goal", message: "Enter a name." });
  if (isWorkflow) {
    // Two steps can have the same problem ("Every step needs a name."): list it once.
    for (const message of new Set(workflowProblems.map((problem) => problem.message)))
      issues.push({ section: "agents", message });
  } else {
    if (!draft.steps.length) issues.push({ section: "agents", message: "Add at least one agent." });
    if (draft.steps.some((step) => !step.agentId))
      issues.push({ section: "agents", message: "Choose an agent for every position." });
    const chosen = draft.steps.map((step) => step.agentId).filter(Boolean);
    if (new Set(chosen).size !== chosen.length)
      issues.push({ section: "agents", message: "Each agent can appear only once." });
  }
  if (draft.mode === "native" && primary && primary.harness !== "opencode")
    issues.push({ section: "agents", message: "Native subagents need an OpenCode primary agent. Pick one, or use a pipeline." });
  if (draft.workSource?.enabled && !draft.workSource.connectionId)
    issues.push({ section: "workspace", message: "Choose the connection the work source polls." });
  const [showIssues, setShowIssues] = useState(false);
  const [revealProblem, setRevealProblem] = useState(0);
  const issueCount = (id: SectionId) => (showIssues ? issues.filter((issue) => issue.section === id).length : 0);

  // ------------------------------------------------------------------ sections
  // One section on screen at a time; the URL hash remembers which.
  const [active, setActive] = useState<SectionId>("goal");
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
    node?.querySelector<HTMLElement>("input:not([type=hidden]), textarea, select")?.focus({ preventScroll: true });
  }, [focusRequest, active]);
  function go(id: SectionId) {
    setActive(id);
    setFocusRequest((count) => count + 1);
    window.history.replaceState(null, "", `#${id}`);
  }

  async function submit() {
    if (issues.length) {
      setShowIssues(true);
      go(issues[0].section);
      if (issues[0].section === "agents") setRevealProblem((count) => count + 1);
      toast.error(issues.length === 1 ? issues[0].message : "Fix the highlighted sections");
      return;
    }
    // Access is stored separately; the parent commits it after the team itself is saved.
    const commitAccess = async (id?: string) => (await accessRef.current?.save(id)) ?? true;
    await onSubmit(toInput(draft), commitAccess);
  }

  // ------------------------------------------------------------------ summaries
  const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;
  const filledSteps = draft.steps.filter((step) => step.agentId);
  const source = draft.workSource;
  const profile = (workerProfiles.data ?? []).find((item) => item.id === draft.workerProfileId);
  const summary: Record<SectionId, { value: string; missing?: boolean }> = {
    goal: { value: draft.name.trim() || "Name not set", missing: !draft.name.trim() },
    agents: isWorkflow
      ? {
          value: workflow.nodes.length ? `Workflow · ${plural(workflow.nodes.length, "step")}` : "No steps yet",
          missing: !workflow.nodes.length,
        }
      : {
          value: filledSteps.length
            ? `${MODE_LABEL[draft.mode]} · ${plural(filledSteps.length, "agent")}`
            : "No agents chosen",
          missing: !filledSteps.length,
        },
    workspace: {
      value: source?.enabled
        ? `Picks up ${source.provider === "gitlab" ? "GitLab" : "GitHub"} issues`
        : draft.repositories.length
          ? plural(draft.repositories.length, "repository").replace("repositorys", "repositories")
          : draft.repository.trim() || "Chosen per run",
    },
    skills: { value: draft.skills.length ? plural(draft.skills.length, "skill") : "None" },
    access: {
      value:
        draft.connectionIds.length || draft.envMappingIds.length
          ? [
              draft.connectionIds.length ? plural(draft.connectionIds.length, "connection") : null,
              draft.envMappingIds.length ? plural(draft.envMappingIds.length, "mapping") : null,
            ]
              .filter(Boolean)
              .join(" · ")
          : "None",
    },
    runtime: {
      value: profile
        ? `${profile.name}${draft.autoLaunch ? ", starts automatically" : ""}`
        : draft.timeout
          ? `Timeout ${draft.timeout} s`
          : "Defaults",
    },
    sharing: { value: team ? "Who can see and use it" : "After the first save" },
  };

  const navSections: NavSection<SectionId>[] = SECTION_ORDER.map((id) => ({
    id,
    title: SECTION_TITLES[id],
    value: summary[id].value,
    missing: summary[id].missing,
    problems: issueCount(id),
  }));
  const sectionNav = (
    <SectionRail label="Team sections" sections={navSections} active={active} onGo={go} wrapValues />
  );

  const sectionIssues = (id: SectionId) =>
    showIssues && issues.some((issue) => issue.section === id) ? (
      <Callout tone="danger" title="Fix before saving">
        <ul className="list-disc space-y-0.5 pl-4">
          {issues
            .filter((issue) => issue.section === id)
            .map((issue) => (
              <li key={issue.message}>{issue.message}</li>
            ))}
        </ul>
      </Callout>
    ) : null;

  const harnessOptions = (harnesses.data ?? []).filter(
    (item) => item.enabled && item.implementationStatus === "available",
  );

  return (
    <div className="pb-24 lg:pb-0">
      {/* Phones and tablets: the sections as a scrollable row of steps. */}
      <SectionPills label="Team sections" sections={navSections} active={active} onGo={go} className="mb-4" />

      <div className="grid gap-6 lg:grid-cols-[15rem_minmax(0,1fr)] lg:items-start">
        <aside className="hidden lg:sticky lg:top-6 lg:block">
          {sectionNav}
          <div className="mt-4 space-y-2 border-t pt-4">
            {showIssues && issues.length ? (
              <p className="text-[13px] text-destructive" role="status">
                {issues.length} {issues.length === 1 ? "issue needs" : "issues need"} fixing.
              </p>
            ) : null}
            <Button onClick={() => void submit()} loading={submitting} size="lg" className="w-full">
              {submitLabel}
            </Button>
            <Button asChild variant="ghost" className="w-full">
              <Link href={cancelHref}>Cancel</Link>
            </Button>
          </div>
        </aside>

        {/* The workflow canvas needs the room a form column does not. */}
        <div className={cn("min-w-0 space-y-5", isWorkflow && active === "agents" ? "max-w-6xl" : "max-w-3xl")}>
          <FormSection id="goal" active={active} go={go} description="What the team is called and what it works toward.">
            {sectionIssues("goal")}
            <div className="grid gap-4 sm:grid-cols-2">
              <Field label="Name" htmlFor="team-name">
                <Input id="team-name" value={draft.name} onChange={(event) => set("name", event.target.value)} />
              </Field>
              <Field label="Description" htmlFor="team-description" optional>
                <Input
                  id="team-description"
                  value={draft.description}
                  onChange={(event) => set("description", event.target.value)}
                  placeholder="What this team is for"
                />
              </Field>
            </div>
            <Field
              label="Goal"
              htmlFor="team-goal"
              hint="Given to the primary agent as the default task, and repeated to every later step along with the previous output. A run can replace it."
            >
              <Textarea
                id="team-goal"
                className="min-h-32"
                value={draft.instructions}
                onChange={(event) => set("instructions", event.target.value)}
                placeholder="Pick up issues labelled agent-ready, fix them in a loop, and stop when none remain."
              />
            </Field>
          </FormSection>

          <FormSection
            id="agents"
            active={active}
            go={go}
            description={
              isWorkflow
                ? "Who is on the team and how they work together. Each step runs one agent; the lines decide what happens next."
                : "Who is on the team and how they work together. The first agent leads; each agent's own instructions define its role."
            }
          >
            {sectionIssues("agents")}
            <div role="radiogroup" aria-label="How the team runs" className="grid gap-2 sm:grid-cols-3">
              {(["native", "pipeline", "workflow"] as const).map((mode) => (
                <ChoiceCard
                  key={mode}
                  selected={draft.mode === mode}
                  onSelect={() => chooseMode(mode)}
                  title={MODE_LABEL[mode]}
                  description={MODE_HINT[mode]}
                />
              ))}
            </div>

            {isWorkflow ? (
              <WorkflowEditor
                value={workflow}
                onChange={(next) => set("workflow", next)}
                agents={workflowAgents}
                agentsState={agentsState}
                issues={workflowProblems}
                revealProblem={revealProblem}
              />
            ) : null}

            <div className="space-y-2" hidden={isWorkflow}>
              <div className="flex items-center justify-between gap-2">
                <p className="text-sm font-medium">{draft.mode === "native" ? "Primary agent and subagents" : "Agents in order"}</p>
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  onClick={() => set("steps", [...draft.steps, { agentId: "" }])}
                >
                  <Plus /> Add agent
                </Button>
              </div>
              <ol className="divide-y rounded-lg border">
                {draft.steps.map((step, index) => {
                  const chosenAgent = agentById.get(step.agentId);
                  const model = modelLabel(step.model);
                  return (
                    <li key={index} className="grid grid-cols-[auto_minmax(0,1fr)] items-start gap-x-3 gap-y-1 p-3 sm:grid-cols-[auto_minmax(0,1fr)_auto]">
                      <StepNumber n={index + 1} className="mt-1.5" />
                      <div className="min-w-0 space-y-1.5">
                        <Select
                          aria-label={`Agent ${index + 1}`}
                          value={step.agentId}
                          onChange={(event) => setStep(index, { agentId: event.target.value })}
                        >
                          <option value="">Choose an agent…</option>
                          {agentOptions.map((agent) => (
                            <option key={agent.id} value={agent.id}>
                              {agent.name}
                              {agent.harness ? ` (${brandName(agent.harness)})` : ""}
                            </option>
                          ))}
                          {step.agentId && (!chosenAgent || chosenAgent.status === "archived") ? (
                            <option value={step.agentId}>
                              {agentLabel(step.agentId, workflowAgents, agentsState)}
                            </option>
                          ) : null}
                        </Select>
                        <p className="text-xs text-muted-foreground">
                          {index === 0
                            ? "Primary agent: receives the goal or the run's task"
                            : draft.mode === "native"
                              ? "Subagent the primary agent can call"
                              : `Runs after agent ${index} and receives its output`}
                          {model ? <span className="font-mono"> · model {model}</span> : null}
                        </p>
                      </div>
                      <div className="col-start-2 -ml-2 flex items-center sm:col-start-auto sm:ml-0">
                        <Button
                          type="button"
                          size="icon-sm"
                          variant="ghost"
                          aria-label={`Move agent ${index + 1} up`}
                          disabled={index === 0}
                          onClick={() => moveStep(index, -1)}
                        >
                          <ArrowUp />
                        </Button>
                        <Button
                          type="button"
                          size="icon-sm"
                          variant="ghost"
                          aria-label={`Move agent ${index + 1} down`}
                          disabled={index === draft.steps.length - 1}
                          onClick={() => moveStep(index, 1)}
                        >
                          <ArrowDown />
                        </Button>
                        <Button
                          type="button"
                          size="icon-sm"
                          variant="ghost"
                          aria-label={`Remove agent ${index + 1}`}
                          disabled={draft.steps.length <= 1}
                          onClick={() => set("steps", draft.steps.filter((_, i) => i !== index))}
                        >
                          <X />
                        </Button>
                      </div>
                    </li>
                  );
                })}
              </ol>
            </div>

            <Disclosure
              title="Model overrides"
              summary={
                modelLabel(draft.model) || draft.steps.some((step) => modelLabel(step.model))
                  ? [
                      modelLabel(draft.model) ? `Team: ${modelLabel(draft.model)}` : null,
                      draft.steps.filter((step) => modelLabel(step.model)).length
                        ? `${plural(draft.steps.filter((step) => modelLabel(step.model)).length, "agent")} overridden`
                        : null,
                    ]
                      .filter(Boolean)
                      .join(" · ")
                  : "Each agent uses its own model"
              }
              defaultOpen={Boolean(modelLabel(draft.model) || draft.steps.some((step) => modelLabel(step.model)))}
            >
              <p className="text-[13px] text-muted-foreground">
                Optional. A team model replaces every agent&apos;s model for this team; a{" "}
                {isWorkflow ? "model set on a step" : "per-agent model"} wins over both. Use provider and model names
                as they appear under Model providers.
              </p>
              <div className="grid gap-3 sm:grid-cols-2">
                <Field label="Team provider" htmlFor="team-model-provider" optional>
                  <Input
                    id="team-model-provider"
                    className="font-mono"
                    value={draft.model?.provider ?? ""}
                    onChange={(event) => set("model", { ...draft.model, provider: event.target.value })}
                    placeholder="openai"
                  />
                </Field>
                <Field label="Team model" htmlFor="team-model-name" optional>
                  <Input
                    id="team-model-name"
                    className="font-mono"
                    value={draft.model?.model ?? ""}
                    onChange={(event) => set("model", { ...draft.model, model: event.target.value })}
                    placeholder="gpt-5"
                  />
                </Field>
              </div>
              {!isWorkflow && draft.steps.some((step) => step.agentId) ? (
                <ul className="space-y-2">
                  {draft.steps.map((step, index) =>
                    step.agentId ? (
                      <li key={`model-${index}`} className="grid gap-2 sm:grid-cols-[minmax(0,10rem)_minmax(0,1fr)_minmax(0,1fr)] sm:items-center">
                        <span className="truncate text-[13px] font-medium">
                          {index + 1}. {agentById.get(step.agentId)?.name ?? "Agent"}
                        </span>
                        <Input
                          aria-label={`Provider for agent ${index + 1}`}
                          className="font-mono"
                          value={step.model?.provider ?? ""}
                          onChange={(event) => setStep(index, { model: { ...step.model, provider: event.target.value } })}
                          placeholder="Provider"
                        />
                        <Input
                          aria-label={`Model for agent ${index + 1}`}
                          className="font-mono"
                          value={step.model?.model ?? ""}
                          onChange={(event) => setStep(index, { model: { ...step.model, model: event.target.value } })}
                          placeholder="Model"
                        />
                      </li>
                    ) : null,
                  )}
                </ul>
              ) : null}
            </Disclosure>
          </FormSection>

          <FormSection
            id="workspace"
            active={active}
            go={go}
            description="Where the team works and where its work comes from. A run can choose another repository."
          >
            {sectionIssues("workspace")}
            <div className="grid gap-4 sm:grid-cols-2">
              <Field label="Repository" htmlFor="team-repo" optional hint="The checkout for every run, unless the run names another.">
                <Input
                  id="team-repo"
                  className="font-mono"
                  value={draft.repository}
                  onChange={(event) => set("repository", event.target.value)}
                  placeholder="owner/repo"
                />
              </Field>
              <Field label="Base branch" htmlFor="team-branch" optional>
                <Input
                  id="team-branch"
                  className="font-mono"
                  value={draft.branch}
                  onChange={(event) => set("branch", event.target.value)}
                  placeholder="main"
                />
              </Field>
            </div>

            <Disclosure
              title="More than one repository"
              summary={
                draft.repositories.length
                  ? `${draft.repositories.length} ${draft.repositories.length === 1 ? "repository" : "repositories"} in the workspace`
                  : "Uses the repository above"
              }
              defaultOpen={draft.repositories.length > 0}
            >
              <p className="text-[13px] text-muted-foreground">
                Check out several repositories side by side. The primary one is the working directory.
              </p>
              {draft.repositories.map((item, index) => (
                <div
                  key={index}
                  className="grid gap-2 rounded-lg border p-2.5 sm:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto_auto] sm:items-center"
                >
                  <Input
                    aria-label={`Repository ${index + 1}`}
                    className="font-mono"
                    value={item.repository}
                    onChange={(event) =>
                      set(
                        "repositories",
                        draft.repositories.map((entry, i) => (i === index ? { ...entry, repository: event.target.value } : entry)),
                      )
                    }
                    placeholder="owner/repo"
                  />
                  <Input
                    aria-label={`Workspace path for repository ${index + 1}`}
                    className="font-mono"
                    value={item.path ?? ""}
                    onChange={(event) =>
                      set(
                        "repositories",
                        draft.repositories.map((entry, i) => (i === index ? { ...entry, path: event.target.value } : entry)),
                      )
                    }
                    placeholder="Path, such as lib"
                  />
                  <label className="flex min-h-9 items-center gap-2 text-sm">
                    <Checkbox
                      checked={Boolean(item.primary)}
                      onChange={(event) =>
                        set(
                          "repositories",
                          draft.repositories.map((entry, i) => (i === index ? { ...entry, primary: event.target.checked } : entry)),
                        )
                      }
                    />
                    Primary
                  </label>
                  <Button
                    type="button"
                    size="icon-sm"
                    variant="ghost"
                    aria-label={`Remove repository ${index + 1}`}
                    onClick={() => set("repositories", draft.repositories.filter((_, i) => i !== index))}
                  >
                    <X />
                  </Button>
                </div>
              ))}
              <Button
                type="button"
                size="sm"
                variant="outline"
                onClick={() =>
                  set("repositories", [
                    ...draft.repositories,
                    { repository: "", path: "", primary: draft.repositories.length === 0, connectionId: null },
                  ])
                }
              >
                <Plus /> Add repository
              </Button>
            </Disclosure>

            <Disclosure
              title="Work source"
              summary={source?.enabled ? `Picks up ${source.provider === "gitlab" ? "GitLab" : "GitHub"} issues automatically` : "Off"}
              defaultOpen={Boolean(source?.enabled)}
            >
              <CheckboxField
                title="Pick up issues automatically"
                description="Polls the connection, claims matching issues and adds them to the team's queue."
                checked={Boolean(source?.enabled)}
                onChange={(event) =>
                  set("workSource", {
                    provider: source?.provider ?? "github",
                    enabled: event.target.checked,
                    connectionId: source?.connectionId ?? null,
                    labels: source?.labels ?? ["agent-ready"],
                    claimLabel: source?.claimLabel ?? "agent-in-progress",
                    doneLabel: source?.doneLabel ?? "agent-done",
                    failedLabel: source?.failedLabel ?? "agent-failed",
                    maxItems: source?.maxItems ?? 5,
                    maxAttempts: source?.maxAttempts ?? 1,
                    pollSeconds: source?.pollSeconds ?? 60,
                    autoRun: source?.autoRun ?? false,
                    repository: source?.repository,
                    project: source?.project,
                    groupId: source?.groupId,
                    boardId: source?.boardId,
                  })
                }
              />
              {source?.enabled ? (
                <div className="grid gap-4 sm:grid-cols-2">
                  <Field label="Provider" htmlFor="ws-provider">
                    <Select
                      id="ws-provider"
                      value={source.provider}
                      onChange={(event) => patchWorkSource({ provider: event.target.value as WorkSource["provider"] })}
                    >
                      <option value="github">GitHub</option>
                      <option value="gitlab">GitLab</option>
                    </Select>
                  </Field>
                  <Field label="Connection" htmlFor="ws-connection">
                    <Select
                      id="ws-connection"
                      value={source.connectionId ?? ""}
                      onChange={(event) => patchWorkSource({ connectionId: event.target.value || null })}
                    >
                      <option value="">Choose a connection…</option>
                      {(connections.data ?? [])
                        .filter((connection) => connection.connectorSlug === source.provider)
                        .map((connection) => (
                          <option key={connection.id} value={connection.id}>
                            {connection.name}
                          </option>
                        ))}
                    </Select>
                  </Field>
                  {source.provider === "github" ? (
                    <Field label="Repository" htmlFor="ws-repo">
                      <Input
                        id="ws-repo"
                        className="font-mono"
                        value={source.repository ?? ""}
                        onChange={(event) => patchWorkSource({ repository: event.target.value })}
                        placeholder="owner/repo"
                      />
                    </Field>
                  ) : (
                    <>
                      <Field label="Project" htmlFor="ws-project">
                        <Input
                          id="ws-project"
                          className="font-mono"
                          value={source.project ?? ""}
                          onChange={(event) => patchWorkSource({ project: event.target.value })}
                          placeholder="group/project"
                        />
                      </Field>
                      <Field label="Group" htmlFor="ws-group" optional hint="Use instead of a project.">
                        <Input
                          id="ws-group"
                          className="font-mono"
                          value={source.groupId ?? ""}
                          onChange={(event) => patchWorkSource({ groupId: event.target.value })}
                          placeholder="group"
                        />
                      </Field>
                      <Field label="Board ID" htmlFor="ws-board" optional>
                        <Input
                          id="ws-board"
                          className="font-mono"
                          value={source.boardId ?? ""}
                          onChange={(event) =>
                            patchWorkSource({ boardId: event.target.value ? Number(event.target.value) : null })
                          }
                          placeholder="123"
                        />
                      </Field>
                    </>
                  )}
                  <Field
                    label="Ready labels"
                    htmlFor="ws-labels"
                    hint="Comma-separated. Issues with any of these are picked up."
                    className="sm:col-span-2"
                  >
                    <Input
                      id="ws-labels"
                      value={(source.labels ?? []).join(",")}
                      onChange={(event) =>
                        patchWorkSource({
                          labels: event.target.value.split(",").map((label) => label.trim()).filter(Boolean),
                        })
                      }
                      placeholder="agent-ready"
                    />
                  </Field>
                  <div className="grid gap-4 sm:col-span-2 sm:grid-cols-3">
                    <Field label="Max items" htmlFor="ws-max-items">
                      <Input
                        id="ws-max-items"
                        type="number"
                        min={1}
                        value={source.maxItems ?? 5}
                        onChange={(event) => patchWorkSource({ maxItems: Number(event.target.value) })}
                      />
                    </Field>
                    <Field label="Max attempts" htmlFor="ws-max-attempts">
                      <Input
                        id="ws-max-attempts"
                        type="number"
                        min={1}
                        value={source.maxAttempts ?? 1}
                        onChange={(event) => patchWorkSource({ maxAttempts: Number(event.target.value) })}
                      />
                    </Field>
                    <Field label="Poll every (seconds)" htmlFor="ws-poll">
                      <Input
                        id="ws-poll"
                        type="number"
                        min={10}
                        value={source.pollSeconds ?? 60}
                        onChange={(event) => patchWorkSource({ pollSeconds: Number(event.target.value) })}
                      />
                    </Field>
                  </div>
                  <CheckboxField
                    className="sm:col-span-2"
                    title="Start runs automatically"
                    description="Run pending queue items as soon as they are found."
                    checked={Boolean(source.autoRun)}
                    onChange={(event) => patchWorkSource({ autoRun: event.target.checked })}
                  />
                </div>
              ) : null}
            </Disclosure>
          </FormSection>

          <FormSection id="skills" active={active} go={go} description="Skills every agent in the team can use, on top of its own.">
            <SkillPicker
              skills={skills.data ?? []}
              value={draft.skills}
              onChange={(next) => set("skills", next)}
              empty={
                <>
                  No skills yet.{" "}
                  <Link href="/skills" className="text-primary hover:underline">
                    Create one
                  </Link>{" "}
                  to share it here.
                </>
              }
            />
          </FormSection>

          <FormSection
            id="access"
            active={active}
            go={go}
            description="Connections and environment variables the team's runs get, in addition to each agent's own."
          >
            <div className="space-y-2">
              <p className="text-sm font-medium">Connections</p>
              <CheckList
                options={(connections.data ?? []).map((connection) => ({
                  id: connection.id,
                  title: connection.name,
                  description: brandName(connection.connectorSlug),
                }))}
                value={draft.connectionIds}
                onChange={(next) => set("connectionIds", next)}
                empty={
                  <>
                    No connections yet.{" "}
                    <Link href="/connections" className="text-primary hover:underline">
                      Add one
                    </Link>
                    .
                  </>
                }
              />
            </div>
            <div className="space-y-2">
              <p className="text-sm font-medium">Environment mappings</p>
              <CheckList
                options={(envMappings.data ?? []).map((mapping) => ({ id: mapping.id, title: mapping.name }))}
                value={draft.envMappingIds}
                onChange={(next) => set("envMappingIds", next)}
                empty={
                  <>
                    No environment mappings yet.{" "}
                    <Link href="/environments" className="text-primary hover:underline">
                      Add one
                    </Link>
                    .
                  </>
                }
              />
            </div>
          </FormSection>

          <FormSection id="runtime" active={active} go={go} description="Where the team's runs execute and how long they may take.">
            <div className="grid gap-4 sm:grid-cols-2">
              <Field label="Worker profile" htmlFor="team-worker" optional>
                <Select id="team-worker" value={draft.workerProfileId} onChange={(event) => set("workerProfileId", event.target.value)}>
                  <option value="">Any matching worker</option>
                  {(workerProfiles.data ?? [])
                    .filter((item) => item.enabled || item.id === draft.workerProfileId)
                    .map((item) => (
                      <option key={item.id} value={item.id}>
                        {item.name} ({brandName(item.harnessKind)}, {item.target})
                      </option>
                    ))}
                </Select>
              </Field>
              <Field label="Harness" htmlFor="team-harness" optional hint="Default for runs; a run can choose another.">
                <Select id="team-harness" value={draft.harness} onChange={(event) => set("harness", event.target.value)}>
                  <option value="">Each agent&apos;s own</option>
                  {harnessOptions.map((item) => (
                    <option key={item.kind} value={item.kind}>
                      {item.name}
                    </option>
                  ))}
                  {draft.harness && !harnessOptions.some((item) => item.kind === draft.harness) ? (
                    <option value={draft.harness}>{draft.harness}</option>
                  ) : null}
                </Select>
              </Field>
              <Field label="Timeout (seconds)" htmlFor="team-timeout" optional hint="Per step. Empty uses each agent's limit.">
                <Input
                  id="team-timeout"
                  type="number"
                  min={0}
                  value={draft.timeout}
                  onChange={(event) => set("timeout", event.target.value)}
                />
              </Field>
              <Field
                label="Team autonomy"
                htmlFor="team-autonomy"
                hint="Follow each agent's own level, or make one level apply to every step and native subagent."
                className="sm:col-span-2"
              >
                <Select
                  id="team-autonomy"
                  value={draft.autonomyMode}
                  onChange={(event) => set("autonomyMode", event.target.value as Draft["autonomyMode"])}
                >
                  <option value="follow_primary">Follow each agent&apos;s level</option>
                  <option value="fixed">Fixed for this team</option>
                </Select>
              </Field>
              {draft.autonomyMode === "fixed" ? (
                <>
                  <Field
                    label="Autonomy level"
                    htmlFor="team-autonomy-level"
                    hint="A policy less strict than a step agent's own (a higher level, or an approval mode that asks less) requires manage access to that agent."
                  >
                    <Select
                      id="team-autonomy-level"
                      value={draft.autonomyLevel}
                      onChange={(event) => set("autonomyLevel", event.target.value)}
                    >
                      {AUTONOMY_LEVELS.map((level) => (
                        <option key={level.level} value={level.level}>
                          Level {level.level} — {level.name}
                        </option>
                      ))}
                    </Select>
                  </Field>
                  <Field
                    label="Approval mode"
                    htmlFor="team-autonomy-approval"
                    hint="If an agent is made stricter later, its steps run with the agent's own policy until the team is saved again by someone who can manage it."
                  >
                    <Select
                      id="team-autonomy-approval"
                      value={draft.autonomyApproval}
                      onChange={(event) => set("autonomyApproval", event.target.value as Draft["autonomyApproval"])}
                    >
                      <option value="agent">Each agent&apos;s own mode</option>
                      <option value="always">Always ask</option>
                      <option value="writes">Ask for writes</option>
                      <option value="auto">Follow the level (level 4 writes without asking)</option>
                    </Select>
                  </Field>
                </>
              ) : null}
            </div>
            <CheckboxField
              title="Start the worker automatically"
              description="Launches the chosen profile before the first step. Its allowlist must include the team's agents."
              checked={draft.autoLaunch}
              disabled={!draft.workerProfileId}
              onChange={(event) => set("autoLaunch", event.target.checked)}
            />
          </FormSection>

          {/* Kept mounted while other sections are open, so access edits survive until Save. */}
          <Card id="section-sharing" className="scroll-mt-20" hidden={active !== "sharing"}>
            <CardHeader>
              <h2 className="text-[15px] font-semibold leading-6">{SECTION_TITLES.sharing}</h2>
              <p className="text-[13px] text-muted-foreground">Who can see, run and change this team. Saved together with the team.</p>
            </CardHeader>
            <CardContent className="space-y-5">
              <ResourceAccessFields
                ref={accessRef}
                resource="agent_teams"
                resourceId={team?.id}
                onDirtyChange={setAccessDirty}
              />
              <div className="flex items-center justify-between gap-3 border-t pt-4">
                <Button type="button" variant="ghost" onClick={() => go("runtime")}>
                  <ChevronLeft /> {SECTION_TITLES.runtime}
                </Button>
              </div>
            </CardContent>
          </Card>
        </div>
      </div>

      {/* Phones and tablets: Save stays within thumb reach. */}
      <div className="fixed inset-x-0 bottom-14 z-30 border-t bg-background/95 p-3 backdrop-blur lg:hidden">
        <div className="flex items-center justify-between gap-3">
          <Button asChild variant="ghost">
            <Link href={cancelHref}>Cancel</Link>
          </Button>
          <Button onClick={() => void submit()} loading={submitting} size="lg">
            {submitLabel}
          </Button>
        </div>
      </div>
    </div>
  );
}
