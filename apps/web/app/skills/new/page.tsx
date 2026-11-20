"use client";

import { useState } from "react";
import { useRouter } from "next/navigation";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import { SkillForm, type SkillInput } from "@/components/skills/skill-form";
import { PageHeader } from "@/components/ui/page-header";
import { queryKeys } from "@/lib/queries";
import { createSkill, validationDetails } from "@/lib/skills";
import { errorMessage } from "@orbit/api-client";

export default function NewSkillPage() {
  const router = useRouter();
  const queryClient = useQueryClient();
  const [submitting, setSubmitting] = useState(false);
  const [serverIssues, setServerIssues] = useState<Array<{ path: string; message: string }>>([]);

  async function handleSubmit(input: SkillInput) {
    setSubmitting(true);
    setServerIssues([]);
    try {
      const skill = await createSkill(input);
      await queryClient.invalidateQueries({ queryKey: queryKeys.skills });
      toast.success("Skill created");
      router.push(`/skills/${skill.id}`);
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
        back={{ href: "/skills", label: "Skills" }}
        title="New skill"
        description="A SKILL.md package agents load when its description matches their task. Name it, say when to use it and write the instructions; files are optional."
      />
      <SkillForm
        submitLabel="Create skill"
        submitting={submitting}
        onSubmit={handleSubmit}
        cancelHref="/skills"
        serverIssues={serverIssues}
      />
    </div>
  );
}
