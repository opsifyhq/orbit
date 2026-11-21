"use client";

import { Suspense, useState } from "react";
import { useRouter, useSearchParams } from "next/navigation";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import { OutputForm } from "@/components/outputs/output-form";
import { PageHeader } from "@/components/ui/page-header";
import { Skeleton } from "@/components/ui/skeleton";
import { validationDetails } from "@/lib/http";
import { createOutput, type OutputInput } from "@/lib/outputs";
import { errorMessage } from "@orbit/api-client";

function NewOutput() {
  const router = useRouter();
  const params = useSearchParams();
  const queryClient = useQueryClient();
  const [submitting, setSubmitting] = useState(false);
  const [serverIssues, setServerIssues] = useState<Array<{ path: string; message: string }>>([]);

  async function handleSubmit(input: OutputInput) {
    setSubmitting(true);
    try {
      const created = await createOutput(input);
      await queryClient.invalidateQueries({ queryKey: ["outputs"] });
      toast.success("Output created");
      router.push(`/outputs/${created.id}`);
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
        back={{ href: "/outputs", label: "Outputs" }}
        title="New output"
        description="Post to Slack, Mattermost or a webhook when a run finishes."
      />
      <OutputForm
        initial={{ agentId: params.get("agent") ?? undefined, teamId: params.get("team") ?? undefined }}
        submitLabel="Create output"
        submitting={submitting}
        onSubmit={handleSubmit}
        cancelHref="/outputs"
        serverIssues={serverIssues}
      />
    </div>
  );
}

export default function NewOutputPage() {
  return (
    <Suspense fallback={<Skeleton className="h-96 rounded-xl" />}>
      <NewOutput />
    </Suspense>
  );
}
