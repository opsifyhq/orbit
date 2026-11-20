import Link from "next/link";
import { Cable, RotateCcw, ShieldCheck, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { ListRow, ROW_LINK } from "@/components/ui/resource-list";
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
    <ListRow
      checkbox={
        onSelect ? (
          <Checkbox
            aria-label={`Select ${agent.name}`}
            checked={Boolean(selected)}
            onChange={(event) => onSelect(event.target.checked)}
          />
        ) : null
      }
      name={
        <>
          <p className="flex items-center gap-2">
            <Link href={`/agents/${agent.id}`} className={ROW_LINK}>
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
        </>
      }
      status={<AgentStatusBadge status={agent.status} />}
      details={
        <>
          <p className="truncate">{agent.harness ? brandName(agent.harness) : "Harness per run"}</p>
          <p className="mt-0.5 flex items-center gap-2.5 text-xs text-muted-foreground">
            <span className="inline-flex items-center gap-1">
              <ShieldCheck className="h-3.5 w-3.5" /> {autonomyName(agent.autonomyLevel)}
            </span>
            <span className="inline-flex items-center gap-1" title={`${agent.connectorCount} connectors`}>
              <Cable className="h-3.5 w-3.5" /> {agent.connectorCount}
            </span>
          </p>
        </>
      }
      updated={relativeTime(agent.updatedAt)}
      footer={
        archived && (onRestore || onDelete) ? (
          <>
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
          </>
        ) : null
      }
    />
  );
}
