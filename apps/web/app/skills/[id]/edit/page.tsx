"use client";

import { LoadFailure } from "@/components/load-failure";
import { useState } from "react";
import { useParams, useRouter } from "next/navigation";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import { SkillForm, type SkillInput } from "@/components/skills/skill-form";
import { PageHeader } from "@/components/ui/page-header";
import { Skeleton } from "@/components/ui/skeleton";
import { queryKeys, useSkill } from "@/lib/queries";
import { updateSkill, validationDetails } from "@/lib/skills";
import { errorMessage } from "@orbit/api-client";

export default function EditSkillPage() {
  const params = useParams<{ id: string }>();
  // Keyed by id: moving from one edit page to another (the command palette does this
  // without a full navigation) must start a fresh editor, never carry the previous
  // object's form state or starting version into a save on this one.
  return <EditSkillEditor key={params.id} skillId={params.id} />;
}

function EditSkillEditor({ skillId }: { skillId: string }) {
  const router = useRouter();
  const queryClient = useQueryClient();
  const skill = useSkill(skillId);
  const [submitting, setSubmitting] = useState(false);
  const [serverIssues, setServerIssues] = useState<Array<{ path: string; message: string }>>([]);

  if (skill.isLoading) {
    return (
      <div className="space-y-4">
        <Skeleton className="h-10 w-64" />
        <Skeleton className="h-96 rounded-xl" />
      </div>
    );
  }
  if (!skill.data) {
    return (
      <LoadFailure
        noun="skill"
        error={skill.error}
        back={{ href: "/skills", label: "Back to skills" }}
        onRetry={() => void skill.refetch()}
        retrying={skill.isFetching}
      />
    );
  }

  async function handleSubmit(input: SkillInput) {
    setSubmitting(true);
    setServerIssues([]);
    try {
      const updated = await updateSkill(skillId, input);
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: queryKeys.skills }),
        queryClient.invalidateQueries({ queryKey: queryKeys.skill(skillId) }),
      ]);
      toast.success(`Saved as version ${updated.version}`);
      router.push(`/skills/${skillId}`);
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
        back={{ href: `/skills/${skillId}`, label: skill.data.name }}
        title={`Edit ${skill.data.name}`}
        description="Saving creates the next version. Agents pick it up on their next run."
      />
      <SkillForm
        skill={skill.data}
        submitLabel="Save changes"
        submitting={submitting}
        onSubmit={handleSubmit}
        cancelHref={`/skills/${skillId}`}
        serverIssues={serverIssues}
      />
    </div>
  );
}
