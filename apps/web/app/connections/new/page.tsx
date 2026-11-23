"use client";

import { Suspense, useState } from "react";
import { useRouter, useSearchParams } from "next/navigation";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import { ConnectionForm } from "@/components/connections/connection-form";
import { Callout } from "@/components/ui/callout";
import { PageHeader } from "@/components/ui/page-header";
import { Skeleton } from "@/components/ui/skeleton";
import { createConnection, type ConnectionInput } from "@/lib/connections";
import { queryKeys, useConnectors } from "@/lib/queries";
import { errorMessage } from "@orbit/api-client";

function NewConnection() {
  const router = useRouter();
  const params = useSearchParams();
  const queryClient = useQueryClient();
  const connectors = useConnectors();
  const [submitting, setSubmitting] = useState(false);

  async function handleSubmit(input: ConnectionInput, commitAccess: () => Promise<void>) {
    setSubmitting(true);
    try {
      const created = await createConnection(input);
      await commitAccess();
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: queryKeys.connections }),
        queryClient.invalidateQueries({ queryKey: queryKeys.connectors }),
      ]);
      toast.success("Connection created");
      router.push(`/connections/${created.id}`);
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <div className="space-y-6">
      <PageHeader
        back={{ href: "/connections", label: "Connectors" }}
        title="New connection"
        description="An account ORBIT may use for one service. Agents get only the capabilities you allow here, and ORBIT enforces that."
      />
      {connectors.isLoading ? (
        <Skeleton className="h-96 rounded-xl" />
      ) : connectors.isError ? (
        <Callout tone="danger" title="The connector catalogue could not be loaded">
          {errorMessage(connectors.error)}
        </Callout>
      ) : (
        <ConnectionForm
          connectors={connectors.data ?? []}
          initialConnector={params.get("connector") ?? undefined}
          submitLabel="Create connection"
          submitting={submitting}
          onSubmit={handleSubmit}
          cancelHref="/connections"
        />
      )}
    </div>
  );
}

export default function NewConnectionPage() {
  return (
    <Suspense fallback={<Skeleton className="h-96 rounded-xl" />}>
      <NewConnection />
    </Suspense>
  );
}
