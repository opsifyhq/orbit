/**
 * React Query keys. Kept free of React and the API client so plain `node --test` tests can
 * import it. `lib/queries.ts` re-exports `queryKeys`.
 */
export const queryKeys = {
  meta: ["meta"] as const,
  me: ["me"] as const,
  dashboard: ["dashboard"] as const,
  agents: (params?: Record<string, unknown>) => ["agents", params ?? {}] as const,
  agent: (id: string) => ["agent", id] as const,
  agentVersions: (id: string) => ["agent", id, "versions"] as const,
  executions: (params?: Record<string, unknown>) => ["executions", params ?? {}] as const,
  execution: (id: string) => ["execution", id] as const,
  executionEvents: (id: string) => ["execution", id, "events"] as const,
  harnesses: ["harnesses"] as const,
  providers: ["providers"] as const,
  connectors: ["connectors"] as const,
  connections: ["connections"] as const,
  tokens: ["api-tokens"] as const,
  approvals: (status?: string) => ["approvals", status ?? "all"] as const,
  workers: ["workers"] as const,
  skills: ["skills"] as const,
  skill: (id: string) => ["skills", id] as const,
  workerProfiles: ["worker-profiles"] as const,
  workerLaunches: ["worker-launches"] as const,
  agentTeams: ["agent-teams"] as const,
  agentTeam: (id: string) => ["agent-teams", id] as const,
  teamQueue: (id: string) => ["agent-teams", id, "queue"] as const,
  teamRuns: ["team-runs"] as const,
  teamRun: (id: string) => ["team-runs", "detail", id] as const,
};

/**
 * The list and detail queries to refresh after a resource's access (visibility or owning
 * team) changed. `resource` is the sharing API's resource name, e.g. "agent_teams".
 */
export function sharingInvalidationKeys(resource: string, id: string): (readonly unknown[])[] {
  // An access team's resource list shows what the team owns.
  const keys: (readonly unknown[])[] = [["sharing", resource, id], ["team-resources"]];
  switch (resource) {
    case "agents":
      // ["agents"] is the prefix of every agent list query.
      keys.push(["agents"], queryKeys.agent(id), queryKeys.dashboard);
      break;
    case "agent_teams":
      keys.push(queryKeys.agentTeams);
      break;
    case "worker_profiles":
      keys.push(queryKeys.workerProfiles);
      break;
    case "env_mappings":
      keys.push(["env-mappings"], ["worker-profile-env-mappings"]);
      break;
    case "providers":
      keys.push(queryKeys.providers);
      break;
    case "connections":
      keys.push(queryKeys.connections);
      break;
    case "skills":
      keys.push(queryKeys.skills);
      break;
    default:
      keys.push([resource]);
  }
  return keys;
}
