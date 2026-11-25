"use client";

import { Suspense, useState } from "react";
import { useRouter, useSearchParams } from "next/navigation";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import { type CommitAccess } from "@/components/resource-access-fields";
import { resourceSaveOutcome } from "@/lib/resource-save";
import { EnvironmentForm, type EnvironmentInput } from "@/components/environments/environment-form";
import { PageHeader } from "@/components/ui/page-header";
import { Skeleton } from "@/components/ui/skeleton";
import { createEnvMapping, useEnvMapping } from "@/lib/env-mappings";
import { validationDetails } from "@/lib/http";
import { errorMessage } from "@orbit/api-client";

function NewEnvironment() {
  const router = useRouter();
  const params = useSearchParams();
  const queryClient = useQueryClient();
  const duplicateId = params.get("duplicate") ?? "";
  const source = useEnvMapping(duplicateId);
  const [submitting, setSubmitting] = useState(false);
  const [serverIssues, setServerIssues] = useState<Array<{ path: string; message: string }>>([]);

  async function handleSubmit(input: EnvironmentInput, commitAccess: CommitAccess) {
    setSubmitting(true);
    try {
      const created = await createEnvMapping(input);
      const accessSaved = await commitAccess(created.id);
      await queryClient.invalidateQueries({ queryKey: ["env-mappings"] });
      const outcome = resourceSaveOutcome({
        mode: "create",
        saved: "Environment created",
        href: `/environments/${created.id}`,
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
        back={{ href: "/environments", label: "Environments" }}
        title={source.data ? `Duplicate ${source.data.name}` : "New environment"}
        description="A named set of variables you can attach to any number of worker profiles."
      />
      {duplicateId && source.isLoading ? (
        <Skeleton className="h-96 rounded-xl" />
      ) : (
        <EnvironmentForm
          // Remount once the source arrives, so the copy starts from it.
          key={source.data?.id ?? "new"}
          copyOf={source.data}
          submitLabel="Create environment"
          submitting={submitting}
          onSubmit={handleSubmit}
          cancelHref="/environments"
          serverIssues={serverIssues}
        />
      )}
    </div>
  );
}

export default function NewEnvironmentPage() {
  return (
    <Suspense fallback={<Skeleton className="h-96 rounded-xl" />}>
      <NewEnvironment />
    </Suspense>
  );
}
