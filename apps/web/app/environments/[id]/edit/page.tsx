"use client";

import { useState } from "react";
import Link from "next/link";
import { useParams, useRouter } from "next/navigation";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import { type CommitAccess } from "@/components/resource-access-fields";
import { resourceSaveOutcome } from "@/lib/resource-save";
import { EnvironmentForm, type EnvironmentInput } from "@/components/environments/environment-form";
import { Button } from "@/components/ui/button";
import { EmptyState } from "@/components/ui/empty-state";
import { PageHeader } from "@/components/ui/page-header";
import { Skeleton } from "@/components/ui/skeleton";
import { updateEnvMapping, useEnvMapping } from "@/lib/env-mappings";
import { validationDetails } from "@/lib/http";
import { errorMessage } from "@orbit/api-client";

export default function EditEnvironmentPage() {
  const params = useParams<{ id: string }>();
  const id = params.id;
  const router = useRouter();
  const queryClient = useQueryClient();
  const mapping = useEnvMapping(id);
  const [submitting, setSubmitting] = useState(false);
  const [serverIssues, setServerIssues] = useState<Array<{ path: string; message: string }>>([]);

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

  async function handleSubmit(input: EnvironmentInput, commitAccess: CommitAccess) {
    setSubmitting(true);
    try {
      await updateEnvMapping(id, input);
      const accessSaved = await commitAccess();
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["env-mappings"] }),
        queryClient.invalidateQueries({ queryKey: ["worker-profile-env-mappings"] }),
      ]);
      const outcome = resourceSaveOutcome({
        mode: "edit",
        saved: "Environment saved",
        href: `/environments/${id}`,
        accessSaved,
      });
      toast[outcome.tone](outcome.message);
      if (outcome.href) router.push(outcome.href);
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
        back={{ href: `/environments/${id}`, label: data.name }}
        title={`Edit ${data.name}`}
        description="Workers pick up changes the next time they start."
      />
      <EnvironmentForm
        mapping={data}
        submitLabel="Save changes"
        submitting={submitting}
        onSubmit={handleSubmit}
        cancelHref={`/environments/${id}`}
        serverIssues={serverIssues}
      />
    </div>
  );
}
