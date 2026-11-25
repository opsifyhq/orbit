"use client";

import { toast } from "sonner";
import { errorMessage } from "@orbit/api-client";

import { useConfirm } from "@/components/ui/confirm";
import { api } from "@/lib/api";
import { stillUsedMessage } from "@/lib/delete-impact";
import { unwrap } from "@/lib/queries";

/**
 * Ask before permanently deleting an agent (#434). Loads what the delete would be refused
 * for and what it removes: an agent still used by a team or worker profile is not offered
 * for deletion; otherwise the dialog lists the executions, triggers and outputs that go too.
 */
export function useConfirmAgentDelete() {
  const confirm = useConfirm();
  return async (agent: { id: string; name: string }): Promise<boolean> => {
    let impact: { usedBy: string[]; removes: string[] };
    try {
      impact = unwrap(
        await api.GET("/api/v1/agents/{agent_id}/delete-impact", {
          params: { path: { agent_id: agent.id } },
        }),
      );
    } catch (error) {
      toast.error(errorMessage(error));
      return false;
    }
    if (impact.usedBy.length) {
      toast.error(stillUsedMessage("agent", impact.usedBy));
      return false;
    }
    return confirm({
      title: `Delete ${agent.name} permanently?`,
      description: (
        <span className="block space-y-2">
          <span className="block">
            {impact.removes.length
              ? "This removes the agent together with:"
              : "This removes the agent. It has no history, triggers or outputs."}
          </span>
          {impact.removes.length ? (
            <span className="block">
              {impact.removes.map((line) => (
                <span key={line} className="block">
                  • {line}
                </span>
              ))}
            </span>
          ) : null}
          <span className="block">It cannot be undone.</span>
        </span>
      ),
      confirmLabel: "Delete agent",
      destructive: true,
    });
  };
}
