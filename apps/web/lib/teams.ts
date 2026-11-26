"use client";

import type { components } from "@orbit/api-client";
import { request } from "./http";
import type { Workflow } from "./workflow";
import type {
  AgentTeam,
  TeamRun,
  TeamRunDetail,
  TeamRepository,
  WorkSource,
} from "./types";

/** A model override on the team or on one step (provider and model names). */
export type TeamModel = { provider?: string | null; model?: string | null; management?: string | null };

export type TeamStepInput = { agentId: string; model?: TeamModel | null };

/** Optional explicit team-wide autonomy; `follow_primary` is the server default. */
export type TeamAutonomy = {
  mode: "follow_primary" | "fixed";
  level?: number | null;
  approvalMode?: "auto" | "always" | "writes" | null;
  /** Server-recorded elevations (read-only; recomputed on every save). */
  elevations?: { agentId: string; baseline: { level: number; approvalMode?: "auto" | "always" | "writes" } }[];
};

export function listAgentTeams(): Promise<AgentTeam[]> {
  return request<{ items: AgentTeam[] }>("/api/v1/agent-teams?limit=500").then(
    (page) => page.items,
  );
}

export function getAgentTeam(teamId: string): Promise<AgentTeam> {
  return request<AgentTeam>(`/api/v1/agent-teams/${teamId}`);
}

/**
 * Create/update body. On update, fields left out keep their stored value and `null`
 * clears an optional field, so a cleared form field must be sent as `null`.
 */
export type TeamInput = {
  name: string;
  description: string;
  mode: "pipeline" | "native" | "workflow";
  /** The graph, for mode "workflow". `steps` is then derived by the API. */
  workflow?: Workflow | null;
  instructions: string;
  repository?: string | null;
  branch?: string | null;
  workSource?: WorkSource | null;
  repositories?: TeamRepository[];
  workerProfileId?: string | null;
  autoLaunch?: boolean;
  skills: string[];
  model?: TeamModel | null;
  harness?: string | null;
  connectionIds: string[];
  envMappingIds: string[];
  timeoutSeconds?: number | null;
  autonomy?: TeamAutonomy | null;
  steps: TeamStepInput[];
  /**
   * Update only: the team's `updatedAt` as loaded. The API refuses the save with 409 when
   * someone else changed the team since.
   */
  expectedUpdatedAt?: string;
};

export function createAgentTeam(input: TeamInput): Promise<AgentTeam> {
  return request<AgentTeam>("/api/v1/agent-teams", {
    method: "POST",
    body: JSON.stringify(input),
  });
}

export function updateAgentTeam(
  teamId: string,
  input: TeamInput,
): Promise<AgentTeam> {
  return request<AgentTeam>(`/api/v1/agent-teams/${teamId}`, {
    method: "PATCH",
    body: JSON.stringify(input),
  });
}

export function deleteAgentTeam(teamId: string): Promise<void> {
  return request<void>(`/api/v1/agent-teams/${teamId}`, { method: "DELETE" });
}

export function startTeamRun(
  teamId: string,
  task: string,
  repository?: string,
  autoApprove = false,
  harnessOverride?: string,
): Promise<TeamRunDetail> {
  return request<TeamRunDetail>(`/api/v1/agent-teams/${teamId}/runs`, {
    method: "POST",
    body: JSON.stringify({
      task,
      payload: repository?.trim() ? { repository: repository.trim() } : {},
      autoApprove,
      harnessOverride: harnessOverride || undefined,
    }),
  });
}

/** The generated schema types `input` as `unknown`; the web view uses the stricter local shape. */
export type TeamRunPage = { items: TeamRun[]; nextCursor?: string | null; total: number };

export function listTeamRuns(
  options: { archived?: boolean; teamId?: string; limit?: number; cursor?: string; q?: string } = {},
): Promise<TeamRunPage> {
  const params = new URLSearchParams();
  if (options.archived !== undefined) params.set("archived", String(options.archived));
  if (options.teamId) params.set("teamId", options.teamId);
  if (options.limit) params.set("limit", String(options.limit));
  if (options.cursor) params.set("cursor", options.cursor);
  if (options.q) params.set("q", options.q);
  const query = params.toString();
  return request<TeamRunPage>(`/api/v1/team-runs${query ? `?${query}` : ""}`);
}

export function getTeamRun(runId: string): Promise<TeamRunDetail> {
  return request<TeamRunDetail>(`/api/v1/team-runs/${runId}`);
}

/** Stop a running team run and the step executions still live in it. */
export function cancelTeamRun(runId: string): Promise<TeamRunDetail> {
  return request<TeamRunDetail>(`/api/v1/team-runs/${runId}/cancel`, { method: "POST", body: "{}" });
}

export type TeamQueueItem = components["schemas"]["TeamQueueItem"];

export function listTeamQueue(teamId: string): Promise<TeamQueueItem[]> {
  return request<{ items: TeamQueueItem[] }>(`/api/v1/agent-teams/${teamId}/queue?limit=500`).then(
    (page) => page.items,
  );
}

export function enqueueTeamItems(
  teamId: string,
  items: Array<{ externalKey: string; repository?: string; payload?: Record<string, unknown> }>,
): Promise<TeamQueueItem[]> {
  return request<TeamQueueItem[]>(`/api/v1/agent-teams/${teamId}/queue`, {
    method: "POST",
    body: JSON.stringify({ items }),
  });
}

export function runTeamQueueItem(
  teamId: string,
  itemId: string,
): Promise<TeamRunDetail> {
  return request<TeamRunDetail>(
    `/api/v1/agent-teams/${teamId}/queue/${itemId}/run`,
    { method: "POST", body: "{}" },
  );
}

export function retryTeamQueueItem(
  teamId: string,
  itemId: string,
): Promise<TeamQueueItem> {
  return request<TeamQueueItem>(
    `/api/v1/agent-teams/${teamId}/queue/${itemId}/retry`,
    { method: "POST", body: "{}" },
  );
}

export function requeueTeamQueueItem(
  teamId: string,
  itemId: string,
): Promise<TeamQueueItem> {
  return request<TeamQueueItem>(
    `/api/v1/agent-teams/${teamId}/queue/${itemId}/requeue`,
    { method: "POST", body: "{}" },
  );
}

export function cancelTeamQueueItem(
  teamId: string,
  itemId: string,
): Promise<TeamQueueItem> {
  return request<TeamQueueItem>(
    `/api/v1/agent-teams/${teamId}/queue/${itemId}/cancel`,
    { method: "POST", body: "{}" },
  );
}
