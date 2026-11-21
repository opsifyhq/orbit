"use client";

import { useMemo } from "react";
import Link from "next/link";
import { useParams, useRouter } from "next/navigation";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Copy, Ellipsis, Pencil, Trash2 } from "lucide-react";
import { toast } from "sonner";

import { AttachedWorkers, VariableSource } from "@/components/environments/parts";
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
import { ENV_STATUS, brokenVariables, envState } from "@/lib/env-mapping-meta";
import { deleteEnvMapping, useEnvMapping, useEnvMappingUsage } from "@/lib/env-mappings";
import { useConnections } from "@/lib/queries";
import { formatDateTime, relativeTime } from "@/lib/utils";
import { errorMessage } from "@orbit/api-client";

export default function EnvironmentPage() {
  const params = useParams<{ id: string }>();
  const id = params.id;
  const router = useRouter();
  const queryClient = useQueryClient();
  const confirm = useConfirm();
  const mapping = useEnvMapping(id);
  const connections = useConnections();
  const usage = useEnvMappingUsage();
  const connectionById = useMemo(
    () => (connections.isSuccess ? new Map((connections.data ?? []).map((connection) => [connection.id, connection])) : null),
    [connections.isSuccess, connections.data],
  );

  const remove = useMutation({
    mutationFn: () => deleteEnvMapping(id),
    onSuccess: async () => {
      toast.success("Environment deleted");
      await queryClient.invalidateQueries({ queryKey: ["env-mappings"] });
      void queryClient.invalidateQueries({ queryKey: ["worker-profile-env-mappings"] });
      router.push("/environments");
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  if (mapping.isLoading) {
    return (
      <div className="space-y-4">
        <Skeleton className="h-10 w-64" />
        <Skeleton className="h-96 rounded-xl" />
      </div>
    );
  }
  if (!mapping.data) {
    return (
      <EmptyState
        title="Environment not found"
        description={mapping.error ? errorMessage(mapping.error) : "It may have been deleted, or you cannot see it."}
        action={
          <Button asChild>
            <Link href="/environments">Back to environments</Link>
          </Button>
        }
      />
    );
  }

  const data = mapping.data;
  const users = usage.usedBy.get(data.id) ?? [];
  const broken = brokenVariables(data, connectionById);
  const fromConnections = data.mappings.filter((entry) => entry.connectionId).length;

  async function confirmDelete() {
    const names = users.map((user) => user.name);
    const ok = await confirm({
      title: `Delete ${data.name}?`,
      description: names.length
        ? `${names.join(", ")} ${names.length === 1 ? "uses" : "use"} it and will stop receiving these variables on the next start. This cannot be undone.`
        : "No worker profile uses it. This cannot be undone.",
      confirmLabel: "Delete environment",
      destructive: true,
    });
    if (ok) remove.mutate();
  }

  return (
    <div className="space-y-6">
      <PageHeader
        back={{ href: "/environments", label: "Environments" }}
        title={data.name}
        description={data.description || "A set of environment variables for worker processes."}
        meta={
          <>
            {usage.known && connectionById ? <Status meta={ENV_STATUS[envState(broken.length, users.length)]} /> : null}
            <span className="text-[13px] text-muted-foreground">Updated {relativeTime(data.updatedAt)}</span>
          </>
        }
        actions={
          <>
            <Button asChild variant="outline">
              <Link href={`/environments/${data.id}/edit`}>
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
                <DropdownMenuItem asChild>
                  <Link href={`/environments/new?duplicate=${data.id}`}>
                    <Copy /> Duplicate
                  </Link>
                </DropdownMenuItem>
                <DropdownMenuSeparator />
                <DropdownMenuItem destructive onSelect={() => void confirmDelete()}>
                  <Trash2 /> Delete environment
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </>
        }
      />

      {broken.length ? (
        <Callout
          tone="warning"
          title={`${broken.length === 1 ? "1 variable would be" : `${broken.length} variables would be`} empty`}
          action={
            <Button asChild size="sm" variant="outline">
              <Link href={`/environments/${data.id}/edit#variables`}>Fix</Link>
            </Button>
          }
        >
          {broken
            .map((item) => `${item.envVar}: ${item.reason === "missing" ? "its connection was removed" : "its connection has no credential"}`)
            .join("; ")}
          .
        </Callout>
      ) : null}

      <div className="grid gap-6 lg:grid-cols-[minmax(0,1fr)_20rem] lg:items-start [&>*]:min-w-0">
        <div className="space-y-6">
          <Card>
            <CardHeader>
              <CardTitle>Variables</CardTitle>
              <p className="text-[13px] text-muted-foreground">
                Values are read from their source when a worker starts and never shown here. A variable set directly on a
                worker profile wins over these.
              </p>
            </CardHeader>
            <CardContent>
              <ul className="divide-y rounded-lg border">
                {data.mappings.map((entry, index) => (
                  <li
                    key={`${entry.envVar}-${index}`}
                    className="grid gap-x-3 gap-y-0.5 px-3 py-2 sm:grid-cols-[minmax(0,1fr)_minmax(0,1.1fr)] sm:items-center"
                  >
                    <span className="break-all font-mono text-[13px] font-medium">{entry.envVar}</span>
                    <VariableSource
                      entry={entry}
                      connection={connectionById?.get(entry.connectionId ?? "")}
                      connectionsLoaded={connections.isSuccess}
                    />
                  </li>
                ))}
              </ul>
            </CardContent>
          </Card>

          <Card>
            <CardHeader>
              <CardTitle>Used by</CardTitle>
              <p className="text-[13px] text-muted-foreground">
                Worker profiles that load these variables. Attaching opens the profile with this environment ticked.
              </p>
            </CardHeader>
            <CardContent className="text-[13px] text-muted-foreground">
              {usage.known ? (
                <AttachedWorkers mappingId={data.id} users={users} profiles={usage.profiles} />
              ) : (
                <span>Checking which workers use it</span>
              )}
            </CardContent>
          </Card>
        </div>

        <Card>
          <CardHeader>
            <CardTitle>Details</CardTitle>
          </CardHeader>
          <CardContent>
            <DescriptionList
              items={[
                { label: "Variables", value: String(data.mappings.length) },
                { label: "From connections", value: String(fromConnections) },
                { label: "From secrets", value: String(data.mappings.length - fromConnections) },
                { label: "Created", value: formatDateTime(data.createdAt) },
                { label: "Updated", value: formatDateTime(data.updatedAt) },
              ]}
            />
          </CardContent>
        </Card>
      </div>
    </div>
  );
}
