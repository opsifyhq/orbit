"use client";

import { useMemo, useRef, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import {
  Cable,
  Download,
  MoreHorizontal,
  Pencil,
  PlugZap,
  Plus,
  Trash2,
  Webhook,
} from "lucide-react";
import { toast } from "sonner";

import { ResourceAccessFields, type ResourceAccessHandle } from "@/components/resource-access-fields";
import { Badge } from "@/components/ui/badge";
import { Disclosure } from "@/components/ui/disclosure";
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
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { EmptyState } from "@/components/ui/empty-state";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { PageHeader } from "@/components/ui/page-header";
import { Section } from "@/components/ui/section";
import { Select } from "@/components/ui/select";
import { exportResource } from "@/components/export-resource-button";
import { ImportPortableButton } from "@/components/import-portable-dialog";
import { Skeleton } from "@/components/ui/skeleton";
import { Status, type StatusMeta } from "@/components/ui/status";
import { Textarea } from "@/components/ui/textarea";
import { api } from "@/lib/api";
import { setConfigField } from "@/lib/connection-config";
import { request } from "@/lib/http";
import { queryKeys, unwrap, useConnections, useConnectors } from "@/lib/queries";
import { errorMessage } from "@orbit/api-client";
import { formatDate, relativeTime, titleCase } from "@/lib/utils";
import { capabilityLabel, effectLabel, effectTone, humanizeIdentifier } from "@/lib/presentation";
import type { ConnectionView } from "@/lib/types";

type Capability = { id: string; name: string; effect: string };

const MAX_CHIPS = 6;

/** Read = quiet, write = warning, external effect = destructive: the same scale as approvals. */
function effectVariant(effect: string | null | undefined) {
  const tone = effectTone(effect);
  // Red is kept for failures and irreversible actions; both risky effects are amber
  // and the label says which one it is.
  return tone === "safe" ? "secondary" : "warning";
}

function connectionStatus(connection: ConnectionView): StatusMeta {
  // "active" is the usable state; a recorded check is what earns "Connected".
  if (connection.status === "active" && connection.lastCheckedAt) {
    return { label: "Connected", tone: "success", shape: "solid" };
  }
  if (connection.status === "error") return { label: "Last check failed", tone: "danger", shape: "cross" };
  return { label: titleCase(connection.status), tone: "neutral", shape: "ring" };
}

const defaultConfig = (slug: string) => {
  if (slug === "gitlab") {
    return JSON.stringify(
      {
        baseUrl: "https://gitlab.com/api/v4",
        cloneBaseUrl: "https://gitlab.com",
      },
      null,
      2,
    );
  }
  if (slug === "webhook") {
    return JSON.stringify({ url: "" }, null, 2);
  }
  return "{}";
};

export default function ConnectionsPage() {
  const accessRef = useRef<ResourceAccessHandle>(null);
  const connections = useConnections();
  const connectors = useConnectors();
  const queryClient = useQueryClient();
  const confirm = useConfirm();
  const [open, setOpen] = useState(false);
  const [editing, setEditing] = useState<{ id: string; connectorSlug: string; hasCredential: boolean } | null>(null);
  const [connectorSlug, setConnectorSlug] = useState("telegram");
  const [name, setName] = useState("");
  const [credential, setCredential] = useState("");
  const [scopes, setScopes] = useState<string[]>([]);
  const [configText, setConfigText] = useState("{}");
  const [configError, setConfigError] = useState<string | null>(null);
  const [connectionUrl, setConnectionUrl] = useState("");
  const [schemaValues, setSchemaValues] = useState<Record<string, string>>({});

  const selected = connectors.data?.find((connector) => connector.slug === connectorSlug);
  const capabilities = (selected?.capabilities ?? []) as Capability[];
  const schema = (selected?.configSchema ?? {}) as {
    properties?: Record<
      string,
      {
        type?: string;
        title?: string;
        description?: string;
        default?: unknown;
        format?: string;
      }
    >;
    required?: string[];
  };
  const schemaFields = Object.entries(schema.properties ?? {}).filter(
    ([key]) => !["baseUrl", "cloneBaseUrl", "url"].includes(key),
  );

  // Effect of every capability, so a connection's scopes can be coloured by what they can do.
  const effectBySlug = useMemo(() => {
    const map = new Map<string, Map<string, string>>();
    for (const connector of connectors.data ?? []) {
      const caps = (connector.capabilities ?? []) as Capability[];
      map.set(connector.slug, new Map(caps.map((cap) => [cap.id, cap.effect])));
    }
    return map;
  }, [connectors.data]);

  const connectionCount = useMemo(() => {
    const counts = new Map<string, number>();
    for (const connection of connections.data ?? []) {
      counts.set(connection.connectorSlug, (counts.get(connection.connectorSlug) ?? 0) + 1);
    }
    return counts;
  }, [connections.data]);

  function setSchemaField(key: string, value: string) {
    setSchemaValues((current) => ({ ...current, [key]: value }));
    const result = setConfigField(configText, key, value, schema.properties?.[key]);
    if (result.error) {
      setConfigError(result.error);
      return;
    }
    setConfigError(null);
    setConfigText(result.configText);
  }

  const save = useMutation({
    mutationFn: async () => {
      let config: Record<string, unknown> = {};
      try {
        const parsed = configText.trim() ? JSON.parse(configText) : {};
        if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
          config = parsed as Record<string, unknown>;
        }
      } catch {
        throw new Error("Connection config must be valid JSON");
      }
      const url = connectionUrl.trim().replace(/\/+$/, "");
      if (url && !/^https?:\/\//.test(url)) {
        throw new Error("URL must start with https:// or http://");
      }
      if (connectorSlug === "gitlab" && url) {
        const root = url.replace(/\/api\/v4$/, "");
        config = { ...config, baseUrl: `${root}/api/v4`, cloneBaseUrl: root };
      }
      if (connectorSlug === "webhook" && url) {
        config = { ...config, url };
      }
      if (editing) {
        return unwrap(
          await api.PATCH("/api/v1/connections/{connection_id}", {
            params: { path: { connection_id: editing.id } },
            body: {
              name: name.trim(),
              scopes,
              config,
              ...(credential ? { credential } : {}),
            },
          }),
        );
      }
      return unwrap(
        await api.POST("/api/v1/connections", {
          body: {
            connectorSlug,
            name: name.trim(),
            ...(credential ? { credential } : {}),
            scopes,
            config,
          },
        }),
      );
    },
    onSuccess: () => {
      toast.success(editing ? "Connection updated" : "Connection created");
      setOpen(false);
      setEditing(null);
      setName("");
      setCredential("");
      setScopes([]);
      setConfigText("{}");
      setConfigError(null);
      setConnectionUrl("");
      void queryClient.invalidateQueries({ queryKey: queryKeys.connections });
      void queryClient.invalidateQueries({ queryKey: queryKeys.connectors });
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  /** Picking a connector resets everything that depends on it. */
  function chooseConnector(slug: string) {
    setConnectorSlug(slug);
    setScopes([]);
    setConfigText(defaultConfig(slug));
    setConfigError(null);
    setSchemaValues({});
    setConnectionUrl(slug === "gitlab" ? "https://gitlab.com" : "");
  }

  function openCreate(slug?: string) {
    setEditing(null);
    setName("");
    setCredential("");
    if (slug) {
      chooseConnector(slug);
    } else {
      setConnectorSlug("telegram");
      setScopes([]);
      setConfigText(defaultConfig("telegram"));
      setConfigError(null);
      setSchemaValues({});
      setConnectionUrl("");
    }
    setOpen(true);
  }

  function openEdit(connection: ConnectionView) {
    setEditing({
      id: connection.id,
      connectorSlug: connection.connectorSlug,
      hasCredential: connection.hasCredential,
    });
    setConnectorSlug(connection.connectorSlug);
    setName(connection.name);
    setCredential("");
    setScopes(connection.scopes);
    setConfigText(JSON.stringify(connection.config ?? {}, null, 2));
    setConfigError(null);
    const config = (connection.config ?? {}) as Record<string, unknown>;
    setSchemaValues(
      Object.fromEntries(
        Object.entries(config).map(([key, value]) => [
          key,
          typeof value === "object" ? JSON.stringify(value) : String(value ?? ""),
        ]),
      ),
    );
    const baseUrl = String(config.baseUrl ?? "");
    const cloneBaseUrl = String(config.cloneBaseUrl ?? "");
    setConnectionUrl(
      connection.connectorSlug === "gitlab"
        ? cloneBaseUrl || baseUrl.replace(/\/api\/v4$/, "")
        : connection.connectorSlug === "webhook"
          ? String(config.url ?? "")
          : "",
    );
    setOpen(true);
  }

  const test = useMutation({
    mutationFn: (id: string) =>
      request<{ supported: boolean; ok: boolean; latencyMs: number; message: string }>(
        `/api/v1/connections/${id}/test`,
        { method: "POST" },
      ),
    onSuccess: (result) => {
      if (!result.supported) {
        // Nothing was contacted and nothing changed: not a failure.
        toast.info(result.message);
        return;
      }
      if (result.ok) {
        toast.success(`${result.message} (${result.latencyMs} ms)`);
      } else {
        toast.error(`Test failed: ${result.message}`);
      }
      void queryClient.invalidateQueries({ queryKey: queryKeys.connections });
    },
    onError: (error) => toast.error(`Test failed: ${errorMessage(error)}`),
  });

  async function saveConnection() {
    if ((await accessRef.current?.save()) === false) return;
    save.mutate();
  }

  const remove = useMutation({
    mutationFn: async (id: string) =>
      unwrap(
        await api.DELETE("/api/v1/connections/{connection_id}", {
          params: { path: { connection_id: id } },
        }),
      ),
    onSuccess: () => {
      toast.success("Connection removed");
      void queryClient.invalidateQueries({ queryKey: queryKeys.connections });
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  async function removeConnection(connection: ConnectionView) {
    const ok = await confirm({
      title: `Remove connection ${connection.name}?`,
      description:
        "Its stored credential is deleted and agents lose access through it. This cannot be undone.",
      confirmLabel: "Remove connection",
      destructive: true,
    });
    if (ok) remove.mutate(connection.id);
  }

  const catalogue = connectors.data ?? [];

  return (
    <div className="space-y-8">
      <PageHeader
        title="Connectors"
        description="Connections are accounts you have authorised for external services. Agents use them only for the capabilities you allow, and ORBIT enforces that."
        actions={
          <div className="flex flex-wrap gap-2">
            <ImportPortableButton />
            <Button onClick={() => openCreate()}>
              <Plus /> New connection
            </Button>
          </div>
        }
      />

      <Section
        title="Your connections"
        count={connections.data?.length}
        description="Each connection holds one credential and the capabilities it may be used for."
      >
        {connections.isLoading ? (
          <Card className="overflow-hidden">
            {Array.from({ length: 2 }).map((_, index) => (
              <div key={index} className="border-b px-5 py-4 last:border-b-0">
                <Skeleton className="h-14" />
              </div>
            ))}
          </Card>
        ) : connections.isError ? (
          <Callout tone="danger" title="Could not load connections">
            {errorMessage(connections.error)}
          </Callout>
        ) : !connections.data?.length ? (
          <EmptyState
            icon={<Cable />}
            title="No connections yet"
            description="Pick a service from the list below to connect a Telegram bot, a GitLab account or a webhook. Then you can grant agents specific capabilities."
            action={
              <Button variant="outline" onClick={() => openCreate()}>
                <Plus /> New connection
              </Button>
            }
          />
        ) : (
          <Card className="overflow-hidden">
            {connections.data.map((connection) => {
              const effects = effectBySlug.get(connection.connectorSlug);
              const shown = connection.scopes.slice(0, MAX_CHIPS);
              const hidden = connection.scopes.slice(MAX_CHIPS);
              return (
                <div
                  key={connection.id}
                  className="flex flex-col gap-3 border-b px-4 py-4 last:border-b-0 sm:px-5 lg:flex-row lg:items-start lg:justify-between"
                >
                  <div className="min-w-0 space-y-2">
                    <div className="flex flex-wrap items-center gap-2">
                      <p className="truncate text-sm font-medium">{connection.name}</p>
                      <Badge variant="outline">{connection.connectorName}</Badge>
                    </div>
                    <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-[13px] text-muted-foreground">
                      <Status meta={connectionStatus(connection)} />
                      <span>
                        {connection.hasCredential ? "Credential set" : "No credential"}
                      </span>
                      <span>
                        {connection.lastCheckedAt
                          ? `Checked ${relativeTime(connection.lastCheckedAt)}`
                          : "Never checked"}
                      </span>
                      <span>Added {formatDate(connection.createdAt)}</span>
                    </div>
                    <div className="flex flex-wrap gap-1.5">
                      {connection.scopes.length ? (
                        <>
                          {shown.map((scope) => (
                            <Badge key={scope} variant={effectVariant(effects?.get(scope))} title={scope}>
                              {capabilityLabel(scope)}
                            </Badge>
                          ))}
                          {hidden.length ? (
                            <Badge
                              variant="outline"
                              title={hidden.map((scope) => capabilityLabel(scope)).join(", ")}
                            >
                              +{hidden.length} more
                            </Badge>
                          ) : null}
                        </>
                      ) : (
                        <span className="text-[13px] text-muted-foreground">
                          No capabilities allowed yet. Edit the connection to choose some.
                        </span>
                      )}
                    </div>
                  </div>
                  <div className="flex shrink-0 flex-wrap items-center gap-2">
                    <Button
                      size="sm"
                      variant="outline"
                      aria-label={`Test ${connection.name}`}
                      onClick={() => test.mutate(connection.id)}
                      loading={test.isPending && test.variables === connection.id}
                    >
                      <PlugZap /> Test
                    </Button>
                    <Button
                      size="sm"
                      variant="ghost"
                      aria-label={`Edit ${connection.name}`}
                      onClick={() => openEdit(connection)}
                    >
                      <Pencil /> Edit
                    </Button>
                    <DropdownMenu>
                      <DropdownMenuTrigger asChild>
                        <Button
                          size="icon-sm"
                          variant="ghost"
                          aria-label={`More actions for ${connection.name}`}
                        >
                          <MoreHorizontal />
                        </Button>
                      </DropdownMenuTrigger>
                      <DropdownMenuContent align="end">
                        <DropdownMenuItem onSelect={() => void exportResource("connections", connection.id, connection.name)}>
                          <Download /> Export as YAML
                        </DropdownMenuItem>
                        <DropdownMenuSeparator />
                        <DropdownMenuItem destructive onSelect={() => void removeConnection(connection)}>
                          <Trash2 /> Remove connection
                        </DropdownMenuItem>
                      </DropdownMenuContent>
                    </DropdownMenu>
                  </div>
                </div>
              );
            })}
          </Card>
        )}
      </Section>

      <Section
        title="Connect a service"
        description="The services ORBIT can connect to. Capabilities are colour-coded by what they can do: read only, change data, or act outside ORBIT."
      >
        {connectors.isLoading ? (
          <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
            {Array.from({ length: 3 }).map((_, index) => (
              <Skeleton key={index} className="h-40 rounded-xl" />
            ))}
          </div>
        ) : connectors.isError ? (
          <Callout tone="danger" title="Could not load the connector catalogue">
            {errorMessage(connectors.error)}
          </Callout>
        ) : (
          <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
            {catalogue.map((connector) => {
              const caps = (connector.capabilities ?? []) as Capability[];
              const auth = (connector.auth ?? []) as string[];
              const counts = { safe: 0, write: 0, external: 0 };
              for (const cap of caps) counts[effectTone(cap.effect)] += 1;
              const existing = connectionCount.get(connector.slug) ?? 0;
              return (
                <Card key={connector.id} className="flex flex-col gap-3 p-4">
                  <div className="space-y-1">
                    <div className="flex items-start justify-between gap-2">
                      <h3 className="text-[15px] font-semibold">{connector.name}</h3>
                      {connector.webhooks ? (
                        <Badge variant="outline">
                          <Webhook className="h-3 w-3" aria-hidden /> Webhooks
                        </Badge>
                      ) : null}
                    </div>
                    <p className="text-[13px] text-muted-foreground">{connector.description}</p>
                  </div>
                  <div className="flex flex-wrap gap-1.5">
                    {counts.safe ? <Badge variant="secondary">{counts.safe} read only</Badge> : null}
                    {counts.write ? <Badge variant="warning">{counts.write} change data</Badge> : null}
                    {counts.external ? (
                      <Badge variant="warning">{counts.external} act outside ORBIT</Badge>
                    ) : null}
                    {auth.map((method) => (
                      <Badge key={method} variant="outline">
                        {humanizeIdentifier(method)}
                      </Badge>
                    ))}
                  </div>
                  <div className="mt-auto flex items-center justify-between gap-2 pt-1">
                    <span className="text-[13px] text-muted-foreground">
                      {existing ? `${existing} connected` : "Not connected"}
                    </span>
                    <Button
                      size="sm"
                      variant="outline"
                      onClick={() => openCreate(connector.slug)}
                      aria-label={`Connect ${connector.name}`}
                    >
                      <Plus /> Connect
                    </Button>
                  </div>
                </Card>
              );
            })}
          </div>
        )}
      </Section>

      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="max-w-xl">
          <DialogHeader>
            <DialogTitle>{editing ? `Edit ${name || "connection"}` : "New connection"}</DialogTitle>
            <DialogDescription>
              Credentials are encrypted at rest and never shown again.
            </DialogDescription>
          </DialogHeader>
          <form
            className="space-y-4"
            onSubmit={(event) => {
              event.preventDefault();
              if (name.trim() && !configError && !save.isPending) void saveConnection();
            }}
          >
            <div className="grid gap-4 sm:grid-cols-2">
              <Field label="Connector" htmlFor="connection-connector">
                <Select
                  id="connection-connector"
                  value={connectorSlug}
                  disabled={Boolean(editing)}
                  onChange={(event) => chooseConnector(event.target.value)}
                >
                  {catalogue.map((connector) => (
                    <option key={connector.slug} value={connector.slug}>
                      {connector.name}
                    </option>
                  ))}
                </Select>
              </Field>
              <Field label="Name" htmlFor="connection-name">
                <Input
                  id="connection-name"
                  value={name}
                  placeholder="Team GitLab"
                  onChange={(event) => setName(event.target.value)}
                />
              </Field>
            </div>

            {connectorSlug === "gitlab" || connectorSlug === "webhook" ? (
              <Field
                label={connectorSlug === "gitlab" ? "GitLab URL" : "Webhook URL"}
                htmlFor="connection-url"
                hint={
                  connectorSlug === "gitlab"
                    ? "The base URL. ORBIT adds /api/v4 for API calls and uses the same host for git clone and push."
                    : "The endpoint ORBIT sends signed JSON to."
                }
              >
                <Input
                  id="connection-url"
                  className="font-mono"
                  value={connectionUrl}
                  placeholder={
                    connectorSlug === "gitlab"
                      ? "https://gitlab.internal"
                      : "https://example.com/hooks/orbit"
                  }
                  onChange={(event) => setConnectionUrl(event.target.value)}
                />
              </Field>
            ) : null}

            <Field
              label="Credential"
              htmlFor="connection-credential"
              hint={
                editing
                  ? editing.hasCredential
                    ? "A credential is stored. Leave blank to keep the current secret, or enter a new one to replace it."
                    : "No credential is stored yet. Enter one to add it."
                  : "A token or bot token. Stored encrypted, and you will not see it again."
              }
            >
              <Input
                id="connection-credential"
                type="password"
                value={credential}
                autoComplete="new-password"
                onChange={(event) => setCredential(event.target.value)}
              />
            </Field>

            {schemaFields.length > 0 && connectorSlug !== "gitlab" && connectorSlug !== "webhook" ? (
              <div className="space-y-4 rounded-lg border p-3">
                {schemaFields.map(([key, property]) => (
                  <Field
                    key={key}
                    htmlFor={`connection-config-${key}`}
                    label={
                      <>
                        {property.title || humanizeIdentifier(key)}
                        {schema.required?.includes(key) ? (
                          <span className="ml-1.5 font-normal text-muted-foreground">Required</span>
                        ) : null}
                      </>
                    }
                    hint={property.description}
                  >
                    {property.type === "boolean" ? (
                      <Select
                        id={`connection-config-${key}`}
                        value={schemaValues[key] ?? String(property.default ?? "false")}
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
                        value={schemaValues[key] ?? String(property.default ?? "")}
                        placeholder={String(property.default ?? "")}
                        onChange={(event) => setSchemaField(key, event.target.value)}
                      />
                    )}
                  </Field>
                ))}
              </div>
            ) : null}

            <fieldset className="space-y-2">
              <legend className="text-sm font-medium">Allowed capabilities</legend>
              <p className="text-[13px] text-muted-foreground">
                Agents can only use the capabilities you tick here.
              </p>
              {capabilities.length ? (
                <div className="grid gap-2 sm:grid-cols-2">
                  {capabilities.map((capability) => (
                    <CheckboxField
                      key={capability.id}
                      checked={scopes.includes(capability.id)}
                      onChange={(event) =>
                        setScopes((current) =>
                          event.target.checked
                            ? [...current, capability.id]
                            : current.filter((id) => id !== capability.id),
                        )
                      }
                      title={capability.name}
                      description={
                        <Badge variant={effectVariant(capability.effect)} className="mt-1">
                          {effectLabel(capability.effect)}
                        </Badge>
                      }
                    />
                  ))}
                </div>
              ) : (
                <p className="text-[13px] text-muted-foreground">This connector has no capabilities to choose.</p>
              )}
            </fieldset>

            <Disclosure title="Advanced configuration" summary="Raw JSON">
                <Field
                  label="Config"
                  htmlFor="connection-config-json"
                  hint="Must be a JSON object."
                  error={configError ?? undefined}
                >
                  <Textarea
                    id="connection-config-json"
                    className="min-h-28 font-mono text-xs"
                    value={configText}
                    onChange={(event) => {
                      setConfigText(event.target.value);
                      setConfigError(null);
                    }}
                  />
                </Field>
              </Disclosure>

            <ResourceAccessFields ref={accessRef} resource="connections" resourceId={editing?.id} />
            <DialogFooter>
              <Button type="button" variant="outline" onClick={() => setOpen(false)}>
                Cancel
              </Button>
              <Button type="submit" loading={save.isPending} disabled={!name.trim() || Boolean(configError)}>
                {editing ? "Save changes" : "Create connection"}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
    </div>
  );
}
