"use client";

import { useState } from "react";
import { useRouter } from "next/navigation";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import { TeamForm } from "@/components/teams/team-form";
import { PageHeader } from "@/components/ui/page-header";
import { queryKeys } from "@/lib/queries";
import { createAgentTeam, type TeamInput } from "@/lib/teams";
import { errorMessage } from "@orbit/api-client";

export default function NewAgentTeamPage() {
  const router = useRouter();
  const queryClient = useQueryClient();
  const [submitting, setSubmitting] = useState(false);

  async function handleSubmit(input: TeamInput, commitAccess: (id?: string) => Promise<void>) {
    setSubmitting(true);
    try {
      const team = await createAgentTeam(input);
      await commitAccess(team.id);
      await queryClient.invalidateQueries({ queryKey: queryKeys.agentTeams });
      toast.success("Agent team created");
      router.push(`/agent-teams/${team.id}`);
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <div className="space-y-6">
      <PageHeader
        back={{ href: "/agent-teams", label: "Agent teams" }}
        title="New agent team"
        description="Choose the agents, how they work together and the goal they share. Everything after Agents is optional."
      />
      <TeamForm submitLabel="Create agent team" submitting={submitting} onSubmit={handleSubmit} cancelHref="/agent-teams" />
    </div>
  );
}
