"use client";

import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import Link from "next/link";
import { ChevronLeft, Plus, X } from "lucide-react";
import { toast } from "sonner";

import {
  ResourceAccessFields,
  type ResourceAccessHandle,
  type ResourceSubmit,
} from "@/components/resource-access-fields";
import { Button } from "@/components/ui/button";
import { Callout } from "@/components/ui/callout";
import { Card, CardContent, CardHeader } from "@/components/ui/card";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import {
  SectionCard,
  SectionPills,
  SectionRail,
  sectionNeighbours,
  type NavSection,
} from "@/components/ui/section-nav";
import { Segmented } from "@/components/ui/segmented";
import { Select } from "@/components/ui/select";
import {
  draftIssues,
  emptyRow,
  nameError,
  suggestName,
  toEntries,
  toRows,
  type SectionId,
  type VariableRow,
} from "@/lib/env-mapping-meta";
import type { EnvMappingEntry, EnvMappingRecord } from "@/lib/env-mappings";
import { brandName } from "@/lib/presentation";
import { useConnections } from "@/lib/queries";
import { useUnsavedChanges } from "@/lib/use-unsaved-changes";
import { cn } from "@/lib/utils";

const SECTION_TITLES: Record<SectionId, string> = {
  details: "Details",
  variables: "Variables",
  sharing: "Access control",
};
const SECTION_ORDER: SectionId[] = ["details", "variables", "sharing"];
const isSectionId = (value: string): value is SectionId => (SECTION_ORDER as string[]).includes(value);

export interface EnvironmentInput {
  name: string;
  description: string;
  mappings: EnvMappingEntry[];
}

function sectionOfPath(path: string): SectionId {
  if (/^mappings/.test(path)) return "variables";
  return "details";
}

/**
 * Environment Studio: the same one-section-at-a-time form as Agent and Connection Studio.
 * A name, then the variables and where each value comes from. Values are never typed or
 * shown: they are read from a connection's credential or a stored secret when a worker
 * starts.
 */
export function EnvironmentForm({
  mapping,
  copyOf,
  submitLabel,
  submitting,
  onSubmit,
  cancelHref,
  serverIssues = [],
}: {
  mapping?: EnvMappingRecord;
  /** Start a new environment from this one (Duplicate). */
  copyOf?: EnvMappingRecord;
  submitLabel: string;
  submitting?: boolean;
  onSubmit: ResourceSubmit<EnvironmentInput>;
  cancelHref: string;
  serverIssues?: Array<{ path: string; message: string }>;
}) {
  const editing = Boolean(mapping);
  const start = mapping ?? copyOf;
  const connections = useConnections();
  const [name, setName] = useState(mapping?.name ?? (copyOf ? `${copyOf.name} copy` : ""));
  const [description, setDescription] = useState(start?.description ?? "");
  const [rows, setRows] = useState<VariableRow[]>(() => (start?.mappings.length ? toRows(start) : [emptyRow()]));
  const [accessDirty, setAccessDirty] = useState(false);
  useUnsavedChanges({ name, description, rows, accessDirty });
  const accessRef = useRef<ResourceAccessHandle>(null);

  const connectionById = useMemo(
    () => new Map((connections.data ?? []).map((connection) => [connection.id, connection])),
    [connections.data],
  );
  const names = rows.filter((row) => row.envVar.trim()).map((row) => row.envVar.trim());

  // ------------------------------------------------------------------ validation
  const issues = draftIssues(name, rows);
  for (const issue of serverIssues) issues.push({ section: sectionOfPath(issue.path), message: issue.message });
  const [showIssues, setShowIssues] = useState(false);
  const issueCount = (id: SectionId) => (showIssues ? issues.filter((issue) => issue.section === id).length : 0);

  // ------------------------------------------------------------------ sections
  const [active, setActive] = useState<SectionId>(editing ? "variables" : "details");
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
    node?.querySelector<HTMLElement>("input:not([type=hidden]):not(:disabled), textarea, select")?.focus({ preventScroll: true });
  }, [focusRequest, active]);
  function go(id: SectionId) {
    setActive(id);
    setFocusRequest((count) => count + 1);
    window.history.replaceState(null, "", `#${id}`);
  }
  useEffect(() => {
    if (!serverIssues.length) return;
    setShowIssues(true);
    go(sectionOfPath(serverIssues[0].path));
  }, [serverIssues]);

  const updateRow = (index: number, patch: Partial<VariableRow>) =>
    setRows((current) => current.map((item, i) => (i === index ? { ...item, ...patch } : item)));
  function chooseConnection(index: number, connectionId: string) {
    const row = rows[index];
    const connection = connectionById.get(connectionId);
    updateRow(index, {
      connectionId,
      // Offer a name while the person has not typed one of their own.
      ...(connection && !row.named ? { envVar: suggestName(connection.connectorSlug) } : {}),
    });
  }

  async function submit() {
    const local = issues.filter((issue) => !serverIssues.some((server) => server.message === issue.message));
    if (local.length) {
      setShowIssues(true);
      go(local[0].section);
      toast.error(local.length === 1 ? local[0].message : "Fix the highlighted sections");
      return;
    }
    const commitAccess = async (id?: string) => {
      await accessRef.current?.save(id);
    };
    await onSubmit(
      { name: name.trim(), description: description.trim(), mappings: toEntries(rows) },
      commitAccess,
    );
  }

  // ------------------------------------------------------------------ summaries
  const fromConnections = rows.filter((row) => row.envVar.trim() && row.source === "connection").length;
  const fromSecrets = names.length - fromConnections;
  const summary: Record<SectionId, { value: string; missing?: boolean }> = {
    details: { value: name.trim() || "Name not set", missing: !name.trim() },
    variables: {
      value: names.length
        ? [
            `${names.length} ${names.length === 1 ? "variable" : "variables"}`,
            fromConnections ? `${fromConnections} from connections` : null,
            fromSecrets ? `${fromSecrets} from secrets` : null,
          ]
            .filter(Boolean)
            .join(" · ")
        : "None yet",
      missing: !names.length,
    },
    sharing: { value: editing ? "Who can see and use it" : "After the first save" },
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

  return (
    <div className="pb-24 lg:pb-0">
      <SectionPills label="Environment sections" sections={navSections} active={active} onGo={go} className="mb-4" />

      <div className="grid gap-6 lg:grid-cols-[15rem_minmax(0,1fr)] lg:items-start">
        <aside className="hidden lg:sticky lg:top-6 lg:block">
          <SectionRail label="Environment sections" sections={navSections} active={active} onGo={go} wrapValues />
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
            <p className="text-[13px] text-muted-foreground">Values are read when a worker starts and never shown here.</p>
          </div>
        </aside>

        <div className="min-w-0 max-w-3xl space-y-5">
          {section(
            "details",
            "What this set of variables is for. Worker profiles list it by name.",
            <>
              <Field label="Name" htmlFor="env-name">
                <Input id="env-name" value={name} placeholder="GitLab and notifications" onChange={(event) => setName(event.target.value)} />
              </Field>
              <Field label="Description" htmlFor="env-description" optional>
                <Input
                  id="env-description"
                  value={description}
                  placeholder="What these variables are for"
                  onChange={(event) => setDescription(event.target.value)}
                />
              </Field>
            </>,
          )}

          {section(
            "variables",
            "Each variable takes its value from a connection's credential or a stored secret when a worker starts. A variable set directly on a worker profile wins over these.",
            <>
              <ul className="space-y-2">
                {rows.map((row, index) => {
                  const trimmed = row.envVar.trim();
                  const error = nameError(row, names);
                  const chosen = connectionById.get(row.connectionId);
                  const missingSource =
                    showIssues && trimmed && (row.source === "connection" ? !row.connectionId : !row.secretKey.trim() && !row.secretRef);
                  return (
                    <li key={index} className="rounded-lg border bg-subtle/60 p-3">
                      <div className="grid grid-cols-[minmax(0,1fr)_auto] items-start gap-3 sm:grid-cols-[minmax(0,1fr)_minmax(0,1.2fr)_auto]">
                        <Field label="Variable name" htmlFor={`env-${index}-var`} error={error}>
                          <Input
                            id={`env-${index}-var`}
                            value={row.envVar}
                            placeholder="GITLAB_TOKEN"
                            autoCapitalize="characters"
                            spellCheck={false}
                            className="font-mono text-base md:text-[13px]"
                            aria-invalid={error ? true : undefined}
                            onChange={(event) => updateRow(index, { envVar: event.target.value, named: Boolean(event.target.value) })}
                          />
                        </Field>
                        <div className="order-3 col-span-2 space-y-1.5 sm:order-2 sm:col-span-1">
                          <span className="block text-[13px] font-medium leading-none" id={`env-${index}-source`}>
                            Value comes from
                          </span>
                          <div className="flex flex-col gap-2">
                            <Segmented
                              label={`Source for ${trimmed || `variable ${index + 1}`}`}
                              value={row.source}
                              onChange={(source) => updateRow(index, { source })}
                              options={[
                                { value: "connection", label: "Connection" },
                                { value: "secret", label: "Secret" },
                              ]}
                              className="self-start"
                            />
                            {row.source === "connection" ? (
                              <Select
                                aria-labelledby={`env-${index}-source`}
                                aria-invalid={missingSource ? true : undefined}
                                value={row.connectionId}
                                onChange={(event) => chooseConnection(index, event.target.value)}
                              >
                                <option value="">{connections.isLoading ? "Loading connections…" : "Choose a connection"}</option>
                                {row.connectionId && !chosen && connections.isSuccess ? (
                                  <option value={row.connectionId}>A connection that no longer exists</option>
                                ) : null}
                                {(connections.data ?? []).map((connection) => (
                                  <option key={connection.id} value={connection.id}>
                                    {connection.name} ({brandName(connection.connectorSlug)})
                                  </option>
                                ))}
                              </Select>
                            ) : (
                              <Input
                                aria-labelledby={`env-${index}-source`}
                                aria-invalid={missingSource ? true : undefined}
                                className="font-mono text-base md:text-[13px]"
                                value={row.secretKey}
                                placeholder="Key of the stored secret"
                                spellCheck={false}
                                onChange={(event) => updateRow(index, { secretKey: event.target.value })}
                              />
                            )}
                          </div>
                          {row.source === "connection" && chosen && !chosen.hasCredential ? (
                            <p className="text-[13px] text-warning">This connection has no credential, so the variable would be empty.</p>
                          ) : null}
                          {row.source === "connection" && row.connectionId && !chosen && connections.isSuccess ? (
                            <p className="text-[13px] text-warning">Its connection was removed. Choose another.</p>
                          ) : null}
                        </div>
                        <Button
                          type="button"
                          size="icon"
                          variant="ghost"
                          className="order-2 mt-[1.625rem] sm:order-3"
                          disabled={rows.length === 1}
                          aria-label={`Remove ${trimmed || `variable ${index + 1}`}`}
                          onClick={() => setRows((current) => current.filter((_, i) => i !== index))}
                        >
                          <X />
                        </Button>
                      </div>
                    </li>
                  );
                })}
              </ul>
              <Button type="button" size="sm" variant="outline" onClick={() => setRows((current) => [...current, emptyRow()])}>
                <Plus /> Add variable
              </Button>
              {connections.isSuccess && !connections.data?.length ? (
                <p className="text-[13px] text-muted-foreground">
                  There are no connections yet.{" "}
                  <Link href="/connections/new" className="font-medium text-primary underline-offset-4 hover:underline">
                    Connect a service
                  </Link>{" "}
                  to take a value from its credential.
                </p>
              ) : null}
            </>,
          )}

          {/* Kept mounted while hidden so its pending edits are still saved with the environment. */}
          <div className={cn(active !== "sharing" && "hidden")}>
            <Card id="section-sharing" className="scroll-mt-20" aria-labelledby="section-sharing-title">
              <CardHeader>
                <h2 id="section-sharing-title" className="text-[15px] font-semibold leading-6">
                  {SECTION_TITLES.sharing}
                </h2>
                <p className="text-[13px] text-muted-foreground">Who can see this environment and attach it to worker profiles.</p>
              </CardHeader>
              <CardContent className="space-y-5">
                <ResourceAccessFields
                  ref={accessRef}
                  resource="env_mappings"
                  resourceId={mapping?.id}
                  onDirtyChange={setAccessDirty}
                />
                <div className="flex items-center justify-between gap-3 border-t pt-4">
                  <Button type="button" variant="ghost" onClick={() => go("variables")}>
                    <ChevronLeft /> {SECTION_TITLES.variables}
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
