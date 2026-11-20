import { useCallback, useMemo } from "react";

import { useAgentTeams, useAllAgents, useWorkerProfiles } from "@/lib/queries";

export interface SkillUser {
  kind: "agent" | "team" | "profile";
  id: string;
  name: string;
  href: string;
}

/**
 * Who uses each skill, by slug: agents (current version), agent teams and worker
 * profiles. Only resources the viewer can see are counted.
 */
export function useSkillUsage() {
  const agents = useAllAgents();
  const teams = useAgentTeams();
  const profiles = useWorkerProfiles();

  const usage = useMemo(() => {
    const map = new Map<string, SkillUser[]>();
    const add = (slug: string, user: SkillUser) => map.set(slug, [...(map.get(slug) ?? []), user]);
    for (const agent of agents.data?.items ?? []) {
      if (agent.status === "archived") continue;
      for (const slug of agent.skills ?? []) {
        add(slug, { kind: "agent", id: agent.id, name: agent.name, href: `/agents/${agent.id}` });
      }
    }
    for (const team of teams.data ?? []) {
      for (const slug of team.definition.skills ?? []) {
        add(slug, { kind: "team", id: team.id, name: team.name, href: `/agent-teams/${team.id}` });
      }
    }
    for (const profile of profiles.data ?? []) {
      for (const slug of profile.skillSlugs ?? []) {
        add(slug, { kind: "profile", id: profile.id, name: profile.name, href: `/workers` });
      }
    }
    return map;
  }, [agents.data, teams.data, profiles.data]);

  const usersOf = useCallback((slug: string) => usage.get(slug) ?? [], [usage]);

  return {
    usage,
    usersOf,
    isLoading: agents.isLoading || teams.isLoading || profiles.isLoading,
  };
}

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

/** "2 agents, 1 team" — or "Not used". */
export function usageSummary(users: SkillUser[]): string {
  const agents = users.filter((user) => user.kind === "agent").length;
  const teams = users.filter((user) => user.kind === "team").length;
  const profiles = users.filter((user) => user.kind === "profile").length;
  const parts = [
    agents ? plural(agents, "agent") : null,
    teams ? plural(teams, "team") : null,
    profiles ? plural(profiles, "worker profile") : null,
  ].filter(Boolean);
  return parts.length ? parts.join(", ") : "Not used";
}
