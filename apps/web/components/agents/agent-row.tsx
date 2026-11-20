import Link from "next/link";
import { Cable, RotateCcw, ShieldCheck, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { AgentStatusBadge } from "@/components/ui/status-badge";
import { relativeTime } from "@/lib/utils";
import { brandName, autonomyName } from "@/lib/presentation";
import type { AgentSummary } from "@/lib/types";

/**
 * One agent in the registry. A row, not a card: agents are compared against each
 * other (status, harness, autonomy), which needs aligned columns.
 */
export function AgentRow({
  agent,
  selected,
  onSelect,
  onRestore,
  onDelete,
}: {
  agent: AgentSummary;
  selected?: boolean;
  onSelect?: (selected: boolean) => void;
  onRestore?: (agent: AgentSummary) => void;
  onDelete?: (agent: AgentSummary) => void;
}) {
  const archived = agent.status === "archived";

  return (
    <li className="group relative border-b last:border-b-0">
      <div className="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-4 gap-y-2 px-4 py-3.5 transition-colors group-hover:bg-muted/50 sm:px-5 md:grid-cols-[1.5rem_minmax(0,1fr)_7rem_11rem_6.5rem]">
        {onSelect ? (
          <Checkbox
            aria-label={`Select ${agent.name}`}
            className="relative z-10"
            checked={Boolean(selected)}
            onChange={(event) => onSelect(event.target.checked)}
          />
        ) : null}
        <div className="min-w-0">
          <p className="flex items-center gap-2">
            <Link
              href={`/agents/${agent.id}`}
              className="truncate text-sm font-semibold after:absolute after:inset-0 focus-visible:outline-none focus-visible:after:outline focus-visible:after:outline-2 focus-visible:after:-outline-offset-2 focus-visible:after:outline-ring"
            >
              {agent.name}
            </Link>
            <span className="tabular shrink-0 text-xs text-muted-foreground">v{agent.currentVersion}</span>
          </p>
          <p className="mt-0.5 line-clamp-1 text-[13px] text-muted-foreground">
            {agent.description || "No description"}
          </p>
          <p className="mt-1.5 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted-foreground md:hidden">
            <span>{agent.harness ? brandName(agent.harness) : "Harness per run"}</span>
            <span>{autonomyName(agent.autonomyLevel)}</span>
            <span>{relativeTime(agent.updatedAt)}</span>
          </p>
        </div>

        <AgentStatusBadge status={agent.status} className="justify-self-end md:justify-self-start" />

        <div className="hidden min-w-0 text-[13px] md:block">
          <p className="truncate">{agent.harness ? brandName(agent.harness) : "Harness per run"}</p>
          <p className="mt-0.5 flex items-center gap-2.5 text-xs text-muted-foreground">
            <span className="inline-flex items-center gap-1">
              <ShieldCheck className="h-3.5 w-3.5" /> {autonomyName(agent.autonomyLevel)}
            </span>
            <span className="inline-flex items-center gap-1" title={`${agent.connectorCount} connectors`}>
              <Cable className="h-3.5 w-3.5" /> {agent.connectorCount}
            </span>
          </p>
        </div>

        <p className="tabular hidden text-right text-[13px] text-muted-foreground md:block">
          {relativeTime(agent.updatedAt)}
        </p>
      </div>

      {archived && (onRestore || onDelete) ? (
        <div className="relative flex flex-wrap gap-2 px-4 pb-3.5 sm:px-5">
          {onRestore ? (
            <Button size="sm" variant="outline" onClick={() => onRestore(agent)}>
              <RotateCcw /> Restore
            </Button>
          ) : null}
          {onDelete ? (
            <Button
              size="sm"
              variant="ghost"
              className="text-destructive hover:bg-destructive/10"
              onClick={() => onDelete(agent)}
            >
              <Trash2 /> Delete permanently
            </Button>
          ) : null}
        </div>
      ) : null}
    </li>
  );
}
