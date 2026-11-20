"use client";

import { useQuery } from "@tanstack/react-query";
import { api } from "./api";
import { apiErrorFromBody, handleUnauthorized, isNoAccessError, isSessionError, request } from "./http";
import { listEnvMappings, workerProfileEnvMappings } from "./env-mappings";
import { listOutputs } from "./outputs";
import type {
  AgentDetail,
  AgentPage,
  AgentVersionView,
  ConnectionView,
  ConnectorView,
  DashboardResponse,
  ExecutionEventRecord,
  ExecutionPage,
  ExecutionRecord,
  MeResponse,
  MetaResponse,
  ProviderView,
  HarnessView,
  TriggerRecord,
  ApiTokenView,
  AgentInstanceView,
  ApprovalWithExecution,
  WorkerView,
  Skill,
  WorkerProfile,
  WorkerLaunch,
  AgentTeam,
  TeamRun,
  TeamRunDetail,
} from "./types";
import { getAgentTeam, getTeamRun, listAgentTeams, listTeamQueue, listTeamRuns, type TeamQueueItem } from "./teams";
import { workerLaunches } from "./launcher";

export function unwrap<T>(result: {
  data?: T;
  error?: unknown;
  response: Response;
}): T {
  if (result.error) {
    // Throw an ApiError so callers keep the HTTP status (401 vs 403 vs 5xx) and the message.
    if (result.response.status === 401) handleUnauthorized();
    throw apiErrorFromBody(result.response.status, result.error, result.response.statusText);
  }
  // DELETE and other no-content endpoints legitimately return no JSON body.
  // Treat a successful 204 as a resolved void instead of an error.
  if (result.response.status === 204) return undefined as T;
  if (result.data === undefined) {
    throw new Error(`HTTP ${result.response.status} ${result.response.statusText}`);
  }
  return result.data;
}

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
  triggers: (agentId?: string) => ["triggers", agentId ?? "all"] as const,
  tokens: ["api-tokens"] as const,
  instances: ["agent-instances"] as const,
  approvals: (status?: string) => ["approvals", status ?? "all"] as const,
  workers: ["workers"] as const,
  skills: ["skills"] as const,
  workerProfiles: ["worker-profiles"] as const,
  workerLaunches: ["worker-launches"] as const,
  agentTeams: ["agent-teams"] as const,
  agentTeam: (id: string) => ["agent-teams", id] as const,
  teamQueue: (id: string) => ["agent-teams", id, "queue"] as const,
  teamRuns: ["team-runs"] as const,
  teamRun: (id: string) => ["team-runs", "detail", id] as const,
};

export function useMeta() {
  return useQuery<MetaResponse>({
    queryKey: queryKeys.meta,
    queryFn: async () => unwrap(await api.GET("/api/v1/meta")),
    staleTime: 5 * 60_000,
  });
}

export interface LicenseOverview {
  license: {
    tier: string;
    customer?: string | null;
    plan?: string | null;
    features: string[];
    limits: Record<string, number | null | undefined>;
    expiresAt?: string | null;
    daysRemaining?: number | null;
    grace: boolean;
    valid: boolean;
    trial: boolean;
    error?: string | null;
  };
  usage: { seats: number; agents: number; workers: number; executionsToday: number };
  warnings: string[];
}

export function useOutputs() {
  return useQuery({
    queryKey: ["outputs"],
    queryFn: listOutputs,
    staleTime: 30_000,
  });
}

export function useEnvMappings() {
  return useQuery({
    queryKey: ["env-mappings"],
    queryFn: listEnvMappings,
    staleTime: 30_000,
  });
}

export function useWorkerProfileEnvMappings(profileId?: string) {
  return useQuery({
    queryKey: ["worker-profile-env-mappings", profileId],
    queryFn: () => workerProfileEnvMappings(profileId as string),
    enabled: Boolean(profileId),
    staleTime: 30_000,
  });
}

export function useLicense() {
  return useQuery<LicenseOverview>({
    queryKey: ["license"],
    queryFn: () => request<LicenseOverview>("/api/v1/license"),
    staleTime: 60_000,
  });
}

export function useMe(enabled = true) {
  return useQuery<MeResponse>({
    queryKey: queryKeys.me,
    enabled,
    queryFn: async () => unwrap(await api.GET("/api/v1/me")),
    staleTime: 60_000,
    // 401 (session over) and 403 (no access) will not heal by retrying. Transient
    // failures (5xx, network) back off exponentially (TanStack default: 1 s, 2 s, 4 s)
    // before the shell shows a retry state.
    retry: (failureCount, error) =>
      !isSessionError(error) && !isNoAccessError(error) && failureCount < 3,
  });
}

export function useDashboard() {
  return useQuery<DashboardResponse>({
    queryKey: queryKeys.dashboard,
    queryFn: async () => unwrap(await api.GET("/api/v1/dashboard/summary")),
    refetchInterval: 15_000,
  });
}

export function useAgents(
  params: {
    status?: string;
    q?: string;
    limit?: number;
    cursor?: string;
  } = {},
  options?: { enabled?: boolean },
) {
  return useQuery<AgentPage>({
    queryKey: queryKeys.agents(params),
    enabled: options?.enabled ?? true,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/agents", {
          params: {
            query: {
              limit: params.limit,
              cursor: params.cursor,
              status: params.status,
              q: params.q,
            },
          },
        }),
      ),
    placeholderData: (previous) => previous,
  });
}

/**
 * Every agent, for views that filter and count on the client. The API caps a page
 * at 100, so this follows `nextCursor` until the whole set is loaded (bounded so a
 * very large registry cannot stall the page).
 */
const ALL_AGENTS_MAX_PAGES = 10;

export function useAllAgents() {
  return useQuery<AgentPage>({
    queryKey: queryKeys.agents({ all: true }),
    queryFn: async () => {
      const items: AgentPage["items"] = [];
      let cursor: string | undefined;
      let total = 0;
      for (let page = 0; page < ALL_AGENTS_MAX_PAGES; page += 1) {
        const result = unwrap(
          await api.GET("/api/v1/agents", { params: { query: { limit: 100, cursor } } }),
        );
        items.push(...result.items);
        total = result.total;
        cursor = result.nextCursor ?? undefined;
        if (!cursor || result.items.length === 0 || items.length >= total) break;
      }
      return { items, total, nextCursor: cursor ?? null } as AgentPage;
    },
    placeholderData: (previous) => previous,
  });
}

export function useAgent(id: string | undefined) {
  return useQuery<AgentDetail>({
    queryKey: queryKeys.agent(id ?? ""),
    queryFn: async () => unwrap(await api.GET("/api/v1/agents/{agent_id}", { params: { path: { agent_id: id! } } })),
    enabled: Boolean(id),
    refetchInterval: 20_000,
  });
}

export function useAgentVersions(id: string | undefined) {
  return useQuery<AgentVersionView[]>({
    queryKey: queryKeys.agentVersions(id ?? ""),
    queryFn: async () =>
      unwrap(await api.GET("/api/v1/agents/{agent_id}/versions", { params: { path: { agent_id: id! } } })),
    enabled: Boolean(id),
  });
}

export function useExecutions(params: {
  status?: string;
  agentId?: string;
  limit?: number;
  cursor?: string;
  archived?: boolean;
} = {}) {
  return useQuery<ExecutionPage>({
    queryKey: queryKeys.executions(params),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/executions", {
          params: {
            query: {
              limit: params.limit,
              cursor: params.cursor,
              status: params.status,
              agentId: params.agentId,
              archived: params.archived,
            },
          },
        }),
      ),
    refetchInterval: 10_000,
    placeholderData: (previous) => previous,
  });
}

export function useExecution(id: string | undefined) {
  return useQuery<ExecutionRecord>({
    queryKey: queryKeys.execution(id ?? ""),
    queryFn: async () =>
      unwrap(await api.GET("/api/v1/executions/{execution_id}", { params: { path: { execution_id: id! } } })),
    enabled: Boolean(id),
    // Some status changes (picked up, cancel requested) arrive without an event on the
    // stream, so keep polling while the run is still going.
    refetchInterval: (query) =>
      ["queued", "dispatched", "running", "awaiting_approval"].includes(query.state.data?.status ?? "")
        ? 5_000
        : false,
  });
}

export function useExecutionEvents(id: string | undefined, options?: { enabled?: boolean }) {
  return useQuery<{ items: ExecutionEventRecord[] }>({
    queryKey: queryKeys.executionEvents(id ?? ""),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/executions/{execution_id}/events", {
          params: { path: { execution_id: id! }, query: { limit: 1000 } },
        }),
      ),
    enabled: Boolean(id) && (options?.enabled ?? true),
  });
}

/**
 * The organization's default harness, used when an agent has none of its own.
 * `known` is false while loading or when the caller may not read the setting.
 */
export function useDefaultHarness() {
  const query = useQuery({
    queryKey: ["harness-settings"],
    queryFn: () => request<{ defaultHarness?: string | null }>("/api/v1/settings/harness"),
    staleTime: 60_000,
    retry: false,
  });
  return { defaultHarness: query.data?.defaultHarness ?? null, known: query.isSuccess };
}

export function useHarnesses() {
  return useQuery<HarnessView[]>({
    queryKey: queryKeys.harnesses,
    queryFn: async () => unwrap(await api.GET("/api/v1/harnesses")),
    staleTime: 60_000,
  });
}

export function useProviders() {
  return useQuery<ProviderView[]>({
    queryKey: queryKeys.providers,
    queryFn: async () => unwrap(await api.GET("/api/v1/providers")),
  });
}

export function useConnectors() {
  return useQuery<ConnectorView[]>({
    queryKey: queryKeys.connectors,
    queryFn: async () => unwrap(await api.GET("/api/v1/connectors")),
    staleTime: 5 * 60_000,
  });
}

export function useConnections() {
  return useQuery<ConnectionView[]>({
    queryKey: queryKeys.connections,
    queryFn: async () => unwrap(await api.GET("/api/v1/connections")),
  });
}

export function useTriggers(agentId?: string) {
  return useQuery<TriggerRecord[]>({
    queryKey: queryKeys.triggers(agentId),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/triggers", {
          params: { query: { agentId: agentId } },
        }),
      ),
  });
}

export function useApiTokens() {
  return useQuery<ApiTokenView[]>({
    queryKey: queryKeys.tokens,
    queryFn: async () => unwrap(await api.GET("/api/v1/api-tokens")),
  });
}

export function useAgentInstances() {
  return useQuery<{ items: AgentInstanceView[]; total: number }>({
    queryKey: queryKeys.instances,
    queryFn: async () => unwrap(await api.GET("/api/v1/agent-instances")),
  });
}

export function useWorkers() {
  return useQuery<WorkerView[]>({
    queryKey: queryKeys.workers,
    queryFn: async () => unwrap(await api.GET("/api/v1/workers")),
    refetchInterval: 10_000,
  });
}

export function useSkills() {
  return useQuery<Skill[]>({
    queryKey: queryKeys.skills,
    queryFn: async () => unwrap(await api.GET("/api/v1/skills")),
  });
}

export function useWorkerProfiles() {
  return useQuery<WorkerProfile[]>({
    queryKey: queryKeys.workerProfiles,
    queryFn: async () => unwrap(await api.GET("/api/v1/worker-profiles")),
  });
}

export function useAgentTeams() {
  return useQuery<AgentTeam[]>({
    queryKey: queryKeys.agentTeams,
    queryFn: listAgentTeams,
  });
}

export function useAgentTeam(teamId: string) {
  return useQuery<AgentTeam>({
    queryKey: queryKeys.agentTeam(teamId),
    queryFn: () => getAgentTeam(teamId),
    enabled: Boolean(teamId),
  });
}

/** The team's work queue; polls while anything is pending or running. */
export function useTeamQueue(teamId: string) {
  return useQuery<TeamQueueItem[]>({
    queryKey: queryKeys.teamQueue(teamId),
    queryFn: () => listTeamQueue(teamId),
    enabled: Boolean(teamId),
    refetchInterval: (query) =>
      query.state.data?.some((item) => item.status === "pending" || item.status === "running") ? 4_000 : false,
  });
}

const TERMINAL_RUN = new Set(["succeeded", "completed", "failed", "cancelled", "timed_out", "rejected"]);

/** One run and its steps; polls until the run is finished. */
export function useTeamRun(runId: string) {
  return useQuery<TeamRunDetail>({
    queryKey: queryKeys.teamRun(runId),
    queryFn: () => getTeamRun(runId),
    enabled: Boolean(runId),
    refetchInterval: (query) => (query.state.data && TERMINAL_RUN.has(query.state.data.run.status) ? false : 3_000),
  });
}

export function useTeamRuns(archived?: boolean) {
  return useQuery<TeamRun[]>({
    queryKey: [...queryKeys.teamRuns, archived ?? "all"],
    queryFn: () => listTeamRuns(archived),
    refetchInterval: 3_000,
  });
}

export function useWorkerLaunches(includeInactive = false) {
  return useQuery<WorkerLaunch[]>({
    queryKey: [...queryKeys.workerLaunches, includeInactive ? "all" : "active"],
    queryFn: () => workerLaunches(includeInactive),
    refetchInterval: 5_000,
  });
}

export function useApprovals(status?: string) {
  return useQuery<ApprovalWithExecution[]>({
    queryKey: queryKeys.approvals(status),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/approvals", {
          params: { query: { status, limit: 100 } },
        }),
      ),
    refetchInterval: 5_000,
  });
}

export function useAuditLogs() {
  return useQuery<{
    items: Array<{
      id: number;
      action: string;
      actorType: string;
      actorEmail?: string | null;
      targetType?: string | null;
      targetId?: string | null;
      createdAt: string;
      metadata?: unknown;
    }>;
    nextCursor?: string | null;
  }>({
    queryKey: ["audit-logs"] as const,
    queryFn: async () =>
      unwrap(await api.GET("/api/v1/audit-logs", { params: { query: { limit: 25 } } })),
  });
}
