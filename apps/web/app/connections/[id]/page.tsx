"use client";

import Link from "next/link";
import { useParams, useRouter } from "next/navigation";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Download, Ellipsis, Pencil, PlugZap, Trash2 } from "lucide-react";
import { toast } from "sonner";

import { exportResource } from "@/components/export-resource-button";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Callout } from "@/components/ui/callout";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { useConfirm } from "@/components/ui/confirm";
import { DescriptionList } from "@/components/ui/description-list";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { EmptyState } from "@/components/ui/empty-state";
import { PageHeader } from "@/components/ui/page-header";
import { Skeleton } from "@/components/ui/skeleton";
import { Status } from "@/components/ui/status";
import { api } from "@/lib/api";
import { EFFECT_GROUPS, connectionStatus, useConnection, type Capability } from "@/lib/connections";
import { request } from "@/lib/http";
import { capabilityLabel, effectLabel, effectTone, humanizeIdentifier } from "@/lib/presentation";
import { queryKeys, unwrap, useConnectors } from "@/lib/queries";
import { formatDateTime, relativeTime } from "@/lib/utils";
import { errorMessage } from "@orbit/api-client";
import { deleteFailureMessage } from "@/lib/delete-impact";

function configValue(value: unknown): string {
  if (value === null || value === undefined || value === "") return "—";
  return typeof value === "object" ? JSON.stringify(value) : String(value);
}

export default function ConnectionPage() {
  const params = useParams<{ id: string }>();
  const connectionId = params.id;
  const router = useRouter();
  const queryClient = useQueryClient();
  const confirm = useConfirm();
  const connection = useConnection(connectionId);
  const connectors = useConnectors();

  const test = useMutation({
    mutationFn: () =>
      request<{ supported: boolean; ok: boolean; latencyMs: number; message: string }>(
        `/api/v1/connections/${connectionId}/test`,
        { method: "POST" },
      ),
    onSuccess: (result) => {
      // No probe for this connector: nothing was contacted and nothing changed.
      if (!result.supported) toast.info(result.message);
      else if (result.ok) toast.success(`${result.message} (${result.latencyMs} ms)`);
      else toast.error(`Test failed: ${result.message}`);
      void queryClient.invalidateQueries({ queryKey: queryKeys.connections });
    },
    onError: (error) => toast.error(`Test failed: ${errorMessage(error)}`),
  });

  const remove = useMutation({
    mutationFn: async () =>
      unwrap(await api.DELETE("/api/v1/connections/{connection_id}", { params: { path: { connection_id: connectionId } } })),
    onSuccess: async () => {
      toast.success("Connection removed");
      await queryClient.invalidateQueries({ queryKey: queryKeys.connections });
      router.push("/connections");
    },
    onError: (error) => toast.error(deleteFailureMessage("connection", error, errorMessage(error))),
  });

  if (connection.isLoading) {
    return (
      <div className="space-y-4">
        <Skeleton className="h-10 w-64" />
        <Skeleton className="h-96 rounded-xl" />
      </div>
    );
  }
  if (!connection.data) {
    return (
      <EmptyState
        title="Connection not found"
        description={connection.error ? errorMessage(connection.error) : "It may have been removed, or you cannot see it."}
        action={
          <Button asChild>
            <Link href="/connections">Back to connectors</Link>
          </Button>
        }
      />
    );
  }

  const data = connection.data;
  const connector = connectors.data?.find((item) => item.slug === data.connectorSlug);
  const capabilities = (connector?.capabilities ?? []) as Capability[];
  const byId = new Map(capabilities.map((cap) => [cap.id, cap]));
  const config = (data.config ?? {}) as Record<string, unknown>;
  const configEntries = Object.entries(config);

  async function confirmRemove() {
    const ok = await confirm({
      title: `Remove connection ${data.name}?`,
      description: "Its stored credential is deleted. This cannot be undone. A connection that agents, agent teams, environment mappings or worker profiles still use is not removed; you are told which ones.",
      confirmLabel: "Remove connection",
      destructive: true,
    });
    if (ok) remove.mutate();
  }

  return (
    <div className="space-y-6">
      <PageHeader
        back={{ href: "/connections", label: "Connectors" }}
        title={data.name}
        description={connector?.description ? `${data.connectorName}: ${connector.description}` : data.connectorName}
        meta={
          <>
            <Status meta={connectionStatus(data)} />
            <span className="text-[13px] text-muted-foreground">
              {data.lastCheckedAt ? `Checked ${relativeTime(data.lastCheckedAt)}` : "Never checked"}
            </span>
          </>
        }
        actions={
          <>
            <Button variant="outline" onClick={() => test.mutate()} loading={test.isPending}>
              <PlugZap /> Test
            </Button>
            <Button asChild variant="outline">
              <Link href={`/connections/${data.id}/edit`}>
                <Pencil /> Edit
              </Link>
            </Button>
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button variant="outline" size="icon" aria-label="More actions">
                  <Ellipsis />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                <DropdownMenuItem onSelect={() => void exportResource("connections", data.id, data.name)}>
                  <Download /> Export as YAML
                </DropdownMenuItem>
                <DropdownMenuSeparator />
                <DropdownMenuItem destructive onSelect={() => void confirmRemove()}>
                  <Trash2 /> Remove connection
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </>
        }
      />

      {data.status === "error" ? (
        <Callout tone="warning" title="The last check failed">
          Agents can still use this connection: the check runs from the ORBIT API, which may not
          reach what your workers reach. If the URL or credential is wrong, fix it in Edit and run{" "}
          <span className="font-medium">Test</span> again.
        </Callout>
      ) : null}
      {!data.hasCredential ? (
        <Callout
          tone="warning"
          title="No credential stored"
          action={
            <Button asChild size="sm" variant="outline">
              <Link href={`/connections/${data.id}/edit#account`}>Add one</Link>
            </Button>
          }
        >
          Most services refuse requests without one.
        </Callout>
      ) : null}

      <div className="grid gap-6 lg:grid-cols-[minmax(0,1fr)_20rem] lg:items-start [&>*]:min-w-0">
        <Card>
          <CardHeader>
            <CardTitle>Allowed capabilities</CardTitle>
            <p className="text-[13px] text-muted-foreground">
              Agents can do only this through the connection; ORBIT refuses everything else.
            </p>
          </CardHeader>
          <CardContent className="space-y-5">
            {data.scopes.length ? (
              EFFECT_GROUPS.map((group) => {
                const scopes = data.scopes.filter((scope) => effectTone(byId.get(scope)?.effect) === group.tone);
                if (!scopes.length) return null;
                return (
                  <div key={group.tone} className="space-y-2">
                    <p className="text-[13px] font-medium">
                      {group.title} <span className="tabular font-normal text-muted-foreground">{scopes.length}</span>
                    </p>
                    <ul className="flex flex-wrap gap-1.5">
                      {scopes.map((scope) => {
                        const cap = byId.get(scope);
                        return (
                          <li key={scope}>
                            <Badge
                              variant={group.tone === "safe" ? "secondary" : "warning"}
                              title={`${scope}${cap ? ` · ${effectLabel(cap.effect)}` : ""}`}
                            >
                              {cap?.name ?? capabilityLabel(scope)}
                            </Badge>
                          </li>
                        );
                      })}
                    </ul>
                  </div>
                );
              })
            ) : (
              <p className="text-[13px] text-muted-foreground">
                Nothing allowed yet, so agents cannot use this connection.{" "}
                <Link href={`/connections/${data.id}/edit#capabilities`} className="font-medium text-primary underline-offset-4 hover:underline">
                  Choose capabilities
                </Link>
                .
              </p>
            )}
            {capabilities.length ? (
              <p className="text-xs text-muted-foreground">
                {data.scopes.length} of {capabilities.length} capabilities of {data.connectorName} allowed.
              </p>
            ) : null}
          </CardContent>
        </Card>

        <div className="space-y-6">
          <Card>
            <CardHeader>
              <CardTitle>Details</CardTitle>
            </CardHeader>
            <CardContent>
              <DescriptionList
                items={[
                  { label: "Service", value: data.connectorName },
                  { label: "Credential", value: data.hasCredential ? "Stored, encrypted" : "Not set" },
                  ...configEntries.map(([key, value]) => ({ label: humanizeIdentifier(key), value: configValue(value), mono: true })),
                  { label: "Last check", value: data.lastCheckedAt ? formatDateTime(data.lastCheckedAt) : "Never" },
                  { label: "Created", value: formatDateTime(data.createdAt) },
                  { label: "Updated", value: formatDateTime(data.updatedAt) },
                ]}
              />
            </CardContent>
          </Card>
        </div>
      </div>
    </div>
  );
}
