"use client";

import { useMemo, useState } from "react";
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
  skill: (id: string) => ["skills", id] as const,
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
    availableFeatures?: string[];
    limits: Record<string, number | null | undefined>;
    expiresAt?: string | null;
    graceEndsAt?: string | null;
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

export function useAllAgents(options: { status?: string; enabled?: boolean } = {}) {
  const { status, enabled = true } = options;
  return useQuery<AgentPage>({
    // Under the "agents" prefix, so every agent mutation refreshes it too.
    queryKey: queryKeys.agents(status ? { all: true, status } : { all: true }),
    enabled,
    queryFn: async () => {
      const items: AgentPage["items"] = [];
      let cursor: string | undefined;
      let total = 0;
      for (let page = 0; page < ALL_AGENTS_MAX_PAGES; page += 1) {
        const result = unwrap(
          await api.GET("/api/v1/agents", { params: { query: { limit: 100, cursor, status } } }),
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

type ExecutionListParams = {
  status?: string;
  agentId?: string;
  limit?: number;
  cursor?: string;
  archived?: boolean;
};

async function fetchExecutions(params: ExecutionListParams): Promise<ExecutionPage> {
  return unwrap(
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
  );
}

export function useExecutions(params: ExecutionListParams = {}) {
  return useQuery<ExecutionPage>({
    queryKey: queryKeys.executions(params),
    queryFn: () => fetchExecutions(params),
    refetchInterval: 10_000,
    placeholderData: (previous) => previous,
  });
}

/**
 * The newest page of executions, kept live, plus older pages loaded on request.
 *
 * Only the first page polls: history does not change, so re-fetching every loaded
 * page every ten seconds would be wasted work. Rows are de-duplicated because new
 * runs push older ones across the page boundary between fetches.
 */
export function usePagedExecutions(params: Omit<ExecutionListParams, "cursor">) {
  const first = useExecutions(params);
  const key = JSON.stringify(params);
  const [older, setOlder] = useState<{ key: string; pages: ExecutionPage[] }>({ key, pages: [] });
  const [loadingMore, setLoadingMore] = useState(false);
  const [loadError, setLoadError] = useState<unknown>(null);
  // Pages loaded under other filters belong to another list.
  const pages = older.key === key ? older.pages : [];

  const items = useMemo(() => {
    const seen = new Set<string>();
    const merged: ExecutionPage["items"] = [];
    for (const page of [first.data, ...pages]) {
      for (const item of page?.items ?? []) {
        if (seen.has(item.id)) continue;
        seen.add(item.id);
        merged.push(item);
      }
    }
    return merged;
  }, [first.data, pages]);

  // While a filter change is loading, `first.data` is still the previous filter's page
  // (kept on screen as a placeholder): its cursor belongs to another result set.
  const settled = !first.isPlaceholderData;
  const nextCursor = !settled
    ? undefined
    : pages.length
      ? pages[pages.length - 1].nextCursor
      : first.data?.nextCursor;

  async function loadMore() {
    if (!nextCursor || loadingMore) return;
    setLoadingMore(true);
    setLoadError(null);
    try {
      const page = await fetchExecutions({ ...params, cursor: nextCursor });
      setOlder((current) => ({
        key,
        pages: [...(current.key === key ? current.pages : []), page],
      }));
    } catch (error) {
      setLoadError(error);
    } finally {
      setLoadingMore(false);
    }
  }

  return {
    first,
    items,
    total: first.data?.total ?? 0,
    hasMore: Boolean(nextCursor),
    loadMore,
    loadingMore,
    loadError,
  };
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

export function useSkill(id: string | undefined) {
  return useQuery<Skill>({
    queryKey: queryKeys.skill(id ?? ""),
    queryFn: async () => unwrap(await api.GET("/api/v1/skills/{skill_id}", { params: { path: { skill_id: id! } } })),
    enabled: Boolean(id),
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

/**
 * Team runs, newest first. Pass `teamId` for one team's runs: the server filters, so a
 * busy organization cannot push this team's runs out of the page.
 */
export function useTeamRuns(options: { archived?: boolean; teamId?: string; limit?: number } = {}) {
  const { archived, teamId, limit } = options;
  return useQuery<TeamRun[]>({
    queryKey: [...queryKeys.teamRuns, archived ?? "all", teamId ?? "any", limit ?? "default"],
    queryFn: () => listTeamRuns({ archived, teamId, limit }),
    // Fast while something is running, slow otherwise (new runs still show up).
    refetchInterval: (query) =>
      query.state.data?.some((run) => !TERMINAL_RUN.has(run.status)) ? 3_000 : 15_000,
  });
}

export function useWorkerLaunches(includeInactive = false) {
  return useQuery<WorkerLaunch[]>({
    queryKey: [...queryKeys.workerLaunches, includeInactive ? "all" : "active"],
    queryFn: () => workerLaunches(includeInactive),
    refetchInterval: 5_000,
  });
}

export function useApprovals(
  status?: string,
  options: { refetchInterval?: number | false; enabled?: boolean; executionId?: string } = {},
) {
  const { executionId } = options;
  return useQuery<ApprovalWithExecution[]>({
    // Under the same prefix as the plain list, so a decision refreshes both.
    queryKey: executionId
      ? [...queryKeys.approvals(status), executionId]
      : queryKeys.approvals(status),
    enabled: options.enabled ?? true,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/approvals", {
          params: { query: { status, executionId, limit: 100 } },
        }),
      ),
    refetchInterval: options.refetchInterval ?? 5_000,
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
