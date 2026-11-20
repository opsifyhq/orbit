"use client";

import { useMemo, useRef, useState } from "react";
import Link from "next/link";
import { useMutation, useQueries, useQueryClient } from "@tanstack/react-query";
import {
  Cable,
  ChevronDown,
  Copy,
  Ellipsis,
  KeyRound,
  Pencil,
  Plus,
  Search,
  Trash2,
  TriangleAlert,
  X,
} from "lucide-react";
import { toast } from "sonner";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Callout } from "@/components/ui/callout";
import { Card } from "@/components/ui/card";
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
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { EmptyState } from "@/components/ui/empty-state";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { PageHeader } from "@/components/ui/page-header";
import { ResourceAccessFields, type ResourceAccessHandle } from "@/components/resource-access-fields";
import { Segmented } from "@/components/ui/segmented";
import { Select } from "@/components/ui/select";
import { Skeleton } from "@/components/ui/skeleton";
import {
  createEnvMapping,
  deleteEnvMapping,
  updateEnvMapping,
  workerProfileEnvMappings,
  type EnvMappingEntry,
  type EnvMappingRecord,
} from "@/lib/env-mappings";
import { useConnections, useEnvMappings, useWorkerProfiles } from "@/lib/queries";
import { brandName } from "@/lib/presentation";
import { relativeTime } from "@/lib/utils";
import { errorMessage } from "@orbit/api-client";

type Source = "connection" | "secret";

interface Row {
  envVar: string;
  source: Source;
  connectionId: string;
  secretKey: string;
  /** Carried through untouched so editing never drops a field this form does not show. */
  secretRef?: string | null;
  /** True once the name was typed by hand, so suggestions stop overwriting it. */
  named: boolean;
}

const emptyRow = (): Row => ({
  envVar: "",
  source: "connection",
  connectionId: "",
  secretKey: "",
  named: false,
});

/** A name a shell and a process environment will both accept. */
const ENV_NAME = /^[A-Za-z_][A-Za-z0-9_]*$/;

/** "github" → "GITHUB_TOKEN": a sensible starting name for a connection's credential. */
function suggestName(connectorSlug: string) {
  return `${connectorSlug.replace(/[^A-Za-z0-9]+/g, "_").toUpperCase()}_TOKEN`;
}

function toRows(mapping: EnvMappingRecord): Row[] {
  return mapping.mappings.map((entry) => ({
    envVar: entry.envVar,
    source: entry.connectionId ? "connection" : "secret",
    connectionId: entry.connectionId ?? "",
    secretKey: entry.secretKey ?? "",
    secretRef: entry.secretRef,
    named: true,
  }));
}

export default function EnvironmentMappingsPage() {
  const accessRef = useRef<ResourceAccessHandle>(null);
  const queryClient = useQueryClient();
  const confirm = useConfirm();
  const mappings = useEnvMappings();
  const connections = useConnections();
  const profiles = useWorkerProfiles();

  const [open, setOpen] = useState(false);
  const [editing, setEditing] = useState<EnvMappingRecord | null>(null);
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [rows, setRows] = useState<Row[]>([emptyRow()]);
  const [search, setSearch] = useState("");

  // Which worker profiles use each mapping. The API answers per profile, so ask for
  // each one; there are only ever a handful.
  const usageQueries = useQueries({
    queries: (profiles.data ?? []).map((profile) => ({
      queryKey: ["worker-profile-env-mappings", profile.id],
      queryFn: () => workerProfileEnvMappings(profile.id),
      staleTime: 30_000,
    })),
  });
  const usageKnown = profiles.isSuccess && usageQueries.every((query) => query.isSuccess);
  const usageStamp = usageQueries.map((query) => query.dataUpdatedAt).join(",");
  const usedBy = useMemo(() => {
    const result = new Map<string, Array<{ id: string; name: string }>>();
    (profiles.data ?? []).forEach((profile, index) => {
      for (const mapping of usageQueries[index]?.data ?? []) {
        result.set(mapping.id, [...(result.get(mapping.id) ?? []), { id: profile.id, name: profile.name }]);
      }
    });
    return result;
    // The query results change identity every render; their data timestamps do not.
  }, [profiles.data, usageStamp]);

  const connectionById = useMemo(
    () => new Map((connections.data ?? []).map((connection) => [connection.id, connection])),
    [connections.data],
  );

  const save = useMutation({
    mutationFn: async () => {
      const body = {
        name: name.trim(),
        description: description.trim(),
        mappings: rows
          .filter((row) => row.envVar.trim())
          // A variable has one source. Whatever the other source held before the
          // switch is not sent, so a stale secret can never ride along with a connection.
          .map<EnvMappingEntry>((row) =>
            row.source === "connection"
              ? { envVar: row.envVar.trim(), connectionId: row.connectionId || null, secretKey: null }
              : {
                  envVar: row.envVar.trim(),
                  connectionId: null,
                  secretKey: row.secretKey.trim() || null,
                  ...(row.secretRef ? { secretRef: row.secretRef } : {}),
                },
          ),
      };
      return editing ? updateEnvMapping(editing.id, body) : createEnvMapping(body);
    },
    onSuccess: () => {
      toast.success(editing ? "Mapping saved" : "Mapping created");
      setOpen(false);
      void queryClient.invalidateQueries({ queryKey: ["env-mappings"] });
      void queryClient.invalidateQueries({ queryKey: ["worker-profile-env-mappings"] });
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  async function saveMapping() {
    if ((await accessRef.current?.save()) === false) return;
    save.mutate();
  }

  const remove = useMutation({
    mutationFn: deleteEnvMapping,
    onSuccess: () => {
      toast.success("Mapping deleted");
      void queryClient.invalidateQueries({ queryKey: ["env-mappings"] });
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  function openCreate() {
    setEditing(null);
    setName("");
    setDescription("");
    setRows([emptyRow()]);
    setOpen(true);
  }

  function openEdit(mapping: EnvMappingRecord) {
    setEditing(mapping);
    setName(mapping.name);
    setDescription(mapping.description);
    setRows(toRows(mapping));
    setOpen(true);
  }

  function openDuplicate(mapping: EnvMappingRecord) {
    setEditing(null);
    setName(`${mapping.name} copy`);
    setDescription(mapping.description);
    setRows(toRows(mapping));
    setOpen(true);
  }

  function updateRow(index: number, patch: Partial<Row>) {
    setRows((current) => current.map((item, i) => (i === index ? { ...item, ...patch } : item)));
  }

  function chooseConnection(index: number, connectionId: string) {
    const row = rows[index];
    const connection = connectionById.get(connectionId);
    updateRow(index, {
      connectionId,
      // Offer a name while the person has not typed one of their own.
      ...(connection && !row.named ? { envVar: suggestName(connection.connectorSlug) } : {}),
    });
  }

  async function confirmDelete(mapping: EnvMappingRecord) {
    const users = (usedBy.get(mapping.id) ?? []).map((profile) => profile.name);
    const confirmed = await confirm({
      title: `Delete ${mapping.name}?`,
      description: users.length
        ? `${users.join(", ")} ${users.length === 1 ? "uses" : "use"} it and will stop receiving these variables on the next start. This cannot be undone.`
        : "No worker profile uses it. This cannot be undone.",
      confirmLabel: "Delete mapping",
      destructive: true,
    });
    if (confirmed) remove.mutate(mapping.id);
  }

  // Say what is missing instead of only disabling the button.
  const filled = rows.filter((row) => row.envVar.trim());
  const names = filled.map((row) => row.envVar.trim());
  const duplicate = names.find((value, index) => names.indexOf(value) !== index);
  const problem = !name.trim()
    ? "Give the mapping a name."
    : !filled.length
      ? "Add at least one variable."
      : filled.some((row) => !ENV_NAME.test(row.envVar.trim()))
        ? "Fix the variable names marked above."
        : duplicate
          ? `${duplicate} appears twice.`
          : filled.some((row) =>
                row.source === "connection" ? !row.connectionId : !row.secretKey.trim(),
              )
            ? "Every variable needs a source."
            : null;

  const all = mappings.data ?? [];
  const terms = search.toLowerCase().split(/\s+/).filter(Boolean);
  const visible = all.filter((mapping) => {
    if (!terms.length) return true;
    const haystack = [
      mapping.name,
      mapping.description,
      ...mapping.mappings.map((entry) => entry.envVar),
      ...mapping.mappings.map((entry) => connectionById.get(entry.connectionId ?? "")?.name ?? ""),
    ]
      .join(" ")
      .toLowerCase();
    return terms.every((term) => haystack.includes(term));
  });
  const variableCount = all.reduce((sum, mapping) => sum + mapping.mappings.length, 0);

  return (
    <div className="space-y-6">
      <PageHeader
        title="Environments"
        description="Sets of environment variables for worker processes. Each value comes from a connection's credential or a stored secret, so nothing is typed in plain text. Build a set once and attach it to any worker profile."
        meta={
          all.length ? (
            <span className="text-[13px] text-muted-foreground">
              {all.length} {all.length === 1 ? "mapping" : "mappings"}, {variableCount}{" "}
              {variableCount === 1 ? "variable" : "variables"}
            </span>
          ) : undefined
        }
        actions={
          <Button onClick={openCreate}>
            <Plus /> New mapping
          </Button>
        }
      />

      {all.length > 4 ? (
        <div className="relative md:w-80">
          <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
          <Input
            type="search"
            className="pl-9"
            placeholder="Filter by name, variable or connection"
            aria-label="Filter mappings"
            value={search}
            onChange={(event) => setSearch(event.target.value)}
          />
        </div>
      ) : null}

      {mappings.isLoading ? (
        <div className="grid gap-4 lg:grid-cols-2">
          {Array.from({ length: 2 }).map((_, index) => (
            <Skeleton key={index} className="h-40 rounded-xl" />
          ))}
        </div>
      ) : mappings.isError ? (
        <Callout tone="danger" title="Mappings could not be loaded">
          {errorMessage(mappings.error)}
        </Callout>
      ) : !all.length ? (
        <EmptyState
          icon={<KeyRound />}
          title="No environment mappings yet"
          description="A mapping is a named set of variables, for example GITLAB_TOKEN taken from your GitLab connection. Create one, then attach it to a worker profile under its advanced settings."
          action={
            <Button variant="outline" onClick={openCreate}>
              <Plus /> New mapping
            </Button>
          }
        />
      ) : !visible.length ? (
        <EmptyState
          icon={<Search />}
          title={`Nothing matches “${search}”`}
          description="Try a shorter term, or clear the filter."
          action={
            <Button variant="outline" onClick={() => setSearch("")}>
              Clear filter
            </Button>
          }
        />
      ) : (
        <ul className="grid gap-4 lg:grid-cols-2">
          {visible.map((mapping) => {
            const users = usedBy.get(mapping.id) ?? [];
            return (
              <li key={mapping.id}>
                <Card className="flex h-full flex-col">
                  <div className="flex items-start justify-between gap-3 p-4 pb-3 sm:p-5 sm:pb-3">
                    <div className="min-w-0">
                      <h2 className="break-words text-[15px] font-semibold">{mapping.name}</h2>
                      {mapping.description ? (
                        <p className="mt-0.5 text-[13px] text-muted-foreground">{mapping.description}</p>
                      ) : null}
                    </div>
                    <div className="flex shrink-0 items-center gap-1">
                      <Button
                        size="sm"
                        variant="outline"
                        aria-label={`Edit ${mapping.name}`}
                        onClick={() => openEdit(mapping)}
                      >
                        <Pencil /> Edit
                      </Button>
                      <DropdownMenu>
                        <DropdownMenuTrigger asChild>
                          <Button
                            size="icon-sm"
                            variant="ghost"
                            aria-label={`More actions for ${mapping.name}`}
                          >
                            <Ellipsis />
                          </Button>
                        </DropdownMenuTrigger>
                        <DropdownMenuContent align="end">
                          <DropdownMenuItem onSelect={() => openDuplicate(mapping)}>
                            <Copy /> Duplicate
                          </DropdownMenuItem>
                          <DropdownMenuSeparator />
                          <DropdownMenuItem destructive onSelect={() => void confirmDelete(mapping)}>
                            <Trash2 /> Delete mapping
                          </DropdownMenuItem>
                        </DropdownMenuContent>
                      </DropdownMenu>
                    </div>
                  </div>

                  <ul className="mx-4 divide-y rounded-lg border sm:mx-5">
                    {mapping.mappings.map((entry, index) => (
                      <li
                        key={`${entry.envVar}-${index}`}
                        className="grid gap-x-3 gap-y-0.5 px-3 py-2 sm:grid-cols-[minmax(0,1fr)_minmax(0,1.1fr)] sm:items-center"
                      >
                        <span className="break-all font-mono text-[13px] font-medium">{entry.envVar}</span>
                        <VariableSource
                          entry={entry}
                          connection={connectionById.get(entry.connectionId ?? "")}
                          connectionsLoaded={connections.isSuccess}
                        />
                      </li>
                    ))}
                  </ul>

                  <div className="mt-auto flex flex-wrap items-center justify-between gap-x-4 gap-y-1 p-4 pt-3 text-[13px] text-muted-foreground sm:p-5 sm:pt-3">
                    {!usageKnown ? (
                      <span>Checking which workers use it</span>
                    ) : (
                      <AttachedWorkers
                        mappingId={mapping.id}
                        users={users}
                        profiles={(profiles.data ?? []).map((profile) => ({ id: profile.id, name: profile.name }))}
                      />
                    )}
                    <span className="tabular">Updated {relativeTime(mapping.updatedAt)}</span>
                  </div>
                </Card>
              </li>
            );
          })}
        </ul>
      )}

      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="max-w-2xl">
          <DialogHeader>
            <DialogTitle>{editing ? `Edit ${editing.name}` : "New environment mapping"}</DialogTitle>
            <DialogDescription>
              {editing
                ? "Workers pick up changes the next time they start."
                : "A named set of variables you can attach to any number of worker profiles."}
            </DialogDescription>
          </DialogHeader>
          <form
            className="contents"
            onSubmit={(event) => {
              event.preventDefault();
              if (!problem && !save.isPending) void saveMapping();
            }}
          >
            <div className="space-y-5">
              <div className="grid gap-3 sm:grid-cols-2">
                <Field label="Name" htmlFor="em-name">
                  <Input
                    id="em-name"
                    value={name}
                    placeholder="GitLab and notifications"
                    onChange={(event) => setName(event.target.value)}
                  />
                </Field>
                <Field label="Description" htmlFor="em-description" optional>
                  <Input
                    id="em-description"
                    value={description}
                    placeholder="What these variables are for"
                    onChange={(event) => setDescription(event.target.value)}
                  />
                </Field>
              </div>

              <div className="space-y-3 border-t pt-4">
                <div>
                  <h3 className="text-[15px] font-semibold">Variables</h3>
                  <p className="mt-0.5 text-[13px] text-muted-foreground">
                    The value is never shown here: it is read from the source when a worker starts. A
                    variable set directly on a worker profile wins over these.
                  </p>
                </div>
                <ul className="space-y-2">
                  {rows.map((row, index) => {
                    const trimmed = row.envVar.trim();
                    const nameError =
                      trimmed && !ENV_NAME.test(trimmed)
                        ? "Letters, digits and underscores only, not starting with a digit"
                        : trimmed && names.indexOf(trimmed) !== names.lastIndexOf(trimmed)
                          ? "This name is used twice"
                          : undefined;
                    const chosen = connectionById.get(row.connectionId);
                    return (
                      <li key={index} className="rounded-lg border bg-subtle/60 p-3">
                        <div className="grid grid-cols-[minmax(0,1fr)_auto] items-start gap-3 sm:grid-cols-[minmax(0,1fr)_minmax(0,1.2fr)_auto]">
                          <Field label="Variable name" htmlFor={`em-${index}-var`} error={nameError}>
                            <Input
                              id={`em-${index}-var`}
                              value={row.envVar}
                              placeholder="GITLAB_TOKEN"
                              autoCapitalize="characters"
                              spellCheck={false}
                              className="font-mono text-base md:text-[13px]"
                              aria-invalid={nameError ? true : undefined}
                              onChange={(event) =>
                                updateRow(index, {
                                  envVar: event.target.value,
                                  named: Boolean(event.target.value),
                                })
                              }
                            />
                          </Field>
                          <div className="order-3 col-span-2 space-y-1.5 sm:order-2 sm:col-span-1">
                            <div className="flex min-h-[1.125rem] items-center justify-between gap-2">
                              <span className="text-[13px] font-medium leading-none" id={`em-${index}-source`}>
                                Value comes from
                              </span>
                            </div>
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
                                  aria-labelledby={`em-${index}-source`}
                                  value={row.connectionId}
                                  onChange={(event) => chooseConnection(index, event.target.value)}
                                >
                                  <option value="">Choose a connection</option>
                                  {(connections.data ?? []).map((connection) => (
                                    <option key={connection.id} value={connection.id}>
                                      {connection.name} ({brandName(connection.connectorSlug)})
                                    </option>
                                  ))}
                                </Select>
                              ) : (
                                <Input
                                  aria-labelledby={`em-${index}-source`}
                                  className="font-mono text-base md:text-[13px]"
                                  value={row.secretKey}
                                  placeholder="Key of the stored secret"
                                  spellCheck={false}
                                  onChange={(event) => updateRow(index, { secretKey: event.target.value })}
                                />
                              )}
                            </div>
                            {row.source === "connection" && chosen && !chosen.hasCredential ? (
                              <p className="text-[13px] text-warning">
                                This connection has no credential, so the variable would be empty.
                              </p>
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
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  onClick={() => setRows((current) => [...current, emptyRow()])}
                >
                  <Plus /> Add variable
                </Button>
                {connections.isSuccess && !connections.data?.length ? (
                  <p className="text-[13px] text-muted-foreground">
                    There are no connections yet.{" "}
                    <Link
                      href="/connections"
                      className="font-medium text-primary underline-offset-4 hover:underline"
                    >
                      Add one under Connectors
                    </Link>{" "}
                    to take a value from its credential.
                  </p>
                ) : null}
              </div>
            </div>
            <ResourceAccessFields ref={accessRef} resource="env_mappings" resourceId={editing?.id} />
            <DialogFooter className="sm:items-center">
              <span className="text-[13px] text-muted-foreground sm:mr-auto" aria-live="polite">
                {problem ?? ""}
              </span>
              <Button type="button" variant="outline" onClick={() => setOpen(false)}>
                Cancel
              </Button>
              <Button type="submit" loading={save.isPending} disabled={Boolean(problem)}>
                {editing ? "Save changes" : "Create mapping"}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
    </div>
  );
}

/** Where a variable's value is read from, with a warning when that source is gone. */
function VariableSource({
  entry,
  connection,
  connectionsLoaded,
}: {
  entry: EnvMappingEntry;
  connection?: { name: string; connectorSlug: string; hasCredential: boolean };
  connectionsLoaded: boolean;
}) {
  if (entry.connectionId) {
    if (!connection) {
      return connectionsLoaded ? (
        <span className="inline-flex items-center gap-1.5 text-[13px] text-warning">
          <TriangleAlert className="h-3.5 w-3.5 shrink-0" /> Its connection no longer exists
        </span>
      ) : (
        <span className="text-[13px] text-muted-foreground">From a connection</span>
      );
    }
    return (
      <span className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-[13px] text-muted-foreground">
        <Cable className="h-3.5 w-3.5 shrink-0" />
        <span className="min-w-0 truncate text-foreground">{connection.name}</span>
        <Badge variant="secondary">{brandName(connection.connectorSlug)}</Badge>
        {connection.hasCredential ? null : <Badge variant="warning">No credential</Badge>}
      </span>
    );
  }
  return (
    <span className="flex min-w-0 items-center gap-2 text-[13px] text-muted-foreground">
      <KeyRound className="h-3.5 w-3.5 shrink-0" />
      <span className="shrink-0 whitespace-nowrap">Stored secret</span>
      <span className="min-w-0 truncate font-mono text-xs text-foreground">
        {entry.secretKey ?? entry.secretRef ?? ""}
      </span>
    </span>
  );
}

/** Which worker profiles use a mapping, and a way to attach it to another one. */
function AttachedWorkers({
  mappingId,
  users,
  profiles,
}: {
  mappingId: string;
  users: Array<{ id: string; name: string }>;
  profiles: Array<{ id: string; name: string }>;
}) {
  const available = profiles.filter((profile) => !users.some((user) => user.id === profile.id));
  const linkClass = "font-medium text-foreground underline-offset-4 hover:underline";

  if (!profiles.length) {
    return (
      <span>
        No worker profiles yet.{" "}
        <Link href="/workers?create=1" className="font-medium text-primary underline-offset-4 hover:underline">
          Create one in Workers
        </Link>
      </span>
    );
  }

  return (
    <span className="flex flex-wrap items-center gap-x-2 gap-y-1">
      {users.length ? (
        <span>
          Used by{" "}
          {users.map((user, index) => (
            <span key={user.id}>
              {index ? ", " : null}
              <Link href={`/workers?edit=${user.id}`} className={linkClass}>
                {user.name}
              </Link>
            </span>
          ))}
        </span>
      ) : (
        <span>Not attached to a worker yet.</span>
      )}
      {available.length ? (
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button size="sm" variant="link" className="h-auto p-0 text-[13px]">
              {users.length ? "Attach to another" : "Attach to a worker"}
              <ChevronDown />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start">
            {available.map((profile) => (
              <DropdownMenuItem key={profile.id} asChild>
                <Link href={`/workers?edit=${profile.id}&attach=${mappingId}`}>{profile.name}</Link>
              </DropdownMenuItem>
            ))}
          </DropdownMenuContent>
        </DropdownMenu>
      ) : null}
    </span>
  );
}
