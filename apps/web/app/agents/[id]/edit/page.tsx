"use client";

import { type CommitAccess } from "@/components/resource-access-fields";
import { resourceSaveOutcome } from "@/lib/resource-save";
import { validationDetails } from "@/lib/http";
import { LoadFailure } from "@/components/load-failure";
import Link from "next/link";
import { useParams, useRouter } from "next/navigation";
import { useRef, useState } from "react";
import { toast } from "sonner";

import { AgentForm } from "@/components/agents/agent-form";
import { Button } from "@/components/ui/button";
import { Callout } from "@/components/ui/callout";
import { EmptyState } from "@/components/ui/empty-state";
import { PageHeader } from "@/components/ui/page-header";
import { Skeleton } from "@/components/ui/skeleton";
import { api } from "@/lib/api";
import { toAgentDefinitionBody } from "@/lib/agent-body";
import { queryKeys, unwrap, useAgent } from "@/lib/queries";
import { errorMessage } from "@orbit/api-client";
import { agentSpecSchema, type AgentDefinition } from "@orbit/contracts";
import { useQueryClient } from "@tanstack/react-query";

export default function EditAgentPage() {
  const params = useParams<{ id: string }>();
  // Keyed by id: moving from one edit page to another (the command palette does this
  // without a full navigation) must start a fresh editor, never carry the previous
  // object's form state or starting version into a save on this one.
  return <EditAgentEditor key={params.id} agentId={params.id} />;
}

function EditAgentEditor({ agentId }: { agentId: string }) {
  const router = useRouter();
  const queryClient = useQueryClient();
  const agent = useAgent(agentId);
  const [submitting, setSubmitting] = useState(false);
  const [serverIssues, setServerIssues] = useState<Array<{ path: string; message: string }>>([]);
  // The version this edit started from. The agent keeps refreshing in the background,
  // so the latest known version is not it; the API refuses a save based on an older one.
  const startedFrom = useRef<number | null>(null);
  if (agent.data && startedFrom.current === null) startedFrom.current = agent.data.currentVersion;

  if (agent.isLoading) {
    return (
      <div className="space-y-4">
        <Skeleton className="h-10 w-64" />
        <Skeleton className="h-96 rounded-xl" />
      </div>
    );
  }

  if (!agent.data) {
    return (
      <LoadFailure
        noun="agent"
        error={agent.error}
        back={{ href: "/agents", label: "Back to agents" }}
        onRetry={() => void agent.refetch()}
        retrying={agent.isFetching}
      />
    );
  }

  const parsedSpec = agentSpecSchema.safeParse(agent.data.spec);
  if (!parsedSpec.success) {
    return (
      <EmptyState
        title="Configuration cannot be loaded"
        description={parsedSpec.error.issues.map((issue) => `${issue.path.join(".")}: ${issue.message}`).join("; ")}
        action={<Button asChild><Link href={`/agents/${agentId}`}>Back to agent</Link></Button>}
      />
    );
  }
  const initial: AgentDefinition = {
    apiVersion: "orbit.io/v1",
    kind: "Agent",
    metadata: {
      name: agent.data.name,
      description: agent.data.description,
      labels: (agent.data.labels as Record<string, string>) ?? {},
    },
    spec: parsedSpec.data,
  };

  async function handleSubmit(definition: AgentDefinition, commitAccess: CommitAccess) {
    setSubmitting(true);
    setServerIssues([]);
    try {
      const updated = unwrap(
        await api.PATCH("/api/v1/agents/{agent_id}", {
          params: { path: { agent_id: agentId } },
          body: {
            definition: toAgentDefinitionBody(definition),
            expectedVersion: startedFrom.current ?? undefined,
          },
        }),
      );
      // Access changes are committed only now that the agent itself was saved.
      const accessSaved = await commitAccess();
      // This save is now the baseline; the refresh below must not look like someone else's.
      startedFrom.current = updated.currentVersion;
      await queryClient.invalidateQueries({ queryKey: queryKeys.agent(agentId) });
      // Name, description and labels are saved in place; only a changed spec is a new version.
      const version = (updated as { currentVersion?: number }).currentVersion;
      const outcome = resourceSaveOutcome({
        mode: "edit",
        saved: version && version > agent.data!.currentVersion ? `Version ${version} created` : "Agent saved",
        href: `/agents/${agentId}`,
        accessSaved,
      });
      toast[outcome.tone](outcome.message);
      if (outcome.href) router.push(outcome.href);
    } catch (error) {
      const details = validationDetails(error);
      setServerIssues(details);
      toast.error(details.length > 1 ? "Fix the highlighted fields" : errorMessage(error));
    } finally {
      setSubmitting(false);
    }
  }

  const movedOn = startedFrom.current !== null && agent.data.currentVersion > startedFrom.current;

  return (
    <div className="space-y-6">
      <PageHeader
        back={{ href: `/agents/${agentId}`, label: agent.data.name }}
        title={`Edit ${agent.data.name}`}
        description="Saving creates a new immutable version. Past executions keep the configuration they ran with."
      />
      {movedOn ? (
        <Callout
          tone="warning"
          title={`Someone saved version ${agent.data.currentVersion} while you were editing`}
          action={
            <Button size="sm" variant="outline" onClick={() => window.location.reload()}>
              Reload
            </Button>
          }
        >
          You started from version {startedFrom.current}. Saving now would be refused so their
          changes are not overwritten. Copy anything you want to keep, then reload.
        </Callout>
      ) : null}
      <AgentForm
        serverIssues={serverIssues}
        initial={initial}
        submitLabel="Save changes"
        cancelHref={`/agents/${agentId}`}
        submitting={submitting}
        onSubmit={handleSubmit}
        resourceId={agentId}
      />
    </div>
  );
}
