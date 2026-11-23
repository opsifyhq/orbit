"use client";

import { useEffect, useRef, useState, type ReactNode } from "react";
import Link from "next/link";
import { Boxes, ChevronLeft, Cpu, Server, Zap } from "lucide-react";
import { toast } from "sonner";

import {
  ResourceAccessFields,
  type ResourceAccessHandle,
  type ResourceSubmit,
} from "@/components/resource-access-fields";
import { Button } from "@/components/ui/button";
import { Callout } from "@/components/ui/callout";
import { Card, CardContent, CardHeader } from "@/components/ui/card";
import { CheckboxField } from "@/components/ui/checkbox";
import { ChoiceCard } from "@/components/ui/choice-card";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import {
  SectionCard,
  SectionPills,
  SectionRail,
  sectionNeighbours,
  type NavSection,
} from "@/components/ui/section-nav";
import { Select } from "@/components/ui/select";
import { Status } from "@/components/ui/status";
import { Textarea } from "@/components/ui/textarea";
import {
  ANTHROPIC_BASE_URL,
  COMPAT_FLAGS,
  EMPTY_DRAFT,
  HARNESS_LABEL,
  KIND_LABEL,
  PROVIDER_PRESETS,
  draftFrom,
  draftIssues,
  parseModels,
  plural,
  type ProviderDraft,
  type ProviderKind,
  type TestResult,
} from "@/lib/provider-meta";
import { testDraft, useTestProvider } from "@/lib/providers";
import type { ProviderView } from "@/lib/types";
import { useUnsavedChanges } from "@/lib/use-unsaved-changes";
import { cn } from "@/lib/utils";

type SectionId = "type" | "connection" | "models" | "limits" | "sharing";

const SECTION_TITLES: Record<SectionId, string> = {
  type: "Type",
  connection: "Connection",
  models: "Models",
  limits: "Limits",
  sharing: "Access control",
};
const isSectionId = (value: string): value is SectionId => value in SECTION_TITLES;

const KIND_CHOICES: Array<{ kind: ProviderKind; icon: ReactNode; description: string }> = [
  { kind: "openai_compatible", icon: <Server />, description: "vLLM, LiteLLM, a gateway, or any /v1 API." },
  { kind: "anthropic", icon: <Cpu />, description: "The native Claude Messages API." },
  { kind: "harness_managed", icon: <Boxes />, description: "The harness signs in itself, for example OpenCode Go." },
];

function sectionOfPath(path: string): SectionId {
  if (/^(name|credential|config\.(baseUrl|harness|provider))/.test(path)) return "connection";
  if (/^config\.(model|settings)/.test(path)) return "models";
  if (/^config\.(contextWindow|maxOutputTokens|compat)/.test(path)) return "limits";
  if (/^kind/.test(path)) return "type";
  return "connection";
}

function testStatus(result: TestResult) {
  return result.ok
    ? { label: `Reachable in ${result.latencyMs ?? 0} ms`, tone: "success" as const, shape: "solid" as const }
    : { label: "Test failed", tone: "danger" as const, shape: "cross" as const };
}

/**
 * Provider Studio: the same one-section-at-a-time form as Agent and Connection Studio.
 * Pick the type, say how to reach it, then choose its models. The credential is
 * write-only: it is sent when entered and never shown.
 */
export function ProviderForm({
  provider,
  submitLabel,
  submitting,
  onSubmit,
  cancelHref,
  serverIssues = [],
}: {
  provider?: ProviderView;
  submitLabel: string;
  submitting?: boolean;
  onSubmit: ResourceSubmit<ProviderDraft>;
  cancelHref: string;
  serverIssues?: Array<{ path: string; message: string }>;
}) {
  const editing = Boolean(provider);
  const [draft, setDraft] = useState<ProviderDraft>(() => (provider ? draftFrom(provider) : EMPTY_DRAFT));
  const [modelsText, setModelsText] = useState(() => draft.models.join("\n"));
  const [accessDirty, setAccessDirty] = useState(false);
  useUnsavedChanges({ draft, modelsText, accessDirty });
  const set = <K extends keyof ProviderDraft>(key: K, value: ProviderDraft[K]) =>
    setDraft((current) => ({ ...current, [key]: value }));
  const accessRef = useRef<ResourceAccessHandle>(null);
  const testSaved = useTestProvider();
  const [testing, setTesting] = useState(false);
  const [lastTest, setLastTest] = useState<TestResult | null>(null);

  const models = parseModels(modelsText);
  const isEndpoint = draft.kind !== "harness_managed";
  const storedBaseUrl = provider ? String((provider.config as Record<string, unknown> | undefined)?.baseUrl ?? "") : "";
  const baseUrlChanged = editing && draft.baseUrl.trim().replace(/\/$/, "") !== storedBaseUrl.replace(/\/$/, "");
  const order: SectionId[] = isEndpoint
    ? ["type", "connection", "models", "limits", "sharing"]
    : ["type", "connection", "models", "sharing"];

  // ------------------------------------------------------------------ validation
  const issues: Array<{ section: SectionId; message: string }> = draftIssues({ ...draft, models });
  if (isEndpoint && editing && provider?.hasCredential && baseUrlChanged && !draft.credential) {
    issues.push({
      section: "connection",
      message: "Enter the credential again: a stored key is never sent to a new base URL.",
    });
  }
  for (const issue of serverIssues) issues.push({ section: sectionOfPath(issue.path), message: issue.message });
  const [showIssues, setShowIssues] = useState(false);
  const issueCount = (id: SectionId) => (showIssues ? issues.filter((issue) => issue.section === id).length : 0);

  // ------------------------------------------------------------------ sections
  const [active, setActive] = useState<SectionId>(editing ? "connection" : "type");
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
      ?.querySelector<HTMLElement>("[role=radio][aria-checked=true], input:not([type=hidden]):not(:disabled), textarea, select")
      ?.focus({ preventScroll: true });
  }, [focusRequest, active]);
  function go(id: SectionId) {
    setActive(id);
    setFocusRequest((count) => count + 1);
    window.history.replaceState(null, "", `#${id}`);
  }
  // The Limits section does not exist for a harness-managed provider.
  useEffect(() => {
    if (!order.includes(active)) setActive("connection");
  }, [order, active]);
  useEffect(() => {
    if (!serverIssues.length) return;
    setShowIssues(true);
    go(sectionOfPath(serverIssues[0].path));
  }, [serverIssues]);

  function chooseKind(kind: ProviderKind) {
    if (editing || kind === draft.kind) return;
    setDraft((current) => ({
      ...current,
      kind,
      baseUrl: kind === "anthropic" ? ANTHROPIC_BASE_URL : kind === "openai_compatible" ? "" : current.baseUrl,
      model: "",
    }));
    setLastTest(null);
  }

  async function runTest() {
    setTesting(true);
    try {
      // A saved provider with its stored credential and URL is tested as saved; anything
      // changed is tested as typed.
      const outcome =
        provider && !draft.credential && !baseUrlChanged ? await testSaved(provider.id) : await testDraft({ ...draft, models });
      setLastTest(outcome);
      if (outcome.ok) {
        toast.success(outcome.summary);
        if (outcome.models.length && !models.length) setModelsText(outcome.models.join("\n"));
        if (outcome.models.length && !draft.model) set("model", outcome.models[0]);
      } else {
        toast.error(outcome.summary);
      }
    } finally {
      setTesting(false);
    }
  }

  async function submit() {
    const local = issues.filter((issue) => !serverIssues.some((server) => server.message === issue.message));
    if (local.length) {
      setShowIssues(true);
      go(local[0].section);
      toast.error(local.length === 1 ? local[0].message : "Fix the highlighted sections");
      return;
    }
    const commitAccess = async () => {
      await accessRef.current?.save();
    };
    await onSubmit({ ...draft, models }, commitAccess);
  }

  // ------------------------------------------------------------------ summaries
  const preset = PROVIDER_PRESETS.find((item) => item.baseUrl && item.baseUrl === draft.baseUrl.trim());
  const summary: Record<SectionId, { value: string; missing?: boolean }> = {
    type: { value: preset ? `${KIND_LABEL[draft.kind]} · ${preset.label}` : KIND_LABEL[draft.kind] },
    connection: {
      value: draft.name.trim()
        ? `${draft.name.trim()} · ${draft.credential ? "new credential" : provider?.hasCredential ? "credential stored" : "no credential"}`
        : "Name not set",
      missing: !draft.name.trim() || (draft.kind === "openai_compatible" && !draft.baseUrl.trim()),
    },
    models: {
      value: `${draft.model || (isEndpoint ? "No default" : "Harness default")} · ${plural(models.length, "model", "models")}`,
    },
    limits: {
      value:
        [draft.contextWindow && `${Number(draft.contextWindow).toLocaleString()} context`, draft.tools && "tools"]
          .filter(Boolean)
          .join(" · ") || "Endpoint defaults",
    },
    sharing: { value: editing ? "Who can see and use it" : "After the first save" },
  };
  const navSections: NavSection<SectionId>[] = order.map((id) => ({
    id,
    title: SECTION_TITLES[id],
    value: summary[id].value,
    missing: summary[id].missing,
    problems: issueCount(id),
  }));

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

  const section = (id: SectionId, description: string, children: ReactNode) => (
    <SectionCard
      id={id}
      active={active}
      title={SECTION_TITLES[id]}
      description={description}
      onGo={go}
      {...sectionNeighbours(order, SECTION_TITLES, id)}
    >
      {sectionIssues(id)}
      {children}
    </SectionCard>
  );

  const credentialHint = editing
    ? provider?.hasCredential
      ? baseUrlChanged
        ? "The base URL changed, so enter the credential again."
        : "A credential is stored. Leave this empty to keep it, or enter a new one to replace it."
      : isEndpoint
        ? "No credential is stored. Enter one to add it."
        : "No credential is stored. The harness normally signs in itself."
    : isEndpoint
      ? "An API key, sent as a bearer token. Stored encrypted; you will not see it again. Local endpoints may need none."
      : "The harness normally signs in itself. Stored encrypted if you add one.";

  const testControl = isEndpoint ? (
    <div className="flex flex-wrap items-center gap-3 rounded-lg border bg-subtle p-3">
      <Button type="button" variant="outline" onClick={() => void runTest()} loading={testing} disabled={!draft.baseUrl.trim()}>
        <Zap /> Test connection
      </Button>
      {lastTest ? (
        <div className="min-w-0 space-y-0.5">
          <Status meta={testStatus(lastTest)} />
          {lastTest.ok ? (
            <p className="text-xs text-muted-foreground">{plural(lastTest.modelCount ?? 0, "model", "models")} found</p>
          ) : (
            <p className="break-words text-xs text-destructive">{lastTest.error}</p>
          )}
        </div>
      ) : (
        <p className="text-[13px] text-muted-foreground">Lists the endpoint&apos;s models; nothing is saved.</p>
      )}
    </div>
  ) : null;

  return (
    <div className="pb-24 lg:pb-0">
      <SectionPills label="Provider sections" sections={navSections} active={active} onGo={go} className="mb-4" />

      <div className="grid gap-6 lg:grid-cols-[15rem_minmax(0,1fr)] lg:items-start">
        <aside className="hidden lg:sticky lg:top-6 lg:block">
          <SectionRail label="Provider sections" sections={navSections} active={active} onGo={go} wrapValues />
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
            <p className="text-[13px] text-muted-foreground">Credentials are encrypted at rest and never shown again.</p>
          </div>
        </aside>

        <div className="min-w-0 max-w-3xl space-y-5">
          {section(
            "type",
            editing ? "How agents reach this provider. It cannot change after creation." : "How agents will reach models.",
            <>
              <div role="radiogroup" aria-label="Provider type" className="grid gap-2 sm:grid-cols-3">
                {KIND_CHOICES.map((choice) => (
                  <ChoiceCard
                    key={choice.kind}
                    selected={draft.kind === choice.kind}
                    disabled={editing && draft.kind !== choice.kind}
                    onSelect={() => chooseKind(choice.kind)}
                    icon={choice.icon}
                    title={KIND_LABEL[choice.kind]}
                    description={choice.description}
                  />
                ))}
              </div>
              {draft.kind === "openai_compatible" ? (
                <fieldset className="space-y-2">
                  <legend className="text-[13px] font-medium">Start from</legend>
                  <div role="radiogroup" aria-label="Start from" className="grid gap-2 sm:grid-cols-2 lg:grid-cols-3">
                    {PROVIDER_PRESETS.map((item) => (
                      <ChoiceCard
                        key={item.id}
                        selected={item.id === "custom" ? !preset : preset?.id === item.id}
                        onSelect={() => {
                          set("baseUrl", item.baseUrl);
                          if (!editing && (!draft.name.trim() || PROVIDER_PRESETS.some((p) => p.label === draft.name))) {
                            set("name", item.id === "custom" ? "" : item.label);
                          }
                        }}
                        title={item.label}
                        description={item.hint}
                      />
                    ))}
                  </div>
                  <p className="text-xs text-muted-foreground">A preset only fills the base URL. You can change it under Connection.</p>
                </fieldset>
              ) : null}
            </>,
          )}

          {section(
            "connection",
            isEndpoint ? "Where the endpoint is and how ORBIT signs in." : "Which harness owns this provider and its sign-in.",
            <>
              <Field label="Name" htmlFor="provider-name" hint="Agents and worker profiles pick a provider by this name.">
                <Input id="provider-name" value={draft.name} placeholder="Company gateway" onChange={(event) => set("name", event.target.value)} />
              </Field>
              {isEndpoint ? (
                <Field
                  label="Base URL"
                  htmlFor="provider-base-url"
                  hint={
                    draft.kind === "anthropic"
                      ? "The Messages API root. Change it only for a proxy."
                      : "The root of the API, usually ending in /v1. Any compatible gateway works."
                  }
                >
                  <Input
                    id="provider-base-url"
                    className="font-mono"
                    spellCheck={false}
                    value={draft.baseUrl}
                    placeholder={draft.kind === "anthropic" ? ANTHROPIC_BASE_URL : "https://gateway.example.com/v1"}
                    onChange={(event) => set("baseUrl", event.target.value)}
                  />
                </Field>
              ) : (
                <div className="grid gap-4 sm:grid-cols-2">
                  <Field label="Harness" htmlFor="provider-harness">
                    <Select id="provider-harness" value={draft.harnessKind} onChange={(event) => set("harnessKind", event.target.value)}>
                      {Object.entries(HARNESS_LABEL).map(([value, label]) => (
                        <option key={value} value={value}>
                          {label}
                        </option>
                      ))}
                    </Select>
                  </Field>
                  <Field label="Provider name in the harness" htmlFor="provider-harness-name" hint="As the harness calls it.">
                    <Input
                      id="provider-harness-name"
                      className="font-mono"
                      value={draft.harnessProvider}
                      placeholder="opencode-go"
                      onChange={(event) => set("harnessProvider", event.target.value)}
                    />
                  </Field>
                </div>
              )}
              <Field label="Credential" htmlFor="provider-credential" optional hint={credentialHint}>
                <Input
                  id="provider-credential"
                  type="password"
                  autoComplete="new-password"
                  value={draft.credential}
                  placeholder={editing && provider?.hasCredential && !baseUrlChanged ? "••••••••  (stored)" : undefined}
                  onChange={(event) => set("credential", event.target.value)}
                />
              </Field>
              {testControl}
              <CheckboxField
                title="Enabled"
                description="Turn off to keep the provider without letting agents use it. Their runs fail until you turn it on or pick another."
                checked={draft.enabled}
                onChange={(event) => set("enabled", event.target.checked)}
              />
            </>,
          )}

          {section(
            "models",
            isEndpoint
              ? "The models agents can pick, and the one they get when they pick none."
              : "Models agents can pick. Leave the list empty to allow any model the harness defines.",
            <>
              <Field
                label="Model catalog"
                htmlFor="provider-models"
                optional
                hint={
                  isEndpoint
                    ? "One per line, or separated by commas. Test connection under Connection fills it from the endpoint."
                    : "One per line, or separated by commas."
                }
              >
                <Textarea
                  id="provider-models"
                  className="min-h-28 font-mono text-base md:text-xs"
                  spellCheck={false}
                  value={modelsText}
                  placeholder={isEndpoint ? "qwen3-coder\nllama-3.3-70b" : "deepseek-v4-pro\nclaude-sonnet-4"}
                  onChange={(event) => setModelsText(event.target.value)}
                />
              </Field>
              {isEndpoint ? (
                <Button type="button" variant="outline" size="sm" onClick={() => void runTest()} loading={testing} disabled={!draft.baseUrl.trim()}>
                  <Zap /> Fetch from the endpoint
                </Button>
              ) : null}
              <Field
                label="Default model"
                htmlFor="provider-model"
                optional={!isEndpoint}
                hint={isEndpoint ? "Used when an agent does not pick a model." : "Leave empty for the harness default."}
              >
                {models.length ? (
                  <Select id="provider-model" value={draft.model} onChange={(event) => set("model", event.target.value)}>
                    <option value="">{isEndpoint ? "Choose a model" : "Harness default"}</option>
                    {draft.model && !models.includes(draft.model) ? <option value={draft.model}>{draft.model} (not in the catalog)</option> : null}
                    {models.map((candidate) => (
                      <option key={candidate} value={candidate}>
                        {candidate}
                      </option>
                    ))}
                  </Select>
                ) : (
                  <Input
                    id="provider-model"
                    className="font-mono"
                    value={draft.model}
                    placeholder={isEndpoint ? "qwen3-coder" : "Harness default"}
                    onChange={(event) => set("model", event.target.value)}
                  />
                )}
              </Field>
            </>,
          )}

          {isEndpoint
            ? section(
                "limits",
                "Optional. What the endpoint accepts, so harnesses can size requests.",
                <>
                  <div className="grid gap-4 sm:grid-cols-2">
                    <Field label="Context window" htmlFor="provider-context" optional hint="In tokens.">
                      <Input
                        id="provider-context"
                        type="number"
                        inputMode="numeric"
                        min={1}
                        value={draft.contextWindow}
                        onChange={(event) => set("contextWindow", event.target.value)}
                      />
                    </Field>
                    <Field label="Max output" htmlFor="provider-max-output" optional hint="In tokens.">
                      <Input
                        id="provider-max-output"
                        type="number"
                        inputMode="numeric"
                        min={1}
                        value={draft.maxOutputTokens}
                        onChange={(event) => set("maxOutputTokens", event.target.value)}
                      />
                    </Field>
                  </div>
                  {draft.kind === "openai_compatible" ? (
                    <fieldset className="space-y-2">
                      <legend className="text-[13px] font-medium">The endpoint supports</legend>
                      <div className="grid gap-2 sm:grid-cols-2">
                        {COMPAT_FLAGS.map((flag) => (
                          <CheckboxField
                            key={flag.key}
                            title={flag.title}
                            description={flag.description}
                            checked={draft[flag.key]}
                            onChange={(event) => set(flag.key, event.target.checked)}
                          />
                        ))}
                      </div>
                    </fieldset>
                  ) : null}
                </>,
              )
            : null}

          {/* Kept mounted while hidden so its pending edits are still saved with the provider. */}
          <div className={cn(active !== "sharing" && "hidden")}>
            <Card id="section-sharing" className="scroll-mt-20" aria-labelledby="section-sharing-title">
              <CardHeader>
                <h2 id="section-sharing-title" className="text-[15px] font-semibold leading-6">
                  {SECTION_TITLES.sharing}
                </h2>
                <p className="text-[13px] text-muted-foreground">Who can see this provider and use it for agents.</p>
              </CardHeader>
              <CardContent className="space-y-5">
                <ResourceAccessFields
                  ref={accessRef}
                  resource="providers"
                  resourceId={provider?.id}
                  onDirtyChange={setAccessDirty}
                />
                <div className="flex items-center justify-between gap-3 border-t pt-4">
                  <Button type="button" variant="ghost" onClick={() => go(order[order.length - 2])}>
                    <ChevronLeft /> {SECTION_TITLES[order[order.length - 2]]}
                  </Button>
                </div>
              </CardContent>
            </Card>
          </div>
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
