"use client";

import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import Link from "next/link";
import { ChevronLeft, Plug } from "lucide-react";
import { toast } from "sonner";

import {
  ResourceAccessFields,
  type ResourceAccessHandle,
  type ResourceSubmit,
} from "@/components/resource-access-fields";
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
import { Card, CardContent, CardHeader } from "@/components/ui/card";
import { CheckboxField } from "@/components/ui/checkbox";
import { ChoiceCard } from "@/components/ui/choice-card";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { setConfigField } from "@/lib/connection-config";
import {
  EFFECT_GROUPS,
  URL_CONNECTORS,
  URL_KEYS,
  applyUrl,
  defaultConfig,
  urlError,
  urlFromConfig,
  type Capability,
  type ConfigSchema,
  type ConnectionInput,
} from "@/lib/connections";
import { effectLabel, effectTone, humanizeIdentifier } from "@/lib/presentation";
import type { ConnectionView } from "@/lib/types";
import { useUnsavedChanges } from "@/lib/use-unsaved-changes";
import { cn } from "@/lib/utils";

type SectionId = "service" | "account" | "capabilities" | "advanced" | "sharing";

const SECTION_TITLES: Record<SectionId, string> = {
  service: "Service",
  account: "Account",
  capabilities: "Capabilities",
  advanced: "Advanced",
  sharing: "Access control",
};
const SECTION_ORDER: SectionId[] = ["service", "account", "capabilities", "advanced", "sharing"];
const isSectionId = (value: string): value is SectionId => (SECTION_ORDER as string[]).includes(value);

type Connector = {
  slug: string;
  name: string;
  description: string;
  capabilities: unknown;
  configSchema: unknown;
  auth: unknown;
};

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

/**
 * Connection Studio: the same one-section-at-a-time form as Agent Studio. Pick the
 * service, give the account and its credential, then allow capabilities by what they
 * can do. The credential is write-only: it is sent when entered and never shown.
 */
export function ConnectionForm({
  connection,
  connectors,
  initialConnector,
  submitLabel,
  submitting,
  onSubmit,
  cancelHref,
}: {
  connection?: ConnectionView;
  connectors: Connector[];
  initialConnector?: string;
  submitLabel: string;
  submitting?: boolean;
  onSubmit: ResourceSubmit<ConnectionInput>;
  cancelHref: string;
}) {
  const editing = Boolean(connection);
  const firstSlug =
    connection?.connectorSlug ??
    (initialConnector && connectors.some((item) => item.slug === initialConnector) ? initialConnector : connectors[0]?.slug ?? "");
  const startConfig = connection ? ((connection.config ?? {}) as Record<string, unknown>) : defaultConfig(firstSlug);

  const [connectorSlug, setConnectorSlug] = useState(firstSlug);
  const [name, setName] = useState(connection?.name ?? "");
  const [credential, setCredential] = useState("");
  const [scopes, setScopes] = useState<string[]>(connection?.scopes ?? []);
  const [configText, setConfigText] = useState(JSON.stringify(startConfig, null, 2));
  const [configError, setConfigError] = useState<string | null>(null);
  const [url, setUrl] = useState(urlFromConfig(firstSlug, startConfig));
  // Access is stored separately; keep pending access edits in the unsaved state too.
  const [accessDirty, setAccessDirty] = useState(false);
  useUnsavedChanges({ accessDirty });
  const accessRef = useRef<ResourceAccessHandle>(null);

  const connector = connectors.find((item) => item.slug === connectorSlug);
  const capabilities = (connector?.capabilities ?? []) as Capability[];
  const schema = (connector?.configSchema ?? {}) as ConfigSchema;
  const usesUrl = URL_CONNECTORS.includes(connectorSlug);
  const schemaFields = usesUrl ? [] : Object.entries(schema.properties ?? {}).filter(([key]) => !URL_KEYS.includes(key));
  const parsedConfig = useMemo(() => {
    try {
      const value = JSON.parse(configText || "{}");
      return value && typeof value === "object" && !Array.isArray(value) ? (value as Record<string, unknown>) : null;
    } catch {
      return null;
    }
  }, [configText]);

  function chooseConnector(slug: string) {
    if (editing || slug === connectorSlug) return;
    setConnectorSlug(slug);
    setScopes([]);
    const config = defaultConfig(slug);
    setConfigText(JSON.stringify(config, null, 2));
    setConfigError(null);
    setUrl(slug === "gitlab" ? "https://gitlab.com" : "");
    if (!name.trim() || connectors.some((item) => name === `${item.name} connection`)) {
      setName(`${connectors.find((item) => item.slug === slug)?.name ?? "New"} connection`);
    }
  }
  // A new connection starts with a sensible name for its service.
  useEffect(() => {
    if (!editing && !name && connector) setName(`${connector.name} connection`);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [connector?.slug]);

  function setSchemaField(key: string, value: string) {
    const result = setConfigField(configText, key, value, schema.properties?.[key]);
    if (result.error) {
      setConfigError(result.error);
      return;
    }
    setConfigError(null);
    setConfigText(result.configText);
  }
  const schemaValue = (key: string) => {
    const value = parsedConfig?.[key];
    if (value === undefined || value === null) return "";
    return typeof value === "object" ? JSON.stringify(value) : String(value);
  };
  const toggleScope = (id: string, checked: boolean) =>
    setScopes((current) => (checked ? [...new Set([...current, id])] : current.filter((scope) => scope !== id)));

  // ------------------------------------------------------------------ validation
  const issues: Issue[] = [];
  if (!connector) issues.push({ section: "service", message: "Choose a service." });
  if (!name.trim()) issues.push({ section: "account", message: "Enter a name." });
  const badUrl = usesUrl ? urlError(url) : null;
  if (badUrl) issues.push({ section: "account", message: badUrl });
  if (connectorSlug === "webhook" && !url.trim()) issues.push({ section: "account", message: "Enter the webhook URL." });
  for (const key of schema.required ?? []) {
    if (!usesUrl && !URL_KEYS.includes(key) && schemaValue(key) === "") {
      issues.push({ section: "account", message: `Enter ${schema.properties?.[key]?.title ?? humanizeIdentifier(key)}.` });
    }
  }
  if (!parsedConfig) issues.push({ section: "advanced", message: "The configuration must be a JSON object." });
  if (configError) issues.push({ section: "account", message: configError });
  const [showIssues, setShowIssues] = useState(false);
  const issueCount = (id: SectionId) => (showIssues ? issues.filter((issue) => issue.section === id).length : 0);

  // ------------------------------------------------------------------ sections
  const [active, setActive] = useState<SectionId>(editing || initialConnector ? "account" : "service");
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

  async function submit() {
    if (issues.length) {
      setShowIssues(true);
      go(issues[0].section);
      toast.error(issues.length === 1 ? issues[0].message : "Fix the highlighted sections");
      return;
    }
    const commitAccess = async () => {
      await accessRef.current?.save();
    };
    await onSubmit(
      {
        connectorSlug,
        name: name.trim(),
        ...(credential ? { credential } : {}),
        scopes,
        config: usesUrl ? applyUrl(connectorSlug, parsedConfig ?? {}, url) : (parsedConfig ?? {}),
      },
      commitAccess,
    );
  }

  // ------------------------------------------------------------------ summaries
  const configKeys = Object.keys(parsedConfig ?? {}).length;
  const summary: Record<SectionId, { value: string; missing?: boolean }> = {
    service: { value: connector?.name ?? "Not chosen", missing: !connector },
    account: {
      value: name.trim()
        ? `${name.trim()} · ${credential ? "new credential" : connection?.hasCredential ? "credential stored" : "no credential"}`
        : "Name not set",
      missing: !name.trim(),
    },
    capabilities: {
      value: capabilities.length ? `${scopes.length} of ${capabilities.length} allowed` : "None to choose",
      missing: capabilities.length > 0 && scopes.length === 0,
    },
    advanced: { value: configKeys ? `${configKeys} config ${configKeys === 1 ? "key" : "keys"}` : "Defaults" },
    sharing: { value: editing ? "Who can see and use it" : "After the first save" },
  };

  const navSections: NavSection<SectionId>[] = SECTION_ORDER.map((id) => ({
    id,
    title: SECTION_TITLES[id],
    value: summary[id].value,
    missing: summary[id].missing,
    problems: issueCount(id),
  }));
  const sectionNav = (
    <SectionRail label="Connection sections" sections={navSections} active={active} onGo={go} wrapValues />
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

  return (
    <div className="pb-24 lg:pb-0">
      {/* Phones and tablets: the sections as a scrollable row of steps. */}
      <SectionPills label="Connection sections" sections={navSections} active={active} onGo={go} className="mb-4" />

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
            <p className="text-[13px] text-muted-foreground">Credentials are encrypted at rest and never shown again.</p>
          </div>
        </aside>

        <div className="min-w-0 max-w-3xl space-y-5">
          <FormSection
            id="service"
            active={active}
            go={go}
            description={editing ? "The service this connection belongs to. It cannot change after creation." : "Which service this connection is for."}
          >
            {sectionIssues("service")}
            <div role="radiogroup" aria-label="Service" className="grid gap-2 sm:grid-cols-2">
              {connectors.map((item) => {
                const caps = (item.capabilities ?? []) as Capability[];
                const counts = { safe: 0, write: 0, external: 0 };
                for (const cap of caps) counts[effectTone(cap.effect)] += 1;
                return (
                  <ChoiceCard
                    key={item.slug}
                    selected={connectorSlug === item.slug}
                    onSelect={() => chooseConnector(item.slug)}
                    disabled={editing && item.slug !== connectorSlug}
                    icon={<Plug />}
                    title={item.name}
                    description={
                      <>
                        <span className="line-clamp-2">{item.description}</span>
                        <span className="mt-1 block text-xs">
                          {[
                            counts.safe ? `${counts.safe} read` : null,
                            counts.write ? `${counts.write} change` : null,
                            counts.external ? `${counts.external} act outside` : null,
                          ]
                            .filter(Boolean)
                            .join(" · ") || "No capabilities"}
                        </span>
                      </>
                    }
                  />
                );
              })}
            </div>
          </FormSection>

          <FormSection id="account" active={active} go={go} description={`The ${connector?.name ?? "service"} account ORBIT uses, and how to reach it.`}>
            {sectionIssues("account")}
            <Field label="Name" htmlFor="connection-name" hint="Shown when choosing a connection for an agent or team.">
              <Input id="connection-name" value={name} placeholder="Team GitLab" onChange={(event) => setName(event.target.value)} />
            </Field>
            {usesUrl ? (
              <Field
                label={connectorSlug === "gitlab" ? "GitLab URL" : "Webhook URL"}
                htmlFor="connection-url"
                error={showIssues ? badUrl : undefined}
                hint={
                  connectorSlug === "gitlab"
                    ? "The base URL. ORBIT adds /api/v4 for API calls and uses the same host for git clone and push."
                    : "The endpoint ORBIT sends signed JSON to."
                }
              >
                <Input
                  id="connection-url"
                  className="font-mono"
                  value={url}
                  placeholder={connectorSlug === "gitlab" ? "https://gitlab.internal" : "https://example.com/hooks/orbit"}
                  onChange={(event) => setUrl(event.target.value)}
                  spellCheck={false}
                />
              </Field>
            ) : null}
            <Field
              label="Credential"
              htmlFor="connection-credential"
              hint={
                editing
                  ? connection?.hasCredential
                    ? "A credential is stored. Leave this empty to keep it, or enter a new one to replace it."
                    : "No credential is stored yet. Enter one to add it."
                  : "A token or bot token. Stored encrypted; you will not see it again."
              }
            >
              <Input
                id="connection-credential"
                type="password"
                value={credential}
                autoComplete="new-password"
                placeholder={editing && connection?.hasCredential ? "••••••••  (stored)" : undefined}
                onChange={(event) => setCredential(event.target.value)}
              />
            </Field>
            {schemaFields.length ? (
              <div className="grid gap-4 sm:grid-cols-2">
                {schemaFields.map(([key, property]) => (
                  <Field
                    key={key}
                    htmlFor={`connection-config-${key}`}
                    label={property.title || humanizeIdentifier(key)}
                    optional={!schema.required?.includes(key)}
                    hint={property.description}
                  >
                    {property.type === "boolean" ? (
                      <Select
                        id={`connection-config-${key}`}
                        value={schemaValue(key) || String(property.default ?? "false")}
                        onChange={(event) => setSchemaField(key, event.target.value)}
                      >
                        <option value="false">No</option>
                        <option value="true">Yes</option>
                      </Select>
                    ) : (
                      <Input
                        id={`connection-config-${key}`}
                        type={
                          property.type === "integer" || property.type === "number"
                            ? "number"
                            : property.format === "password"
                              ? "password"
                              : "text"
                        }
                        value={schemaValue(key)}
                        placeholder={property.default !== undefined ? String(property.default) : undefined}
                        onChange={(event) => setSchemaField(key, event.target.value)}
                      />
                    )}
                  </Field>
                ))}
              </div>
            ) : null}
          </FormSection>

          <FormSection
            id="capabilities"
            active={active}
            go={go}
            description="What agents may do through this connection. ORBIT refuses everything else, whatever an agent is told."
          >
            {sectionIssues("capabilities")}
            {capabilities.length ? (
              <div className="space-y-5">
                {EFFECT_GROUPS.map((group) => {
                  const items = capabilities.filter((cap) => effectTone(cap.effect) === group.tone);
                  if (!items.length) return null;
                  const allOn = items.every((cap) => scopes.includes(cap.id));
                  return (
                    <fieldset key={group.tone} className="space-y-2">
                      <div className="flex flex-wrap items-baseline justify-between gap-2">
                        <legend className="text-sm font-medium">
                          {group.title}{" "}
                          <span className="tabular font-normal text-muted-foreground">
                            {items.filter((cap) => scopes.includes(cap.id)).length}/{items.length}
                          </span>
                        </legend>
                        <button
                          type="button"
                          onClick={() => items.forEach((cap) => toggleScope(cap.id, !allOn))}
                          className="rounded text-xs font-medium text-primary underline-offset-4 hover:underline"
                        >
                          {allOn ? "Clear" : "Allow all"}
                        </button>
                      </div>
                      <p className="text-[13px] text-muted-foreground">{group.hint}</p>
                      <div className="grid gap-2 sm:grid-cols-2">
                        {items.map((capability) => (
                          <CheckboxField
                            key={capability.id}
                            checked={scopes.includes(capability.id)}
                            onChange={(event) => toggleScope(capability.id, event.target.checked)}
                            title={capability.name}
                            description={
                              <span className="flex flex-wrap items-center gap-1.5">
                                <Badge variant={group.tone === "safe" ? "secondary" : "warning"}>{effectLabel(capability.effect)}</Badge>
                                <code className="font-mono text-xs">{capability.id}</code>
                              </span>
                            }
                          />
                        ))}
                      </div>
                    </fieldset>
                  );
                })}
                {scopes.length === 0 ? (
                  <Callout tone="warning" title="Nothing allowed yet">
                    Agents cannot use this connection until at least one capability is allowed.
                  </Callout>
                ) : null}
              </div>
            ) : (
              <p className="text-[13px] text-muted-foreground">This service has no capabilities to choose.</p>
            )}
          </FormSection>

          <FormSection id="advanced" active={active} go={go} description="The raw connector configuration. The fields above write into it; most connections never need this.">
            {sectionIssues("advanced")}
            <Field label="Config" htmlFor="connection-config-json" hint="A JSON object. Never put secrets here: use the credential." error={!parsedConfig ? "This is not a JSON object." : undefined}>
              <Textarea
                id="connection-config-json"
                className="min-h-40 font-mono text-base md:text-xs"
                spellCheck={false}
                value={configText}
                onChange={(event) => {
                  setConfigText(event.target.value);
                  setConfigError(null);
                }}
              />
            </Field>
          </FormSection>

          {/* Kept mounted while hidden so its pending edits are still saved with the connection. */}
          <div className={cn(active !== "sharing" && "hidden")}>
            <Card id="section-sharing" className="scroll-mt-20" aria-labelledby="section-sharing-title">
              <CardHeader>
                <h2 id="section-sharing-title" className="text-[15px] font-semibold leading-6">
                  {SECTION_TITLES.sharing}
                </h2>
                <p className="text-[13px] text-muted-foreground">Who can see this connection and use it for agents.</p>
              </CardHeader>
              <CardContent className="space-y-5">
                <ResourceAccessFields
                  ref={accessRef}
                  resource="connections"
                  resourceId={connection?.id}
                  onDirtyChange={setAccessDirty}
                />
                <div className="flex items-center justify-between gap-3 border-t pt-4">
                  <Button type="button" variant="ghost" onClick={() => go("advanced")}>
                    <ChevronLeft /> {SECTION_TITLES.advanced}
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
