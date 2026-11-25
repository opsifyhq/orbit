"use client";

import { useState } from "react";
import Link from "next/link";
import { useParams, useRouter } from "next/navigation";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import { type CommitAccess } from "@/components/resource-access-fields";
import { resourceSaveOutcome } from "@/lib/resource-save";
import { ConnectionForm } from "@/components/connections/connection-form";
import { Button } from "@/components/ui/button";
import { EmptyState } from "@/components/ui/empty-state";
import { PageHeader } from "@/components/ui/page-header";
import { Skeleton } from "@/components/ui/skeleton";
import { updateConnection, useConnection, type ConnectionInput } from "@/lib/connections";
import { queryKeys, useConnectors } from "@/lib/queries";
import { errorMessage } from "@orbit/api-client";

export default function EditConnectionPage() {
  const params = useParams<{ id: string }>();
  const connectionId = params.id;
  const router = useRouter();
  const queryClient = useQueryClient();
  const connection = useConnection(connectionId);
  const connectors = useConnectors();
  const [submitting, setSubmitting] = useState(false);

  if (connection.isLoading || connectors.isLoading) {
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

  async function handleSubmit(input: ConnectionInput, commitAccess: CommitAccess) {
    setSubmitting(true);
    try {
      await updateConnection(connectionId, input);
      const accessSaved = await commitAccess();
      await queryClient.invalidateQueries({ queryKey: queryKeys.connections });
      const outcome = resourceSaveOutcome({
        mode: "edit",
        saved: "Connection saved",
        href: `/connections/${connectionId}`,
        accessSaved,
      });
      toast[outcome.tone](outcome.message);
      if (outcome.href) router.push(outcome.href);
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <div className="space-y-6">
      <PageHeader
        back={{ href: `/connections/${connectionId}`, label: connection.data.name }}
        title={`Edit ${connection.data.name}`}
        description="Changes apply to every agent and team that uses this connection from their next run."
      />
      <ConnectionForm
        connection={connection.data}
        connectors={connectors.data ?? []}
        submitLabel="Save changes"
        submitting={submitting}
        onSubmit={handleSubmit}
        cancelHref={`/connections/${connectionId}`}
      />
    </div>
  );
}
