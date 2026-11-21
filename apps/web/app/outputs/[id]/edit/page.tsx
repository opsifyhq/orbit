"use client";

import { useState } from "react";
import Link from "next/link";
import { useParams, useRouter } from "next/navigation";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import { OutputForm } from "@/components/outputs/output-form";
import { Button } from "@/components/ui/button";
import { EmptyState } from "@/components/ui/empty-state";
import { PageHeader } from "@/components/ui/page-header";
import { Skeleton } from "@/components/ui/skeleton";
import { validationDetails } from "@/lib/http";
import { updateOutput, type OutputInput } from "@/lib/outputs";
import { useOutputs } from "@/lib/queries";
import { errorMessage } from "@orbit/api-client";

export default function EditOutputPage() {
  const params = useParams<{ id: string }>();
  const outputId = params.id;
  const router = useRouter();
  const queryClient = useQueryClient();
  const outputs = useOutputs();
  const output = outputs.data?.find((item) => item.id === outputId);
  const [submitting, setSubmitting] = useState(false);
  const [serverIssues, setServerIssues] = useState<Array<{ path: string; message: string }>>([]);

  if (outputs.isLoading) {
    return (
      <div className="space-y-4">
        <Skeleton className="h-10 w-64" />
        <Skeleton className="h-96 rounded-xl" />
      </div>
    );
  }
  if (!output) {
    return (
      <EmptyState
        title="Output not found"
        description={outputs.error ? errorMessage(outputs.error) : "It may have been deleted, or you cannot see it."}
        action={
          <Button asChild>
            <Link href="/outputs">Back to outputs</Link>
          </Button>
        }
      />
    );
  }

  async function handleSubmit(input: OutputInput) {
    setSubmitting(true);
    try {
      await updateOutput(outputId, input);
      await queryClient.invalidateQueries({ queryKey: ["outputs"] });
      toast.success("Output saved");
      router.push(`/outputs/${outputId}`);
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
        back={{ href: `/outputs/${outputId}`, label: output.name }}
        title={`Edit ${output.name}`}
        description="Changes apply to runs that finish after you save."
      />
      <OutputForm
        output={output}
        submitLabel="Save changes"
        submitting={submitting}
        onSubmit={handleSubmit}
        cancelHref={`/outputs/${outputId}`}
        serverIssues={serverIssues}
      />
    </div>
  );
}
