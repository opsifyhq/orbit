"use client";

import { useEffect, useState, type ReactNode } from "react";
import Link from "next/link";
import { Bot, Cloud, Globe, Hash, MessageSquare, Send, Users, Webhook } from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Callout } from "@/components/ui/callout";
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
import { Textarea } from "@/components/ui/textarea";
import { OUTPUT_EVENTS, OUTPUT_KINDS, eventsLabel, kindLabel, type OutputKind } from "@/lib/output-meta";
import type { OutputInput, OutputView } from "@/lib/outputs";
import { useAgentTeams, useAllAgents } from "@/lib/queries";
import { useUnsavedChanges } from "@/lib/use-unsaved-changes";
import { titleCase } from "@/lib/utils";

type SectionId = "destination" | "trigger" | "message";

const SECTION_TITLES: Record<SectionId, string> = {
  destination: "Destination",
  trigger: "Trigger",
  message: "Message",
};
const SECTION_ORDER: SectionId[] = ["destination", "trigger", "message"];
const isSectionId = (value: string): value is SectionId => (SECTION_ORDER as string[]).includes(value);

const KIND_ICON: Record<OutputKind, ReactNode> = {
  slack: <Hash />,
  mattermost: <MessageSquare />,
  telegram: <Send />,
  webhook: <Webhook />,
  s3: <Cloud />,
};

type Scope = "all" | "agent" | "team";

type Draft = {
  name: string;
  kind: OutputKind;
  webhookUrl: string;
  botToken: string;
  chatId: string;
  messageThreadId: string;
  s3Provider: "amazon" | "rustfs";
  s3Endpoint: string;
  s3Region: string;
  s3Bucket: string;
  s3KeyPrefix: string;
  s3PathStyle: boolean;
  s3AccessKeyId: string;
  s3SecretAccessKey: string;
  s3SessionToken: string;
  scope: Scope;
  agentIds: string[];
  teamId: string;
  events: string[];
  template: string;
  enabled: boolean;
};

function draftFrom(output?: OutputView, initial?: { agentId?: string; teamId?: string }): Draft {
  const agentIds = output?.agentIds?.length
    ? output.agentIds
    : output?.agentId
      ? [output.agentId]
      : initial?.agentId
        ? [initial.agentId]
        : [];
  const teamId = output?.teamId ?? initial?.teamId ?? "";
  return {
    name: output?.name ?? "",
    kind: output?.kind ?? "slack",
    webhookUrl: "",
    botToken: "",
    chatId: output?.chatId ?? "",
    messageThreadId: output?.messageThreadId ?? "",
    s3Provider: output?.config?.provider === "rustfs" ? "rustfs" : "amazon",
    s3Endpoint: String(output?.config?.endpoint ?? ""),
    s3Region: String(output?.config?.region ?? "us-east-1"),
    s3Bucket: String(output?.config?.bucket ?? output?.config?.defaultBucket ?? ""),
    s3KeyPrefix: String(output?.config?.keyPrefix ?? ""),
    s3PathStyle: Boolean(output?.config?.pathStyle),
    s3AccessKeyId: "",
    s3SecretAccessKey: "",
    s3SessionToken: "",
    scope: teamId ? "team" : agentIds.length ? "agent" : "all",
    agentIds,
    teamId,
    events: output?.events ?? [...OUTPUT_EVENTS],
    template: output?.template ?? "",
    enabled: output?.enabled ?? true,
  };
}

interface Issue {
  section: SectionId;
  message: string;
}

function sectionOfPath(path: string): SectionId {
  if (/^(agentId|teamId|events)/.test(path)) return "trigger";
  if (/^template/.test(path)) return "message";
  return "destination";
}

/**
 * Output Studio: the same one-section-at-a-time form as Agent and Connection Studio.
 * Where the message goes, which runs send it, and what it says. Credentials are
 * write-only: they are sent when entered and never shown again.
 */
export function OutputForm({
  output,
  initial,
  submitLabel,
  submitting,
  onSubmit,
  cancelHref,
  serverIssues = [],
}: {
  output?: OutputView;
  /** Preselect the agent or team a new output applies to (`?agent=` / `?team=`). */
  initial?: { agentId?: string; teamId?: string };
  submitLabel: string;
  submitting?: boolean;
  onSubmit: (input: OutputInput) => Promise<void> | void;
  cancelHref: string;
  /** Validation details from the last failed save (`path` + `message`). */
  serverIssues?: Array<{ path: string; message: string }>;
}) {
  const editing = Boolean(output);
  const agents = useAllAgents();
  const teams = useAgentTeams();
  const [draft, setDraft] = useState<Draft>(() => draftFrom(output, initial));
  useUnsavedChanges(draft);
  const set = <K extends keyof Draft>(key: K, value: Draft[K]) => setDraft((current) => ({ ...current, [key]: value }));

  const agentList = (agents.data?.items ?? []).filter((agent) => agent.status !== "archived" || draft.agentIds.includes(agent.id));
  const teamList = teams.data ?? [];
  const agentNames = draft.agentIds
    .map((id) => agentList.find((agent) => agent.id === id)?.name)
    .filter((name): name is string => Boolean(name));
  const agentName = agentNames.length ? agentNames.join(", ") : undefined;
  const teamName = teamList.find((team) => team.id === draft.teamId)?.name;
  const storedCredential = Boolean(output?.hasWebhook) && output?.kind === draft.kind;
  const kindName = kindLabel(draft.kind);
  const isTelegram = draft.kind === "telegram";
  const isS3 = draft.kind === "s3";

  // ------------------------------------------------------------------ validation
  const issues: Issue[] = [];
  if (!draft.name.trim()) issues.push({ section: "destination", message: "Enter a name." });
  const url = draft.webhookUrl.trim();
  const botToken = draft.botToken.trim();
  const chatId = draft.chatId.trim();
  const threadId = draft.messageThreadId.trim();
  if (isTelegram) {
    if (!botToken && !storedCredential) issues.push({ section: "destination", message: "Enter the bot token." });
    if (botToken && !/^\d+:[A-Za-z0-9_-]+$/.test(botToken)) {
      issues.push({ section: "destination", message: "The bot token must look like <numeric id>:<secret>." });
    }
    if (!chatId) issues.push({ section: "destination", message: "Enter the chat id." });
    else if (!/^-?\d+$/.test(chatId) || chatId === "0" || chatId === "-0") {
      issues.push({ section: "destination", message: "The chat id must be numeric and not zero." });
    }
    if (threadId && (!/^\d+$/.test(threadId) || threadId === "0")) {
      issues.push({ section: "destination", message: "The topic id must be a positive number." });
    }
  } else if (isS3) {
    if (!draft.s3Bucket.trim()) issues.push({ section: "destination", message: "Enter the bucket name." });
    if (draft.s3Provider === "rustfs" && !draft.s3Endpoint.trim()) {
      issues.push({ section: "destination", message: "Enter the RustFS endpoint." });
    }
    if (draft.s3Endpoint.trim() && !/^https?:\/\/\S+$/i.test(draft.s3Endpoint.trim())) {
      issues.push({ section: "destination", message: "The S3 endpoint must start with https:// or http://." });
    }
    if (draft.s3KeyPrefix && !draft.s3KeyPrefix.endsWith("/")) {
      issues.push({ section: "destination", message: "The key prefix must end with /." });
    }
    if (!storedCredential && (!draft.s3AccessKeyId.trim() || !draft.s3SecretAccessKey.trim())) {
      issues.push({ section: "destination", message: "Enter the access key id and secret access key." });
    }
  } else {
    if (!url && !storedCredential) issues.push({ section: "destination", message: "Enter the webhook URL." });
    if (url && !/^https?:\/\/\S+$/i.test(url)) {
      issues.push({ section: "destination", message: "The webhook URL must start with https:// or http://." });
    }
  }
  if (draft.scope === "agent" && !draft.agentIds.length) {
    issues.push({ section: "trigger", message: "Choose at least one agent." });
  }
  if (draft.scope === "team" && !draft.teamId) issues.push({ section: "trigger", message: "Choose the agent team." });
  if (!draft.events.length) issues.push({ section: "trigger", message: "Choose at least one event." });
  for (const issue of serverIssues) issues.push({ section: sectionOfPath(issue.path), message: issue.message });
  const [showIssues, setShowIssues] = useState(false);
  const issueCount = (id: SectionId) => (showIssues ? issues.filter((issue) => issue.section === id).length : 0);

  // ------------------------------------------------------------------ sections
  const [active, setActive] = useState<SectionId>("destination");
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
  // A failed save opens the first section the server found a problem in.
  useEffect(() => {
    if (!serverIssues.length) return;
    setShowIssues(true);
    go(sectionOfPath(serverIssues[0].path));
  }, [serverIssues]);

  async function submit() {
    const local = issues.filter((issue) => !serverIssues.some((server) => server.message === issue.message));
    if (local.length) {
      setShowIssues(true);
      go(local[0].section);
      toast.error(local.length === 1 ? local[0].message : "Fix the highlighted sections");
      return;
    }
    await onSubmit({
      name: draft.name.trim(),
      kind: draft.kind,
      agentIds: draft.scope === "agent" ? draft.agentIds : [],
      teamId: draft.scope === "team" ? draft.teamId : null,
      events: OUTPUT_EVENTS.filter((event) => draft.events.includes(event)),
      ...(isTelegram
        ? {
            ...(botToken ? { botToken } : {}),
            chatId,
            messageThreadId: threadId || null,
          }
        : isS3
          ? {
              ...(draft.s3AccessKeyId.trim() || draft.s3SecretAccessKey.trim()
                ? {
                    s3Credential: JSON.stringify({
                      accessKeyId: draft.s3AccessKeyId.trim(),
                      secretAccessKey: draft.s3SecretAccessKey.trim(),
                      ...(draft.s3SessionToken.trim() ? { sessionToken: draft.s3SessionToken.trim() } : {}),
                    }),
                  }
                : {}),
              config: {
                provider: draft.s3Provider,
                endpoint: draft.s3Endpoint.trim() || undefined,
                region: draft.s3Region.trim() || undefined,
                bucket: draft.s3Bucket.trim(),
                keyPrefix: draft.s3KeyPrefix.trim(),
                pathStyle: draft.s3PathStyle,
              },
            }
          : url
            ? { webhookUrl: url }
            : {}),
      template: draft.template.trim() || null,
      enabled: draft.enabled,
    });
  }

  // ------------------------------------------------------------------ summaries
  const scopeValue =
    draft.scope === "team"
      ? teamName ?? "Team not chosen"
      : draft.scope === "agent"
        ? agentName ?? "Agents not chosen"
        : "Every agent";
  const destinationDetail = isTelegram
    ? chatId
      ? `${chatId}${threadId ? ` · topic ${threadId}` : ""}`
      : "chat not set"
    : url
      ? "new URL"
      : storedCredential
        ? "credential stored"
        : "no URL";
  const summary: Record<SectionId, { value: string; missing?: boolean }> = {
    destination: {
      value: draft.name.trim() ? `${kindName} · ${destinationDetail}` : "Name not set",
      missing:
        !draft.name.trim() ||
        (isTelegram
          ? !chatId || (!botToken && !storedCredential)
          : !url && !storedCredential),
    },
    trigger: {
      value: `${scopeValue} · ${eventsLabel(draft.events).toLowerCase()}`,
      missing: !draft.events.length || (draft.scope !== "all" && scopeValue.endsWith("not chosen")),
    },
    message: { value: `${draft.template.trim() ? "Summary and note" : "Run summary"} · ${draft.enabled ? "on" : "off"}` },
  };
  const navSections: NavSection<SectionId>[] = SECTION_ORDER.map((id) => ({
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
      {...sectionNeighbours(SECTION_ORDER, SECTION_TITLES, id)}
    >
      {sectionIssues(id)}
      {children}
    </SectionCard>
  );

  // Mirrors the server's summary: "<agent> <status> — <task>", the output, then the note.
  const headline = draft.scope === "team" ? "Agent team run completed" : `${agentName ?? "Release notes"} succeeded`;
  const summaryText = `${headline} — Summarise this week's releases\nThree releases shipped: …`;
  const preview = draft.template.trim() ? `${summaryText}\n\n${draft.template.trim()}` : summaryText;

  return (
    <div className="pb-24 lg:pb-0">
      {/* Phones and tablets: the sections as a scrollable row of steps. */}
      <SectionPills label="Output sections" sections={navSections} active={active} onGo={go} className="mb-4" />

      <div className="grid gap-6 lg:grid-cols-[15rem_minmax(0,1fr)] lg:items-start">
        <aside className="hidden lg:sticky lg:top-6 lg:block">
          <SectionRail label="Output sections" sections={navSections} active={active} onGo={go} wrapValues />
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
            <p className="text-[13px] text-muted-foreground">Webhook URLs and bot tokens are encrypted at rest and never shown again.</p>
          </div>
        </aside>

        <div className="min-w-0 max-w-3xl space-y-5">
          {section(
            "destination",
            "Where the message goes.",
            <>
              <div role="radiogroup" aria-label="Destination" className="grid gap-2 sm:grid-cols-2 lg:grid-cols-4">
                {OUTPUT_KINDS.map((item) => (
                  <ChoiceCard
                    key={item.kind}
                    selected={draft.kind === item.kind}
                    onSelect={() => set("kind", item.kind)}
                    icon={KIND_ICON[item.kind]}
                    title={item.label}
                    description={item.description}
                  />
                ))}
              </div>
              <Field label="Name" htmlFor="output-name" hint="Shown in the list and when starting work.">
                <Input
                  id="output-name"
                  value={draft.name}
                  placeholder="Engineering alerts"
                  onChange={(event) => set("name", event.target.value)}
                />
              </Field>
              {isTelegram ? (
                <>
                  <Callout tone="info" title="Telegram bot setup">
                    Create a bot with @BotFather, add it to the target group or channel (as an
                    administrator for channels), then copy its numeric chat id. The token is
                    stored encrypted and never shown again.
                  </Callout>
                  <Field
                    label="Bot token"
                    htmlFor="output-bot-token"
                    hint={
                      storedCredential
                        ? "A token is stored. Leave this empty to keep it, or enter a new one to replace it."
                        : "Create one with @BotFather. Stored encrypted; you will not see it again."
                    }
                  >
                    <Input
                      id="output-bot-token"
                      type="password"
                      autoComplete="new-password"
                      spellCheck={false}
                      className="font-mono"
                      value={draft.botToken}
                      placeholder={storedCredential ? "••••••••  (stored)" : "123456:ABC-def"}
                      onChange={(event) => set("botToken", event.target.value)}
                    />
                  </Field>
                  <div className="grid gap-3 sm:grid-cols-2">
                    <Field
                      label="Chat id"
                      htmlFor="output-chat-id"
                      hint="Numeric user, group or channel id, for example -1001234567890."
                    >
                      <Input
                        id="output-chat-id"
                        inputMode="numeric"
                        value={draft.chatId}
                        placeholder="-1001234567890"
                        onChange={(event) => set("chatId", event.target.value)}
                      />
                    </Field>
                    <Field
                      label="Message thread id"
                      optional
                      htmlFor="output-thread-id"
                      hint="Forum topic id, if the chat uses topics."
                    >
                      <Input
                        id="output-thread-id"
                        inputMode="numeric"
                        value={draft.messageThreadId}
                        placeholder="42"
                        onChange={(event) => set("messageThreadId", event.target.value)}
                      />
                    </Field>
                  </div>
                </>
              ) : isS3 ? (
                <>
                  <Callout tone="info" title="S3-compatible object storage">
                    Amazon S3 uses the regional endpoint and virtual-hosted addressing. RustFS
                    normally needs its own endpoint and path-style addressing. Credentials are
                    stored encrypted and never shown again.
                  </Callout>
                  <div className="grid gap-3 sm:grid-cols-2">
                    <Field label="Provider" htmlFor="output-s3-provider">
                      <Select
                        id="output-s3-provider"
                        value={draft.s3Provider}
                        onChange={(event) => set("s3Provider", event.target.value as Draft["s3Provider"])}
                      >
                        <option value="amazon">Amazon S3</option>
                        <option value="rustfs">RustFS (S3-compatible)</option>
                      </Select>
                    </Field>
                    <Field label="Bucket" htmlFor="output-s3-bucket">
                      <Input
                        id="output-s3-bucket"
                        value={draft.s3Bucket}
                        placeholder="orbit-runs"
                        onChange={(event) => set("s3Bucket", event.target.value)}
                      />
                    </Field>
                    <Field
                      label="Endpoint"
                      htmlFor="output-s3-endpoint"
                      optional={draft.s3Provider === "amazon"}
                      hint={draft.s3Provider === "rustfs" ? "Required for RustFS." : "Optional VPC/custom endpoint."}
                    >
                      <Input
                        id="output-s3-endpoint"
                        className="font-mono"
                        value={draft.s3Endpoint}
                        placeholder="https://s3.example.com"
                        onChange={(event) => set("s3Endpoint", event.target.value)}
                      />
                    </Field>
                    <Field label="Region" htmlFor="output-s3-region">
                      <Input
                        id="output-s3-region"
                        value={draft.s3Region}
                        placeholder="us-east-1"
                        onChange={(event) => set("s3Region", event.target.value)}
                      />
                    </Field>
                    <Field
                      label="Key prefix"
                      htmlFor="output-s3-prefix"
                      optional
                      hint="Objects are written as <prefix><delivery-id>.txt. End with /."
                      className="sm:col-span-2"
                    >
                      <Input
                        id="output-s3-prefix"
                        className="font-mono"
                        value={draft.s3KeyPrefix}
                        placeholder="orbit/runs/"
                        onChange={(event) => set("s3KeyPrefix", event.target.value)}
                      />
                    </Field>
                  </div>
                  <CheckboxField
                    title="Path-style addressing"
                    description="Required for most RustFS deployments; leave off for Amazon S3."
                    checked={draft.s3PathStyle}
                    onChange={(event) => set("s3PathStyle", event.target.checked)}
                  />
                  <div className="grid gap-3 sm:grid-cols-2">
                    <Field
                      label="Access key id"
                      htmlFor="output-s3-access-key"
                      hint={storedCredential ? "A credential is stored. Leave these empty to keep it." : undefined}
                    >
                      <Input
                        id="output-s3-access-key"
                        className="font-mono"
                        value={draft.s3AccessKeyId}
                        onChange={(event) => set("s3AccessKeyId", event.target.value)}
                      />
                    </Field>
                    <Field label="Secret access key" htmlFor="output-s3-secret-key">
                      <Input
                        id="output-s3-secret-key"
                        type="password"
                        autoComplete="new-password"
                        className="font-mono"
                        value={draft.s3SecretAccessKey}
                        placeholder={storedCredential ? "••••••••  (stored)" : undefined}
                        onChange={(event) => set("s3SecretAccessKey", event.target.value)}
                      />
                    </Field>
                    <Field label="Session token" htmlFor="output-s3-session-token" optional className="sm:col-span-2">
                      <Input
                        id="output-s3-session-token"
                        type="password"
                        autoComplete="new-password"
                        className="font-mono"
                        value={draft.s3SessionToken}
                        onChange={(event) => set("s3SessionToken", event.target.value)}
                      />
                    </Field>
                  </div>
                </>
              ) : (
                <Field
                  label={draft.kind === "webhook" ? "Webhook URL" : `${kindName} incoming webhook URL`}
                  htmlFor="output-webhook"
                  hint={
                    storedCredential
                      ? "A URL is stored. Leave this empty to keep it, or enter a new one to replace it."
                      : draft.kind === "webhook"
                        ? "Stored encrypted; you will not see it again."
                        : `Create one under your ${kindName} integrations. Stored encrypted; you will not see it again.`
                  }
                >
                  <Input
                    id="output-webhook"
                    type="password"
                    autoComplete="new-password"
                    spellCheck={false}
                    className="font-mono"
                    value={draft.webhookUrl}
                    placeholder={storedCredential ? "••••••••  (stored)" : "https://…"}
                    onChange={(event) => set("webhookUrl", event.target.value)}
                  />
                </Field>
              )}
            </>,
          )}

          {section(
            "trigger",
            "Which runs send a message, and when.",
            <>
              <fieldset className="space-y-2">
                <legend className="text-[13px] font-medium">Applies to</legend>
                <div role="radiogroup" aria-label="Applies to" className="grid gap-2 sm:grid-cols-3">
                  <ChoiceCard
                    selected={draft.scope === "all"}
                    onSelect={() => set("scope", "all")}
                    icon={<Globe />}
                    title="Every agent"
                    description="One message per run of any agent."
                  />
                  <ChoiceCard
                    selected={draft.scope === "agent"}
                    onSelect={() => set("scope", "agent")}
                    icon={<Bot />}
                    title="Specific agents"
                    description="One message per run of each selected agent."
                  />
                  <ChoiceCard
                    selected={draft.scope === "team"}
                    onSelect={() => set("scope", "team")}
                    icon={<Users />}
                    title="One agent team"
                    description="One message when the whole team run finishes."
                  />
                </div>
              </fieldset>
              {draft.scope === "agent" ? (
                <div className="space-y-2">
                  <p className="text-[13px] font-medium">Agents</p>
                  {agents.isLoading ? (
                    <p className="text-[13px] text-muted-foreground">Loading agents…</p>
                  ) : (
                    <div className="grid max-h-60 gap-2 overflow-y-auto sm:grid-cols-2">
                      {agentList.map((agent) => (
                        <CheckboxField
                          key={agent.id}
                          title={agent.name}
                          checked={draft.agentIds.includes(agent.id)}
                          onChange={(event) =>
                            set(
                              "agentIds",
                              event.target.checked
                                ? [...draft.agentIds, agent.id]
                                : draft.agentIds.filter((id) => id !== agent.id),
                            )
                          }
                        />
                      ))}
                    </div>
                  )}
                </div>
              ) : null}
              {draft.scope === "team" ? (
                <Field label="Agent team" htmlFor="output-team">
                  <Select id="output-team" value={draft.teamId} onChange={(event) => set("teamId", event.target.value)}>
                    <option value="">{teams.isLoading ? "Loading teams…" : "Choose a team"}</option>
                    {teamList.map((team) => (
                      <option key={team.id} value={team.id}>
                        {team.name}
                      </option>
                    ))}
                  </Select>
                </Field>
              ) : null}
              <fieldset className="space-y-2">
                <legend className="text-[13px] font-medium">Send when a run</legend>
                <div className="grid gap-2 sm:grid-cols-3">
                  {OUTPUT_EVENTS.map((event) => (
                    <CheckboxField
                      key={event}
                      title={titleCase(event)}
                      checked={draft.events.includes(event)}
                      onChange={(changed) =>
                        set(
                          "events",
                          changed.target.checked ? [...draft.events, event] : draft.events.filter((item) => item !== event),
                        )
                      }
                    />
                  ))}
                </div>
              </fieldset>
            </>,
          )}

          {section(
            "message",
            "What the message says. ORBIT writes the run summary; you can add a note under it.",
            <>
              <Field label="Note" optional htmlFor="output-template" hint="Added under the run summary, for example who should look or a runbook link.">
                <Textarea
                  id="output-template"
                  className="min-h-20"
                  value={draft.template}
                  placeholder="@release-team please review"
                  onChange={(event) => set("template", event.target.value)}
                />
              </Field>
              <div className="space-y-1.5">
                <p className="text-[13px] font-medium">Example</p>
                <pre className="whitespace-pre-wrap break-words rounded-lg border bg-subtle p-3 font-mono text-xs leading-5 text-muted-foreground">
                  {draft.kind === "webhook"
                    ? JSON.stringify(
                        {
                          event: draft.scope === "team" ? "team_run.completed" : "execution.succeeded",
                          deliveryId: "…",
                          text: preview,
                          [draft.scope === "team" ? "teamRun" : "execution"]: { id: "…", status: draft.scope === "team" ? "completed" : "succeeded" },
                        },
                        null,
                        2,
                      )
                    : preview}
                </pre>
                <p className="text-xs text-muted-foreground">The summary uses the run&apos;s task and up to 3,000 characters of its output or error.</p>
              </div>
              <CheckboxField
                title="On"
                description="Turn off to keep the output without sending anything. Runs that finish while it is off are not sent later."
                checked={draft.enabled}
                onChange={(event) => set("enabled", event.target.checked)}
              />
            </>,
          )}
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
