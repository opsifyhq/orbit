"use client";

import { LoadFailure } from "@/components/load-failure";
import { useState } from "react";
import { useParams, useRouter } from "next/navigation";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import { TeamForm } from "@/components/teams/team-form";
import { PageHeader } from "@/components/ui/page-header";
import { Skeleton } from "@/components/ui/skeleton";
import { queryKeys, useAgentTeam } from "@/lib/queries";
import { updateAgentTeam, type TeamInput } from "@/lib/teams";
import { errorMessage } from "@orbit/api-client";

export default function EditAgentTeamPage() {
  const params = useParams<{ id: string }>();
  // Keyed by id: moving from one edit page to another (the command palette does this
  // without a full navigation) must start a fresh editor, never carry the previous
  // object's form state or starting version into a save on this one.
  return <EditAgentTeamEditor key={params.id} teamId={params.id} />;
}

function EditAgentTeamEditor({ teamId }: { teamId: string }) {
  const router = useRouter();
  const queryClient = useQueryClient();
  const team = useAgentTeam(teamId);
  const [submitting, setSubmitting] = useState(false);

  if (team.isLoading) {
    return (
      <div className="space-y-4">
        <Skeleton className="h-10 w-64" />
        <Skeleton className="h-96 rounded-xl" />
      </div>
    );
  }
  if (!team.data) {
    return (
      <LoadFailure
        noun="agent team"
        error={team.error}
        back={{ href: "/agent-teams", label: "Back to agent teams" }}
        onRetry={() => void team.refetch()}
        retrying={team.isFetching}
      />
    );
  }

  async function handleSubmit(input: TeamInput) {
    setSubmitting(true);
    try {
      await updateAgentTeam(teamId, input);
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: queryKeys.agentTeams }),
        queryClient.invalidateQueries({ queryKey: queryKeys.agentTeam(teamId) }),
      ]);
      toast.success("Agent team saved");
      router.push(`/agent-teams/${teamId}`);
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <div className="space-y-6">
      <PageHeader
        back={{ href: `/agent-teams/${teamId}`, label: team.data.name }}
        title={`Edit ${team.data.name}`}
        description="Changes apply to runs started after you save."
      />
      <TeamForm
        team={team.data}
        submitLabel="Save changes"
        submitting={submitting}
        onSubmit={handleSubmit}
        cancelHref={`/agent-teams/${teamId}`}
      />
    </div>
  );
}
