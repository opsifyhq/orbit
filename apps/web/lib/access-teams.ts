"use client";

import { request } from "./http";

export interface AccessTeam {
  id: string;
  name: string;
  slug: string;
  createdAt: string;
  updatedAt: string;
}

export interface TeamMemberRecord {
  teamId: string;
  userId: string;
  role: string;
  createdAt: string;
}

export function listAccessTeams(): Promise<AccessTeam[]> {
  return request<{ items: AccessTeam[] }>("/api/v1/teams?limit=500").then((page) => page.items);
}

export function createAccessTeam(body: { name: string; slug: string }): Promise<AccessTeam> {
  return request<AccessTeam>("/api/v1/teams", { method: "POST", body: JSON.stringify(body) });
}

export function deleteAccessTeam(id: string): Promise<void> {
  return request<void>(`/api/v1/teams/${id}`, { method: "DELETE" });
}

export function listTeamMembers(teamId: string): Promise<TeamMemberRecord[]> {
  return request<{ items: TeamMemberRecord[] }>(`/api/v1/teams/${teamId}/members?limit=500`).then(
    (page) => page.items,
  );
}

export function addTeamMember(
  teamId: string,
  body: { userId: string; role: string },
): Promise<TeamMemberRecord> {
  return request<TeamMemberRecord>(`/api/v1/teams/${teamId}/members`, {
    method: "POST",
    body: JSON.stringify(body),
  });
}

export function removeTeamMember(teamId: string, userId: string): Promise<void> {
  return request<void>(`/api/v1/teams/${teamId}/members/${userId}`, { method: "DELETE" });
}

export interface TeamRoleCounts {
  lead: number;
  member: number;
  viewer: number;
}

export interface TeamResourceCounts {
  total: number;
  agents: number;
  skills: number;
  connections: number;
  providers: number;
  workerProfiles: number;
  agentTeams: number;
  triggers: number;
  toolPolicies: number;
  envMappings: number;
}

export interface TeamAccessSummary {
  roles: TeamRoleCounts;
  resources: TeamResourceCounts;
  orgVisibleCount: number;
  canManageTeam: boolean;
}

export interface TeamResourceItem {
  id: string;
  resourceType: string;
  name: string;
  ownerName: string | null;
  ownerTeamId: string | null;
  isOwnerTeam: boolean;
  visibility: string;
  effectiveAccess: string[];
  updatedAt: string;
}

export interface TeamResourcesPage {
  items: TeamResourceItem[];
  nextCursor: string | null;
  total: number;
  summary: TeamAccessSummary;
}

export interface TeamResourceQuery {
  type?: string;
  cursor?: string;
  limit?: number;
}

/** Server-filtered team access overview. Never assemble this from resource list endpoints. */
export function listTeamResources(
  teamId: string,
  query: TeamResourceQuery = {},
): Promise<TeamResourcesPage> {
  const params = new URLSearchParams();
  if (query.type) params.set("type", query.type);
  if (query.cursor) params.set("cursor", query.cursor);
  if (query.limit) params.set("limit", String(query.limit));
  const suffix = params.toString();
  return request<TeamResourcesPage>(
    `/api/v1/teams/${teamId}/resources${suffix ? `?${suffix}` : ""}`,
  );
}

export interface ResourceSharing {
  visibility: string;
  ownerTeamId: string | null;
}

export function getResourceSharing(
  resourceType: string,
  resourceId: string,
): Promise<ResourceSharing> {
  return request<ResourceSharing>(`/api/v1/sharing/${resourceType}/${resourceId}`);
}

export function updateResourceSharing(
  resourceType: string,
  resourceId: string,
  body: ResourceSharing,
): Promise<void> {
  return request<void>(`/api/v1/sharing/${resourceType}/${resourceId}`, {
    method: "PATCH",
    body: JSON.stringify(body),
  });
}
