"use client";

import { useState } from "react";
import Link from "next/link";
import { useParams, useRouter } from "next/navigation";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import { ProviderForm } from "@/components/providers/provider-form";
import { Button } from "@/components/ui/button";
import { EmptyState } from "@/components/ui/empty-state";
import { PageHeader } from "@/components/ui/page-header";
import { Skeleton } from "@/components/ui/skeleton";
import { validationDetails } from "@/lib/http";
import type { ProviderDraft } from "@/lib/provider-meta";
import { updateProvider, useProvider } from "@/lib/providers";
import { queryKeys } from "@/lib/queries";
import { errorMessage } from "@orbit/api-client";

export default function EditProviderPage() {
  const params = useParams<{ id: string }>();
  const providerId = params.id;
  const router = useRouter();
  const queryClient = useQueryClient();
  const provider = useProvider(providerId);
  const [submitting, setSubmitting] = useState(false);
  const [serverIssues, setServerIssues] = useState<Array<{ path: string; message: string }>>([]);

  if (provider.isLoading) {
    return (
      <div className="space-y-4">
        <Skeleton className="h-10 w-64" />
        <Skeleton className="h-96 rounded-xl" />
      </div>
    );
  }
  if (!provider.data) {
    return (
      <EmptyState
        title="Provider not found"
        description={provider.error ? errorMessage(provider.error) : "It may have been deleted, or you cannot see it."}
        action={
          <Button asChild>
            <Link href="/models">Back to model providers</Link>
          </Button>
        }
      />
    );
  }
  const data = provider.data;

  async function handleSubmit(draft: ProviderDraft) {
    setSubmitting(true);
    try {
      await updateProvider(data, draft);
      await queryClient.invalidateQueries({ queryKey: queryKeys.providers });
      toast.success("Provider saved");
      router.push(`/models/${providerId}`);
    } catch (error) {
      setServerIssues(validationDetails(error));
      toast.error(errorMessage(error));
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <div className="space-y-6">
      <PageHeader
        back={{ href: `/models/${providerId}`, label: data.name }}
        title={`Edit ${data.name}`}
        description="Changes apply to every agent and worker profile that uses this provider from their next run."
      />
      <ProviderForm
        provider={data}
        submitLabel="Save changes"
        submitting={submitting}
        onSubmit={handleSubmit}
        cancelHref={`/models/${providerId}`}
        serverIssues={serverIssues}
      />
    </div>
  );
}
