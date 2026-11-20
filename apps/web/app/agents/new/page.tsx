"use client";

import { validationDetails } from "@/lib/http";
import { useRouter } from "next/navigation";
import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import { AgentForm } from "@/components/agents/agent-form";
import { PageHeader } from "@/components/ui/page-header";
import { api } from "@/lib/api";
import { toAgentDefinitionBody } from "@/lib/agent-body";
import { queryKeys, unwrap } from "@/lib/queries";
import { errorMessage } from "@orbit/api-client";
import type { AgentDefinition } from "@orbit/contracts";

export default function NewAgentPage() {
  const router = useRouter();
  const queryClient = useQueryClient();
  const [submitting, setSubmitting] = useState(false);
  const [serverIssues, setServerIssues] = useState<Array<{ path: string; message: string }>>([]);

  async function handleSubmit(definition: AgentDefinition) {
    setSubmitting(true);
    setServerIssues([]);
    try {
      const created = unwrap(
        await api.POST("/api/v1/agents", {
          body: toAgentDefinitionBody(definition),
        }),
      );
      toast.success("Agent created");
      void queryClient.invalidateQueries({ queryKey: queryKeys.agents() });
      void queryClient.invalidateQueries({ queryKey: queryKeys.dashboard });
      router.push(`/agents/${created.id}`);
    } catch (error) {
      const details = validationDetails(error);
      setServerIssues(details);
      toast.error(details.length > 1 ? "Fix the highlighted fields" : errorMessage(error));
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <div className="space-y-6">
      <PageHeader
        back={{ href: "/agents", label: "Agents" }}
        title="New agent"
        description="Define who the agent is, what it does and how much it may do alone. Saving creates version 1, which is immutable."
      />
      <AgentForm
        serverIssues={serverIssues}
        submitLabel="Create agent"
        submitting={submitting}
        onSubmit={handleSubmit}
      />
    </div>
  );
}
