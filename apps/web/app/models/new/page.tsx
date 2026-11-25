"use client";

import { useState } from "react";
import { useRouter } from "next/navigation";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import { type CommitAccess } from "@/components/resource-access-fields";
import { resourceSaveOutcome } from "@/lib/resource-save";
import { ProviderForm } from "@/components/providers/provider-form";
import { PageHeader } from "@/components/ui/page-header";
import { validationDetails } from "@/lib/http";
import type { ProviderDraft } from "@/lib/provider-meta";
import { createProvider } from "@/lib/providers";
import { queryKeys } from "@/lib/queries";
import { errorMessage } from "@orbit/api-client";

export default function NewProviderPage() {
  const router = useRouter();
  const queryClient = useQueryClient();
  const [submitting, setSubmitting] = useState(false);
  const [serverIssues, setServerIssues] = useState<Array<{ path: string; message: string }>>([]);

  async function handleSubmit(draft: ProviderDraft, commitAccess: CommitAccess) {
    setSubmitting(true);
    try {
      const created = await createProvider(draft);
      const accessSaved = await commitAccess(created.id);
      await queryClient.invalidateQueries({ queryKey: queryKeys.providers });
      const outcome = resourceSaveOutcome({
        mode: "create",
        saved: "Provider added",
        href: `/models/${created.id}`,
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
        back={{ href: "/models", label: "Model providers" }}
        title="Add provider"
        description="Where agents get their models. Providers are separate from harnesses, so agents can mix them freely."
      />
      <ProviderForm
        submitLabel="Add provider"
        submitting={submitting}
        onSubmit={handleSubmit}
        cancelHref="/models"
        serverIssues={serverIssues}
      />
    </div>
  );
}
