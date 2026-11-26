"use client";

import { useQuery } from "@tanstack/react-query";

import { Badge } from "@/components/ui/badge";
import { listAccessTeams } from "@/lib/access-teams";

const VISIBILITY: Record<string, { label: string; variant: "secondary" | "info" | "success" }> = {
  private: { label: "Private", variant: "secondary" },
  team: { label: "Team", variant: "info" },
  org: { label: "Organization", variant: "success" },
};

/**
 * The owning team and visibility of a resource. Team names are resolved from the access
 * teams the caller may see; an owner the caller cannot see stays explicitly unnamed.
 */
export function ResourceAccess({
  ownerTeamId,
  visibility,
  className,
}: {
  ownerTeamId?: string | null;
  visibility?: string | null;
  className?: string;
}) {
  const teams = useQuery({ queryKey: ["access-teams"], queryFn: listAccessTeams, staleTime: 60_000 });
  const name = ownerTeamId
    ? teams.data?.find((team) => team.id === ownerTeamId)?.name ??
      (teams.isLoading ? "Loading team…" : "Team not visible")
    : visibility === "org"
      ? "Organization"
      : "Personal";
  const scope = VISIBILITY[visibility ?? ""] ?? {
    label: visibility ? visibility[0].toUpperCase() + visibility.slice(1) : "Unknown",
    variant: "secondary" as const,
  };
  return (
    <span className={className}>
      <span className="block truncate text-[13px] font-medium">{name}</span>
      <Badge variant={scope.variant} className="mt-1">
        {scope.label}
      </Badge>
    </span>
  );
}
